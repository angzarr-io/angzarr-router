#include <catch2/catch.hpp>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include "angzarr/router/router.h"
#include "builders.h"
#include "gherkin.h"

namespace {
using namespace angzarr::conformance;
using angzarr::router::AggregateDispatch;
using angzarr::router::CodedError;
using angzarr::router::CommandContext;
using angzarr::router::GrpcCode;
using angzarr::router::Rebuilder;

// A business response carrying one header-less event page of test.counter.<name>.
pb::BusinessResponse OneEvent(const std::string& name) {
  pb::BusinessResponse resp;
  SetAnyEmpty(resp.mutable_events()->add_pages()->mutable_event(), "test.counter." + name);
  return resp;
}

// The payment aggregate (domain "payment"): one compensator per (compensates
// entry, emitted event name) pair.
AggregateDispatch<tc::CounterState> PaymentAggregate(
    const std::vector<std::pair<std::string, std::string>>& entries) {
  AggregateDispatch<tc::CounterState> agg("Payment", "payment", Rebuilder<tc::CounterState>{});
  for (const auto& [key, event] : entries) {
    agg.OnRejected(
        key, [event](const pb::Notification&, const pb::RejectionNotification&, tc::CounterState&,
                     const CommandContext&) { return std::optional(OneEvent(event)); });
  }
  return agg;
}

// The inventory aggregate (domain "inventory"): undoes AdjustStock with
// StockAdjustmentReverted and Reserve with StockReleased.
AggregateDispatch<tc::CounterState> InventoryAggregate() {
  AggregateDispatch<tc::CounterState> agg("Inventory", "inventory", Rebuilder<tc::CounterState>{});
  auto undo = [](const std::string& event) {
    return [event](const pb::Notification&, const pb::Compensate&, tc::CounterState&,
                   const CommandContext&) { return std::optional(OneEvent(event)); };
  };
  agg.OnUndo("test.counter.AdjustStock", undo("StockAdjustmentReverted"))
      .OnUndo("test.counter.Reserve", undo("StockReleased"));
  return agg;
}

// A Notification command addressed to domain wrapping payload, over prior
// history whose next sequence is next_sequence when given.
pb::ContextualCommand NotificationCommand(const std::string& domain,
                                          const google::protobuf::Any& payload,
                                          std::optional<uint32_t> next_sequence) {
  pb::Notification n;
  *n.mutable_payload() = payload;
  pb::ContextualCommand cc;
  auto* book = cc.mutable_command();
  book->mutable_cover()->set_domain(domain);
  *book->add_pages()->mutable_command() = angzarr::router::Pack::Wrap(n);
  if (next_sequence) {
    auto* events = cc.mutable_events();
    events->set_next_sequence(*next_sequence);
    auto* page = events->add_pages();
    page->mutable_header()->set_sequence(*next_sequence > 0 ? *next_sequence - 1 : 0);
    SetAnyEmpty(page->mutable_event(), "test.counter.Unrelated");
  }
  return cc;
}

// The rejection of test.counter.<command> sent to target_domain, delivered to
// the payment aggregate.
pb::ContextualCommand RejectionSentTo(const std::string& command, const std::string& target_domain,
                                      std::optional<uint32_t> next_sequence) {
  pb::RejectionNotification rejection;
  auto* rc = rejection.mutable_rejected_command();
  rc->mutable_cover()->set_domain(target_domain);
  SetAnyEmpty(rc->add_pages()->mutable_command(), "test.counter." + command);
  return NotificationCommand("payment", angzarr::router::Pack::Wrap(rejection), next_sequence);
}

struct CompensationWorld {
  angzarr::router::Router router;
  std::optional<pb::BusinessResponse> resp;
  std::optional<CodedError> err;

  void Dispatch(const pb::ContextualCommand& cc) {
    try {
      resp = router.Dispatch(cc);
      err.reset();
    } catch (const CodedError& e) {
      err = e;
      resp.reset();
    }
  }

