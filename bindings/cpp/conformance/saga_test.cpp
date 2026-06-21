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
using angzarr::router::SagaEmission;

// The conformance OrderSaga fixture: a declared source event emits a Reserve
// command stamped with the supplied destination sequence; a rejection injects
// one fact event.
class SagaFixture : public tc::OrderSagaHandler {
 public:
  SagaEmission Increased(const tc::Increased&, const Destinations& dests) override {
    auto cmd = ReserveCommand();
    if (dests.Has("inventory")) cmd = dests.StampCommand(cmd, "inventory");
    return {{cmd}, {}};
  }
  std::vector<pb::EventBook> OnReserveRejected(const pb::Notification&,
                                               const pb::RejectionNotification&) override {
    return {OneFact()};
  }
};

struct SagaWorld {
  angzarr::router::Router router;
  SagaFixture fixture;
  std::optional<pb::SagaResponse> resp;
  std::optional<CodedError> err;

  SagaWorld() { tc::RegisterOrderSaga(router, fixture); }

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
  r.On("an order saga delivering to {string}", [](const StepArgs&) {});
  r.On("an Increased event is dispatched with destination inventory sequence {int}",
       [&w](const StepArgs& a) {
         w.Dispatch(SagaEventSource("test.counter.Increased",
                                    {{"inventory", static_cast<uint32_t>(std::stoi(a[0]))}}));
       });
  r.On("a Reserve event is dispatched",
       [&w](const StepArgs&) { w.Dispatch(SagaEventSource("test.counter.Reserve", {})); });
  r.On("a source with no pages is dispatched",
       [&w](const StepArgs&) { w.Dispatch(SagaSourceNoPages()); });
  r.On("a request with no source is dispatched",
       [&w](const StepArgs&) { w.Dispatch(SagaRequestNoSource()); });
  r.On("a rejection of Reserve is dispatched",
       [&w](const StepArgs&) { w.Dispatch(SagaRejectionSource("test.counter.Reserve")); });
  r.On("a rejection of Unwatched is dispatched",
       [&w](const StepArgs&) { w.Dispatch(SagaRejectionSource("test.counter.Unwatched")); });

  r.On("the saga emits one command to {string}", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->commands_size() == 1);
    REQUIRE(w.resp->commands(0).cover().domain() == a[0]);
  });
  r.On("the command carries destination sequence {int}", [&w](const StepArgs& a) {
    REQUIRE(static_cast<int>(w.resp->commands(0).pages(0).header().sequence()) == std::stoi(a[0]));
  });
  r.On("the saga emits no commands", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->commands_size() == 0);
  });
  r.On("the dispatch fails with {word}", [&w](const StepArgs& a) {
    REQUIRE(w.err.has_value());
    REQUIRE(w.err->code == a[0]);
  });
  r.On("the saga injects one fact event", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->events_size() == 1);
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
