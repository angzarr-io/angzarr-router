#include <catch2/catch.hpp>
#include <optional>
#include <string>
#include <vector>

#include "angzarr/router/router.h"
#include "helpers.h"
#include "test/counter/counter.pb.h"

namespace {

using namespace angzarr::router;
using namespace angzarr::router::testing;
namespace tc = test::counter;

pb::BusinessResponse OnePage(const std::string& fq) {
  pb::BusinessResponse resp;
  *resp.mutable_events()->add_pages()->mutable_event() = EmptyAny(fq);
  return resp;
}

// A Compensate for an executed fq_command, delivered to the "counter" domain.
pb::ContextualCommand CompensateOf(const std::string& fq_command) {
  pb::Compensate compensate;
  compensate.set_command_type(fq_command);
  compensate.set_reason("aborted");
  pb::Notification n;
  *n.mutable_payload() = Pack::Wrap(compensate);
  pb::ContextualCommand cc;
  auto* book = cc.mutable_command();
  book->mutable_cover()->set_domain("counter");
  book->mutable_cover()->mutable_root()->set_value("root-1");
  *book->add_pages()->mutable_command() = Pack::Wrap(n);
  return cc;
}

}  // namespace

TEST_CASE("an undo handler receives the Compensate and the handled cover", "[undo]") {
  Router router;
  std::vector<std::string> undone;
  std::vector<std::string> roots;
  AggregateDispatch<tc::CounterState> d("Counter", "counter", Rebuilder<tc::CounterState>{});
  d.OnUndo("test.counter.Reserve", [&](const pb::Notification&, const pb::Compensate& compensate,
                                       tc::CounterState&, const CommandContext& cctx) {
    undone.push_back(compensate.command_type());
    roots.push_back(cctx.cover.root().value());
    return std::optional(OnePage("test.counter.StockReleased"));
  });
  router.RegisterAggregate(std::move(d));

  auto resp = router.Dispatch(CompensateOf("test.counter.Reserve"));
  REQUIRE(undone == std::vector<std::string>{"test.counter.Reserve"});
  REQUIRE(roots == std::vector<std::string>{"root-1"});
  REQUIRE(resp.events().pages_size() == 1);
  REQUIRE(resp.events().pages(0).event().type_url() == "/test.counter.StockReleased");
}

TEST_CASE("an undo handler returning no result undoes nothing", "[undo]") {
  Router router;
  AggregateDispatch<tc::CounterState> d("Counter", "counter", Rebuilder<tc::CounterState>{});
  d.OnUndo(
      "test.counter.Reserve",
      [](const pb::Notification&, const pb::Compensate&, tc::CounterState&,
         const CommandContext&) -> std::optional<pb::BusinessResponse> { return std::nullopt; });
  router.RegisterAggregate(std::move(d));

  auto resp = router.Dispatch(CompensateOf("test.counter.Reserve"));
  REQUIRE(resp.events().pages_size() == 0);
}

TEST_CASE("a compensator reads the handled cover", "[compensation]") {
  Router router;
  std::vector<std::string> domains;
  AggregateDispatch<tc::CounterState> d("Counter", "counter", Rebuilder<tc::CounterState>{});
  d.OnRejected("test.counter.Reserve",
               [&domains](const pb::Notification&, const pb::RejectionNotification&,
                          tc::CounterState&, const CommandContext& cctx) {
                 domains.push_back(cctx.cover.domain());
                 return std::optional(OnePage("test.counter.CompensatedFirst"));
               });
  router.RegisterAggregate(std::move(d));

  router.Dispatch(RejectionOf("test.counter.Reserve"));
  REQUIRE(domains == std::vector<std::string>{"counter"});
}

namespace {

AggregateDispatch<tc::CounterState> Ledger() {
  Rebuilder<tc::CounterState> rebuilder;
  rebuilder.Apply("test.counter.Increased",
                  [](tc::CounterState& state, const google::protobuf::Any&) {
                    state.set_count(state.count() + 1);
                  });
  return AggregateDispatch<tc::CounterState>("Ledger", "ledger", std::move(rebuilder));
}

pb::FactRequest FactsOf(const std::string& fq, int n) {
  pb::FactRequest req;
  req.mutable_facts()->mutable_cover()->set_domain("ledger");
  for (int i = 0; i < n; ++i) *req.mutable_facts()->add_pages()->mutable_event() = EmptyAny(fq);
  return req;
}

std::vector<std::string> TypesOf(const pb::EventBook& book) {
  std::vector<std::string> types;
  for (const auto& page : book.pages()) types.push_back(page.event().type_url());
  return types;
}

}  // namespace