  const pb::EventBook& Book() const {
    REQUIRE_FALSE(err.has_value());
    REQUIRE(resp.has_value());
    return resp->events();
  }
};

void Register(StepRegistry& r, CompensationWorld& w) {
  r.On("a payment aggregate compensating Reserve from any domain with {word}",
       [&w](const StepArgs& a) {
         w.router.RegisterAggregate(PaymentAggregate({{"test.counter.Reserve", a[0]}}));
       });
  r.On("a second payment aggregate compensating Reserve from any domain with {word}",
       [&w](const StepArgs& a) {
         w.router.RegisterAggregate(PaymentAggregate({{"test.counter.Reserve", a[0]}}));
       });
  r.On(
      "a payment aggregate compensating Reserve from {string} with {word} and from {string} with "
      "{word}",
      [&w](const StepArgs& a) {
        w.router.RegisterAggregate(PaymentAggregate(
            {{a[0] + ":test.counter.Reserve", a[1]}, {a[2] + ":test.counter.Reserve", a[3]}}));
      });
  r.On(
      "an inventory aggregate undoing AdjustStock with StockAdjustmentReverted and Reserve with "
      "StockReleased",
      [&w](const StepArgs&) { w.router.RegisterAggregate(InventoryAggregate()); });

  r.On(
      "a rejection of {word} sent to {string} is dispatched to the payment aggregate over history "
      "ending at sequence {int}",
      [&w](const StepArgs& a) {
        w.Dispatch(RejectionSentTo(a[0], a[1], static_cast<uint32_t>(std::stoi(a[2]) + 1)));
      });
  r.On("a rejection of {word} sent to {string} is dispatched to the payment aggregate",
       [&w](const StepArgs& a) { w.Dispatch(RejectionSentTo(a[0], a[1], std::nullopt)); });
  r.On("a Compensate for {word} is dispatched to the inventory aggregate", [&w](const StepArgs& a) {
    w.Dispatch(NotificationCommand("inventory", CompensatePayload(a[0]), std::nullopt));
  });

  r.On("the aggregate emits one {word} event", [&w](const StepArgs& a) {
    const auto& book = w.Book();
    REQUIRE(book.pages_size() == 1);
    REQUIRE(FqFromUrl(book.pages(0).event().type_url()) == "test.counter." + a[0]);
  });
  r.On("the aggregate emits nothing", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp.has_value());
    REQUIRE(w.resp->events().pages_size() == 0);
  });
  r.On("the emitted event takes sequence {int}", [&w](const StepArgs& a) {
    const auto& book = w.Book();
    REQUIRE(book.pages_size() >= 1);
    REQUIRE(book.pages(0).header().sequence_type_case() == pb::PageHeader::kSequence);
    REQUIRE(static_cast<int>(book.pages(0).header().sequence()) == std::stoi(a[0]));
  });
  r.On("the aggregates emit {word} at sequence {int} then {word} at sequence {int}",
       [&w](const StepArgs& a) {
         const auto& book = w.Book();
         std::vector<std::pair<std::string, int>> got;
         for (const auto& page : book.pages()) {
           REQUIRE(page.header().sequence_type_case() == pb::PageHeader::kSequence);
           got.emplace_back(FqFromUrl(page.event().type_url()),
                            static_cast<int>(page.header().sequence()));
         }
         const std::vector<std::pair<std::string, int>> want{
             {"test.counter." + a[0], std::stoi(a[1])}, {"test.counter." + a[2], std::stoi(a[3])}};
         REQUIRE(got == want);
       });
  r.On("the dispatch fails with {word} as UNIMPLEMENTED", [&w](const StepArgs& a) {
    REQUIRE(w.err.has_value());
    REQUIRE(w.err->code == a[0]);
    REQUIRE(w.err->grpc == GrpcCode::kUnimplemented);
  });
}

}  // namespace

TEST_CASE("compensation routing", "[compensation]") {
  for (auto& sc : ParseFeature(FeaturePath("compensation.feature"))) {
    DYNAMIC_SECTION(sc.name) {
      CompensationWorld world;
      StepRegistry reg;
      Register(reg, world);
      for (const auto& step : sc.steps) reg.Run(step.text);
    }
  }
}
