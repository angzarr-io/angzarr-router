#include <catch2/catch.hpp>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include "builders.h"
#include "gherkin.h"
#include "test/counter/audit_process_manager_angzarr.h"
#include "test/counter/order_process_manager_angzarr.h"

namespace {
using namespace angzarr::conformance;
using angzarr::router::CodedError;
using angzarr::router::Destinations;
using angzarr::router::PmRejection;

// The conformance OrderProcessManager fixture: the newest trigger reacts with a
// Reserve command (deferred: the router stamps its provenance) plus one fact
// per rebuilt prior-state event; a rejection records its code and message in
// seen, injects one process event and escalates.
class PmFixture : public tc::OrderProcessManagerHandler {
 public:
  // The (code, rejection_reason) of each rejection the Reserve compensator
  // handled.
  std::vector<std::pair<std::string, std::string>> seen;

  pb::ProcessManagerHandleResponse Increased(const tc::Increased&,
                                             tc::OrderProcessManagerState& state,
                                             const Destinations&) override {
    pb::ProcessManagerHandleResponse resp;
    *resp.add_commands() = ReserveCommand();
    for (uint32_t i = 0; i < state.count(); ++i) *resp.add_facts() = OneFact();
    return resp;
  }
  void ApplyIncreased(tc::OrderProcessManagerState& state, const tc::Increased&) override {
    state.set_count(state.count() + 1);
  }
  PmRejection OnReserveRejected(const pb::Notification&, const pb::RejectionNotification& rejection,
                                tc::OrderProcessManagerState&) override {
    seen.emplace_back(rejection.code(), rejection.rejection_reason());
    pb::Notification escalation;
    escalation.mutable_cover()->set_domain("escalated");
    return {{OneFact()}, escalation};
  }
};

// The cover domain marking every book the AuditProcessManager emits, so its
// output is distinguishable from the order PM's within one merged response.
constexpr char kAuditMark[] = "audit";

pb::EventBook AuditBook() {
  pb::EventBook book;
  book.mutable_cover()->set_domain(kAuditMark);
  return book;
}

// The conformance AuditProcessManager fixture, co-resident with the order PM: it
// records each rebuilt Increased, reacts to a trigger with one audit-marked fact
// per recorded event (no commands), and compensates with one audit-marked
// process event and no escalation.
class AuditFixture : public tc::AuditProcessManagerHandler {
 public:
  pb::ProcessManagerHandleResponse Increased(const tc::Increased&,
                                             tc::AuditProcessManagerState& state,
                                             const Destinations&) override {
    pb::ProcessManagerHandleResponse resp;
    for (int i = 0; i < state.seen_size(); ++i) *resp.add_facts() = AuditBook();
    return resp;
  }
  void ApplyIncreased(tc::AuditProcessManagerState& state, const tc::Increased&) override {
    state.add_seen("Increased");
  }
  PmRejection OnReserveRejected(const pb::Notification&, const pb::RejectionNotification&,
                                tc::AuditProcessManagerState&) override {
    return {{AuditBook()}, std::nullopt};
  }
};

struct PmWorld {
  angzarr::router::Router router;
  PmFixture fixture;
  AuditFixture audit;
  std::optional<pb::ProcessManagerHandleResponse> resp;
  std::optional<CodedError> err;