TEST_CASE("a fact handler returning the fact records it as received", "[fact]") {
  Router router;
  std::vector<uint32_t> saw;
  auto d = Ledger();
  d.OnFact("test.counter.Increased",
           [&saw](const google::protobuf::Any& fact, const tc::CounterState& state) -> FactRecord {
             saw.push_back(state.count());
             return fact;
           });
  router.RegisterAggregate(std::move(d));

  auto book = router.DispatchFact(FactsOf("test.counter.Increased", 2));
  REQUIRE(TypesOf(book) ==
          std::vector<std::string>{"/test.counter.Increased", "/test.counter.Increased"});
  REQUIRE(saw == std::vector<uint32_t>{0, 1});
}

TEST_CASE("a fact handler replaces the fact and appends its flags in order", "[fact]") {
  Router router;
  auto d = Ledger();
  d.OnFact("test.counter.Increased", [](const google::protobuf::Any&, const tc::CounterState&) {
    tc::CounterState annotated;
    annotated.set_count(7);
    tc::CounterState last;
    last.set_count(9);
    return FactRecord(Pack::Wrap(annotated), {EmptyAny("test.counter.Reserve"), Pack::Wrap(last)});
  });
  router.RegisterAggregate(std::move(d));

  auto book = router.DispatchFact(FactsOf("test.counter.Increased", 1));
  REQUIRE(TypesOf(book) == std::vector<std::string>{"/test.counter.CounterState",
                                                    "/test.counter.Reserve",
                                                    "/test.counter.CounterState"});
  tc::CounterState first;
  REQUIRE(first.ParseFromString(book.pages(0).event().value()));
  REQUIRE(first.count() == 7);
  tc::CounterState third;
  REQUIRE(third.ParseFromString(book.pages(2).event().value()));
  REQUIRE(third.count() == 9);
}

TEST_CASE("a fact of an undeclared type is refused before any handler runs", "[fact]") {
  Router router;
  int handled = 0;
  auto d = Ledger();
  d.OnFact("test.counter.Increased",
           [&handled](const google::protobuf::Any& fact, const tc::CounterState&) -> FactRecord {
             ++handled;
             return fact;
           });
  router.RegisterAggregate(std::move(d));

  try {
    router.DispatchFact(FactsOf("test.counter.Reserve", 1));
    FAIL("an undeclared fact was recorded");
  } catch (const CodedError& e) {
    REQUIRE(e.code == "NO_FACT_HANDLER");
    REQUIRE(e.grpc == GrpcCode::kInvalidArgument);
  }
  REQUIRE(handled == 0);
}

TEST_CASE("replay packs the rebuilt state under the bare-slash type URL", "[replay]") {
  Router router;
  Rebuilder<tc::CounterState> rebuilder;
  rebuilder.Apply("test.counter.Increased",
                  [](tc::CounterState& state, const google::protobuf::Any&) {
                    state.set_count(state.count() + 1);
                  });
  router.RegisterAggregate(
      AggregateDispatch<tc::CounterState>("Counter", "counter", std::move(rebuilder)));

  pb::ReplayRequest req;
  for (uint32_t i = 0; i < 3; ++i) {
    auto* page = req.add_events();
    page->mutable_header()->set_sequence(i);
    *page->mutable_event() = EmptyAny("test.counter.Increased");
  }
  auto resp = router.DispatchReplay("", req);
  REQUIRE(resp.state().type_url() == "/test.counter.CounterState");
  tc::CounterState state;
  REQUIRE(state.ParseFromString(resp.state().value()));
  REQUIRE(state.count() == 3);
}

TEST_CASE("a saga handler sees its declared target domains", "[saga]") {
  Router router;
  std::vector<std::string> seen;
  bool has_inventory = false;
  bool has_billing = true;
  SagaDispatch d("order-saga", "order", {"inventory", "audit"});
  d.OnEvent("test.counter.Increased",
            [&](const google::protobuf::Any&, const Destinations& dests, const pb::Cover&) {
              seen = dests.Domains();
              has_inventory = dests.Has("inventory");
              has_billing = dests.Has("billing");
              return SagaEmission{};
            });
  router.RegisterSaga(std::move(d));

  pb::SagaHandleRequest req;
  req.mutable_source()->mutable_cover()->set_domain("order");
  *req.mutable_source()->add_pages()->mutable_event() = EmptyAny("test.counter.Increased");
  router.DispatchSaga(req);
  REQUIRE(seen == std::vector<std::string>{"inventory", "audit"});
  REQUIRE(has_inventory);
  REQUIRE_FALSE(has_billing);
}

