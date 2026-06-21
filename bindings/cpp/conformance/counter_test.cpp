#include <catch2/catch.hpp>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

#include "builders.h"
#include "gherkin.h"
#include "test/counter/counter_aggregate_angzarr.h"

namespace {
using namespace angzarr::conformance;
using angzarr::router::CodedError;
using angzarr::router::CommandContext;

struct Observation {
  bool had_prior;
  uint32_t next_seq;
  uint32_t count;
};

// The conformance CounterAggregate fixture, implementing the generated seam.
class CounterFixture : public tc::CounterAggregateHandler {
 public:
  explicit CounterFixture(std::vector<Observation>& observed) : observed_(observed) {}

  std::vector<tc::Increased> IncreaseBy(const tc::IncreaseBy& cmd, tc::CounterState& state,
                                        const CommandContext& cctx) override {
    observed_.push_back({cctx.had_prior_events, cctx.next_sequence, state.count()});
    if (cmd.n() == 0) {
      throw CodedError::Reject("VALUE_NOT_POSITIVE", "increase amount must be positive");
    }
    std::vector<tc::Increased> events(cmd.n());
    return events;
  }
  pb::EventBook FailHard(const tc::FailHard&, tc::CounterState&, const CommandContext&) override {
    throw std::runtime_error("hard failure");
  }
  void ApplyIncreased(tc::CounterState& state, const tc::Increased&) override {
    state.set_count(state.count() + 1);
  }
  pb::BusinessResponse OnReserveRejected(const pb::Notification&, const pb::RejectionNotification&,
                                         tc::CounterState&, const CommandContext&) override {
    pb::BusinessResponse resp;
    auto* book = resp.mutable_events();
    SetAnyEmpty(book->add_pages()->mutable_event(), "test.counter.CompensatedFirst");
    SetAnyEmpty(book->add_pages()->mutable_event(), "test.counter.CompensatedSecond");
    return resp;
  }

 private:
  std::vector<Observation>& observed_;
};

struct CounterWorld {
  angzarr::router::Router router;
  std::vector<Observation> observed;
  CounterFixture fixture{observed};
  std::optional<pb::EventBook> prior;
  std::optional<pb::BusinessResponse> resp;
  std::optional<CodedError> err;

  CounterWorld() { tc::RegisterCounterAggregate(router, fixture); }

