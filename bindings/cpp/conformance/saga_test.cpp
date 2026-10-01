#include <catch2/catch.hpp>
#include <optional>
#include <string>
#include <vector>

#include "builders.h"
#include "gherkin.h"
#include "test/counter/order_saga_angzarr.h"

namespace {
using namespace angzarr::conformance;
using angzarr::router::CodedError;
using angzarr::router::Destinations;
using angzarr::router::PageContext;
using angzarr::router::SagaDispatch;
using angzarr::router::SagaEmission;

// The conformance OrderSaga fixture: a declared source event emits one Reserve
// command for "inventory" (deferred: the router stamps its provenance).
class SagaFixture : public tc::OrderSagaHandler {
 public:
  SagaEmission Increased(const tc::Increased&, const Destinations&, const pb::Cover&) override {
    return {{ReserveCommand()}, {}};
  }
};

// The OrderSaga Increased handler through the context-taking registration: it
// records each triggering event's sequence in seen, then runs fixture.
SagaDispatch::EventWithContextFn RecordingIncreased(SagaFixture& fixture,
                                                    std::vector<uint32_t>& seen) {
  return [&fixture, &seen](const google::protobuf::Any& event_any, const Destinations& dests,
                           const PageContext& source) {
    seen.push_back(source.sequence);
    return fixture.Increased(CodedError::Parse<tc::Increased>(event_any), dests, source.cover);
  };
}

// The OrderSaga dispatch delivering to target, recording into seen.
SagaDispatch RecordingSaga(SagaFixture& fixture, const std::string& target,
                           std::vector<uint32_t>& seen) {
  SagaDispatch saga("OrderSaga", "order", {target});
  saga.OnEventWithContext("test.counter.Increased", RecordingIncreased(fixture, seen));
  return saga;
}

struct SagaWorld {
  angzarr::router::Router router;
  SagaFixture fixture;
  std::vector<uint32_t> seen;
  std::optional<pb::SagaResponse> resp;
  std::optional<CodedError> err;

  void Dispatch(pb::SagaHandleRequest req) {
    try {
      resp = router.DispatchSaga(req);
      err.reset();
    } catch (const CodedError& e) {
      err = e;
      resp.reset();
    }
  }
};

void Register(StepRegistry& r, SagaWorld& w) {
  r.On("an order saga delivering to {string}",
       [&w](const StepArgs& a) { w.router.RegisterSaga(RecordingSaga(w.fixture, a[0], w.seen)); });
  r.On("an Increased event at sequence {int} is dispatched", [&w](const StepArgs& a) {
    w.Dispatch(SagaEventSource("test.counter.Increased", static_cast<uint32_t>(std::stoi(a[0]))));
  });
  r.On("a Reserve event is dispatched",
       [&w](const StepArgs&) { w.Dispatch(SagaEventSource("test.counter.Reserve")); });
  r.On("a source with no pages is dispatched",
       [&w](const StepArgs&) { w.Dispatch(SagaSourceNoPages()); });
  r.On("a request with no source is dispatched",
       [&w](const StepArgs&) { w.Dispatch(SagaRequestNoSource()); });
  r.On("a rejection of Reserve is dispatched",
       [&w](const StepArgs&) { w.Dispatch(SagaRejectionSource("test.counter.Reserve")); });

  r.On("the saga emits one command to {string}", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->commands_size() == 1);
    REQUIRE(w.resp->commands(0).cover().domain() == a[0]);
  });
  r.On("the command is deferred from source sequence {int} at command index {int}",
       [&w](const StepArgs& a) { RequireDeferred(w.resp->commands(0), "order", a); });
  r.On("the saga handler saw source sequence {int}", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.seen == std::vector<uint32_t>{static_cast<uint32_t>(std::stoi(a[0]))});
  });
  r.On("the saga emits no commands", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->commands_size() == 0);
  });
  r.On("the dispatch fails with {word}", [&w](const StepArgs& a) {
    REQUIRE(w.err.has_value());
    REQUIRE(w.err->code == a[0]);
  });
  r.On("the saga injects no events", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->events_size() == 0);
  });
}

}  // namespace

TEST_CASE("order saga dispatch", "[saga]") {
  for (auto& sc : ParseFeature(FeaturePath("saga.feature"))) {
    DYNAMIC_SECTION(sc.name) {
      SagaWorld world;
      StepRegistry reg;
      Register(reg, world);
      for (const auto& step : sc.steps) reg.Run(step.text);
    }
  }
}