TEST_CASE("a process-manager handler sees its targets and the trigger cover", "[pm]") {
  Router router;
  std::vector<std::string> targets;
  std::string trigger_root;
  ProcessManagerDispatch<tc::OrderProcessManagerState> d("order-pm", "order-pm", {"inventory"},
                                                         Rebuilder<tc::OrderProcessManagerState>{});
  d.OnEventWithCover("counter", "test.counter.Increased",
                     [&](const google::protobuf::Any&, tc::OrderProcessManagerState&,
                         const Destinations& dests, const pb::Cover& cover) {
                       targets = dests.Domains();
                       trigger_root = cover.root().value();
                       return pb::ProcessManagerHandleResponse{};
                     });
  router.RegisterProcessManager(std::move(d));

  pb::ProcessManagerHandleRequest req;
  auto* trigger = req.mutable_trigger();
  trigger->mutable_cover()->set_domain("counter");
  trigger->mutable_cover()->mutable_root()->set_value("table-1");
  *trigger->add_pages()->mutable_event() = EmptyAny("test.counter.Increased");
  router.DispatchProcessManager(req);
  REQUIRE(targets == std::vector<std::string>{"inventory"});
  REQUIRE(trigger_root == "table-1");
}

TEST_CASE("a process-manager compensator's full response carries deferred commands", "[pm]") {
  Router router;
  ProcessManagerDispatch<tc::OrderProcessManagerState> d("order-pm", "order-pm", {"inventory"},
                                                         Rebuilder<tc::OrderProcessManagerState>{});
  d.OnRejectedWithResponse(
      "test.counter.Reserve",
      [](const pb::Notification&, const pb::RejectionNotification&, tc::OrderProcessManagerState&) {
        pb::ProcessManagerHandleResponse resp;
        auto* cmd = resp.add_commands();
        cmd->mutable_cover()->set_domain("inventory");
        *cmd->add_pages()->mutable_command() = EmptyAny("test.counter.Release");
        resp.add_facts();
        return resp;
      });
  router.RegisterProcessManager(std::move(d));

  pb::RejectionNotification rejection;
  rejection.mutable_rejected_command()->mutable_cover()->set_domain("inventory");
  *rejection.mutable_rejected_command()->add_pages()->mutable_command() =
      EmptyAny("test.counter.Reserve");
  pb::Notification n;
  *n.mutable_payload() = Pack::Wrap(rejection);
  pb::ProcessManagerHandleRequest req;
  auto* trigger = req.mutable_trigger();
  trigger->mutable_cover()->set_domain("order-pm");
  auto* page = trigger->add_pages();
  page->mutable_header()->set_sequence(5);
  *page->mutable_event() = Pack::Wrap(n);
  auto resp = router.DispatchProcessManager(req);

  REQUIRE(resp.commands_size() == 1);
  REQUIRE(resp.facts_size() == 1);
  const auto& header = resp.commands(0).pages(0).header();
  REQUIRE(header.sequence_type_case() == pb::PageHeader::kAngzarrDeferred);
  REQUIRE(header.angzarr_deferred().source_seq() == 5);
}

TEST_CASE("a projector fold reads its event's cover and sequence", "[projector]") {
  Router router;
  std::vector<std::pair<std::string, uint32_t>> seen;
  ProjectorDispatch<tc::CounterProjectorState> d("tracker");
  d.OnEventWithContext(
      "test.counter.Increased",
      [&seen](tc::CounterProjectorState&, const google::protobuf::Any&, const PageContext& ctx) {
        seen.emplace_back(ctx.cover.domain(), ctx.sequence);
      });
  router.RegisterProjector(std::move(d));

  pb::EventBook book;
  book.mutable_cover()->set_domain("counter");
  auto* first = book.add_pages();
  first->mutable_header()->set_sequence(4);
  *first->mutable_event() = EmptyAny("test.counter.Increased");
  *book.add_pages()->mutable_event() = EmptyAny("test.counter.Increased");
  router.DispatchProjector(book);
  REQUIRE(seen == std::vector<std::pair<std::string, uint32_t>>{{"counter", 4}, {"counter", 0}});
}