  void Dispatch(pb::ContextualCommand cc) {
    if (prior) *cc.mutable_events() = *prior;
    try {
      resp = router.Dispatch(cc);
      err.reset();
    } catch (const CodedError& e) {
      err = e;
      resp.reset();
    }
  }
};

std::string FqFromUrl(const std::string& url) {
  const auto i = url.rfind('/');
  return i == std::string::npos ? url : url.substr(i + 1);
}

void Register(StepRegistry& r, CounterWorld& w) {
  r.On("a new counter", [&w](const StepArgs&) { w.prior.reset(); });
  r.On("a counter that has already recorded {int} increase(s)",
       [&w](const StepArgs& a) { w.prior = PriorIncreases(std::stoi(a[0])); });
  r.On("a counter whose history holds a corrupt event",
       [&w](const StepArgs&) { w.prior = CorruptHistory(); });
  r.On("a counter restored from a snapshot of 10 with one newer event",
       [&w](const StepArgs&) { w.prior = SnapshotHistory(); });

  r.On("the operator increases the counter by {int}",
       [&w](const StepArgs& a) { w.Dispatch(IncreaseCommand(std::stoi(a[0]))); });
  r.On("the operator increases the counter by {int} on behalf of a parent",
       [&w](const StepArgs& a) { w.Dispatch(IncreaseCommandWithLinkage(std::stoi(a[0]))); });
  r.On("the operator triggers a hard failure",
       [&w](const StepArgs&) { w.Dispatch(FailHardCommand()); });
  r.On("an unhandled command is dispatched",
       [&w](const StepArgs&) { w.Dispatch(UnhandledCommand()); });
  r.On("a command with no command book is dispatched",
       [&w](const StepArgs&) { w.Dispatch(CommandMissingBook()); });
  r.On("a command with an empty command book is dispatched",
       [&w](const StepArgs&) { w.Dispatch(CommandMissingPage()); });
  r.On("a command whose page carries no payload is dispatched",
       [&w](const StepArgs&) { w.Dispatch(CommandMissingPayload()); });
  r.On("a Reserve command is rejected",
       [&w](const StepArgs&) { w.Dispatch(RejectionCommand("test.counter.Reserve")); });
  r.On("an unregistered command is rejected",
       [&w](const StepArgs&) { w.Dispatch(RejectionCommand("test.counter.Undeclared")); });

  auto recorded = [&w](int count, int start) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp.has_value());
    const auto& book = w.resp->events();
    REQUIRE(book.pages_size() == count);
    for (int i = 0; i < count; ++i) {
      REQUIRE(static_cast<int>(book.pages(i).header().sequence()) == start + i);
    }
  };
  r.On("{int} increases are recorded, starting at sequence {int}",
       [recorded](const StepArgs& a) { recorded(std::stoi(a[0]), std::stoi(a[1])); });
  r.On("{int} increases are recorded, continuing from sequence {int}",
       [recorded](const StepArgs& a) { recorded(std::stoi(a[0]), std::stoi(a[1])); });

  auto fails_with = [&w](const std::string& code) {
    REQUIRE(w.err.has_value());
    REQUIRE(w.err->code == code);
  };
  r.On("the command is rejected as {word}", [fails_with](const StepArgs& a) { fails_with(a[0]); });
  r.On("the command fails with {word}", [fails_with](const StepArgs& a) { fails_with(a[0]); });

  r.On("no events are recorded",
       [&w](const StepArgs&) { REQUIRE((w.resp ? w.resp->events().pages_size() : 0) == 0); });
  r.On("the recorded events carry the parent linkage", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp.has_value());
    REQUIRE(w.resp->events().cover().ext().SerializeAsString() ==
            ParentLinkage().SerializeAsString());
  });
  r.On("the compensations run first then second", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    const auto& book = w.resp->events();
    REQUIRE(book.pages_size() == 2);
    REQUIRE(FqFromUrl(book.pages(0).event().type_url()) == "test.counter.CompensatedFirst");
    REQUIRE(FqFromUrl(book.pages(1).event().type_url()) == "test.counter.CompensatedSecond");
  });
  r.On("no compensation is recorded", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp.has_value());
    REQUIRE(w.resp->events().pages_size() == 0);
  });

  auto history = [&w](bool want_prior, int next_seq) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE_FALSE(w.observed.empty());
    const auto& o = w.observed.back();
    REQUIRE(o.had_prior == want_prior);
    REQUIRE(static_cast<int>(o.next_seq) == next_seq);
  };
  r.On("the handler saw prior history, at next sequence {int}",
       [history](const StepArgs& a) { history(true, std::stoi(a[0])); });
  r.On("the handler saw no prior history, at next sequence {int}",
       [history](const StepArgs& a) { history(false, std::stoi(a[0])); });
  r.On("the handler saw a counter of {int}, at next sequence {int}", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE_FALSE(w.observed.empty());
    const auto& o = w.observed.back();
    REQUIRE(static_cast<int>(o.count) == std::stoi(a[0]));
    REQUIRE(static_cast<int>(o.next_seq) == std::stoi(a[1]));
  });
}

}  // namespace

TEST_CASE("counter aggregate dispatch", "[counter]") {
  for (auto& sc : ParseFeature(FeaturePath("counter.feature"))) {
    DYNAMIC_SECTION(sc.name) {
      CounterWorld world;
      StepRegistry reg;
      Register(reg, world);
      for (const auto& step : sc.steps) reg.Run(step.text);
    }
  }
}
