#include <catch2/catch.hpp>
#include <optional>
#include <string>

#include "builders.h"
#include "gherkin.h"
#include "test/counter/counter_angzarr.h"

namespace {
using namespace angzarr::conformance;
using angzarr::router::CodedError;
using angzarr::router::Destinations;
using angzarr::router::PmRejection;

// The conformance OrderProcessManager fixture: the newest trigger reacts with a
// stamped Reserve command plus one fact per rebuilt prior-state event; a
// rejection injects one process event and escalates.
class PmFixture : public tc::OrderProcessManagerHandler {
 public:
  pb::ProcessManagerHandleResponse Increased(const tc::Increased&,
                                             tc::OrderProcessManagerState& state,
                                             const Destinations& dests) override {
    auto cmd = ReserveCommand();
    if (dests.Has("inventory")) cmd = dests.StampCommand(cmd, "inventory");
    pb::ProcessManagerHandleResponse resp;
    *resp.add_commands() = cmd;
    for (uint32_t i = 0; i < state.count(); ++i) *resp.add_facts() = OneFact();
    return resp;
  }
  void ApplyIncreased(tc::OrderProcessManagerState& state, const tc::Increased&) override {
    state.set_count(state.count() + 1);
  }
  PmRejection OnReserveRejected(const pb::Notification&, const pb::RejectionNotification&,
                                tc::OrderProcessManagerState&) override {
    pb::Notification escalation;
    escalation.mutable_cover()->set_domain("escalated");
    return {{OneFact()}, escalation};
  }
};

struct PmWorld {
  angzarr::router::Router router;
  PmFixture fixture;
  std::optional<pb::ProcessManagerHandleResponse> resp;
  std::optional<CodedError> err;

  PmWorld() { tc::RegisterOrderProcessManager(router, fixture); }

  void Dispatch(pb::ProcessManagerHandleRequest req) {
    try {
      resp = router.DispatchProcessManager(req);
      err.reset();
    } catch (const CodedError& e) {
      err = e;
      resp.reset();
    }
  }
};

void Register(StepRegistry& r, PmWorld& w) {
  r.On("an order process-manager", [](const StepArgs&) {});
  r.On(
      "an Increased trigger in domain {string} is dispatched with destination inventory sequence "
      "{int}",
      [&w](const StepArgs& a) {
        w.Dispatch(PmTrigger(a[0], {"test.counter.Increased"}, std::nullopt,
                             {{"inventory", static_cast<uint32_t>(std::stoi(a[1]))}}));
      });
  r.On("an Increased trigger in domain {string} is dispatched", [&w](const StepArgs& a) {
    w.Dispatch(PmTrigger(a[0], {"test.counter.Increased"}, std::nullopt, {}));
  });
  r.On("a trigger whose newest page is an undeclared event is dispatched", [&w](const StepArgs&) {
    w.Dispatch(PmTrigger("counter", {"test.counter.Increased", "test.counter.Unwatched"},
                         std::nullopt, {}));
  });
  r.On("an Increased trigger is dispatched over a prior state of {int} events",
       [&w](const StepArgs& a) {
         w.Dispatch(
             PmTrigger("counter", {"test.counter.Increased"}, PmStateOf(std::stoi(a[0])), {}));
       });
  r.On("a request with no trigger is dispatched",
       [&w](const StepArgs&) { w.Dispatch(PmNoTrigger()); });
  r.On("a trigger with no pages is dispatched",
       [&w](const StepArgs&) { w.Dispatch(PmEmptyTrigger()); });
  r.On("a rejection of Reserve is dispatched", [&w](const StepArgs&) {
    // Qualify: the unqualified PmRejection resolves to the router struct (used as
    // the handler return type) via the using-declaration, not the builder.
    w.Dispatch(angzarr::conformance::PmRejection("test.counter.Reserve"));
  });

  r.On("the process-manager emits one command to {string}", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->commands_size() == 1);
    REQUIRE(w.resp->commands(0).cover().domain() == a[0]);
  });
  r.On("the command carries destination sequence {int}", [&w](const StepArgs& a) {
    REQUIRE(static_cast<int>(w.resp->commands(0).pages(0).header().sequence()) == std::stoi(a[0]));
  });
  r.On("the process-manager emits no commands", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->commands_size() == 0);
  });
  r.On("the process-manager rebuilt {int} prior state events", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->facts_size() == std::stoi(a[0]));
  });
  r.On("the dispatch fails with {word}", [&w](const StepArgs& a) {
    REQUIRE(w.err.has_value());
    REQUIRE(w.err->code == a[0]);
  });
  r.On("the process-manager emits one process event", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->process_events_size() == 1);
  });
  r.On("the process-manager escalates", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->has_notification());
  });
}

}  // namespace

TEST_CASE("order process-manager dispatch", "[pm]") {
  for (auto& sc : ParseFeature(FeaturePath("process_manager.feature"))) {
    DYNAMIC_SECTION(sc.name) {
      PmWorld world;
      StepRegistry reg;
      Register(reg, world);
      for (const auto& step : sc.steps) reg.Run(step.text);
    }
  }
}