  int FactsMarked(bool audit_mark) const {
    int n = 0;
    for (const auto& f : resp->facts()) {
      if ((f.cover().domain() == kAuditMark) == audit_mark) ++n;
    }
    return n;
  }

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
  r.On("an order process-manager",
       [&w](const StepArgs&) { tc::RegisterOrderProcessManager(w.router, w.fixture); });
  r.On("co-resident order and audit process-managers", [&w](const StepArgs&) {
    tc::RegisterOrderProcessManager(w.router, w.fixture);
    tc::RegisterAuditProcessManager(w.router, w.audit);
  });
  r.On("an Increased trigger in domain {string} at sequence {int} is dispatched",
       [&w](const StepArgs& a) {
         w.Dispatch(PmTrigger(a[0], {"test.counter.Increased"}, std::nullopt,
                              static_cast<uint32_t>(std::stoi(a[1]))));
       });
  r.On("an Increased trigger in domain {string} is dispatched", [&w](const StepArgs& a) {
    w.Dispatch(PmTrigger(a[0], {"test.counter.Increased"}, std::nullopt));
  });
  r.On("a trigger whose newest page is an undeclared event is dispatched", [&w](const StepArgs&) {
    w.Dispatch(
        PmTrigger("counter", {"test.counter.Increased", "test.counter.Unwatched"}, std::nullopt));
  });
  r.On("an Increased trigger is dispatched over a prior state of {int} events",
       [&w](const StepArgs& a) {
         w.Dispatch(PmTrigger("counter", {"test.counter.Increased"}, PmStateOf(std::stoi(a[0]))));
       });
  r.On("an Increased trigger is dispatched over a prior {string} state of {int} events",
       [&w](const StepArgs& a) {
         auto state = PmStateOf(std::stoi(a[1]));
         state.mutable_cover()->set_domain(a[0]);
         w.Dispatch(PmTrigger("counter", {"test.counter.Increased"}, state));
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

  r.On(
      "a rejection of Reserve with code {string} and message {string} is dispatched",
      [&w](const StepArgs& a) { w.Dispatch(PmRejectionWith("test.counter.Reserve", a[0], a[1])); });
  r.On("a Compensate for Reserve is dispatched to the order process-manager",
       [&w](const StepArgs&) { w.Dispatch(PmCompensateRequest("Reserve")); });
  r.On("a rejection of Reserve issued by {string} is dispatched",
       [&w](const StepArgs& a) { w.Dispatch(PmIssuedRejection("test.counter.Reserve", a[0])); });

  r.On("the process-manager emits one command to {string}", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->commands_size() == 1);
    REQUIRE(w.resp->commands(0).cover().domain() == a[0]);
  });
  r.On("the command is deferred from source sequence {int} at command index {int}",
       [&w](const StepArgs& a) { RequireDeferred(w.resp->commands(0), "counter", a); });
  r.On("the command leaves its source component to the coordinator", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    RequireNoSourceComponent(w.resp->commands(0));
  });
  r.On("the process-manager emits no commands", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->commands_size() == 0);
  });
  r.On("the process-manager rebuilt {int} prior state events", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->facts_size() == std::stoi(a[0]));
  });
  r.On("the order process-manager rebuilt {int} prior state events", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.FactsMarked(false) == std::stoi(a[0]));
  });
  r.On("the audit process-manager rebuilt {int} prior state events", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.FactsMarked(true) == std::stoi(a[0]));
  });
  r.On("the order process-manager did not react", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->commands_size() == 0);
    REQUIRE(w.FactsMarked(false) == 0);
  });
  r.On("only the audit process-manager compensates", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->process_events_size() == 1);
    REQUIRE(w.resp->process_events(0).cover().domain() == kAuditMark);
    REQUIRE_FALSE(w.resp->has_notification());
  });
  r.On("the process-manager compensator saw code {string} and message {string}",
       [&w](const StepArgs& a) {
         REQUIRE_FALSE(w.err.has_value());
         const std::vector<std::pair<std::string, std::string>> want{{a[0], a[1]}};
         REQUIRE(w.fixture.seen == want);
       });
  r.On("the dispatch fails with {word}", [&w](const StepArgs& a) {
    REQUIRE(w.err.has_value());
    REQUIRE(w.err->code == a[0]);
  });
  r.On("the process-manager emits one process event", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->process_events_size() == 1);
  });
  r.On("the process event is addressed to {string}", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->process_events_size() == 1);
    REQUIRE(w.resp->process_events(0).cover().domain() == a[0]);
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
