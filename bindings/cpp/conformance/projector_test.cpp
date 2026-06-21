#include <catch2/catch.hpp>
#include <optional>
#include <string>

#include "builders.h"
#include "gherkin.h"
#include "test/counter/counter_projector_angzarr.h"

namespace {
using namespace angzarr::conformance;
using angzarr::router::CodedError;

// The conformance CounterProjector fixture: every delivered event folds into one
// projection; the finisher carries the cover and folded count.
class ProjectorFixture : public tc::CounterProjectorHandler {
 public:
  void Increased(tc::CounterProjectorState& projection, const tc::Increased&) override {
    projection.set_count(projection.count() + 1);
  }
  pb::Projection Finish(tc::CounterProjectorState& projection,
                        const pb::EventBook& events) override {
    pb::Projection proj;
    *proj.mutable_cover() = events.cover();
    proj.set_projector("counter-projector");
    proj.set_sequence(projection.count());
    return proj;
  }
};

struct ProjectorWorld {
  angzarr::router::Router router;
  ProjectorFixture fixture;
  std::optional<pb::Projection> proj;
  std::optional<CodedError> err;

  ProjectorWorld() { tc::RegisterCounterProjector(router, fixture); }

  void Dispatch(pb::EventBook book) {
    try {
      proj = router.DispatchProjector(book);
      err.reset();
    } catch (const CodedError& e) {
      err = e;
      proj.reset();
    }
  }
};

void Register(StepRegistry& r, ProjectorWorld& w) {
  r.On("a counter projection", [](const StepArgs&) {});
  r.On("{int} events are delivered in domain {string}",
       [&w](const StepArgs& a) { w.Dispatch(DeliveryBook(a[1], std::stoi(a[0]))); });
  r.On("a delivery arrives with no cover",
       [&w](const StepArgs&) { w.Dispatch(DeliveryNoCover()); });

  r.On("the projection records {int} events", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(static_cast<int>(w.proj->sequence()) == std::stoi(a[0]));
  });
  r.On("the projection records nothing", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.proj->sequence() == 0);
  });
  r.On("the delivery fails with {word}", [&w](const StepArgs& a) {
    REQUIRE(w.err.has_value());
    REQUIRE(w.err->code == a[0]);
  });
}

}  // namespace

TEST_CASE("counter projector dispatch", "[projector]") {
  for (auto& sc : ParseFeature(FeaturePath("projector.feature"))) {
    DYNAMIC_SECTION(sc.name) {
      ProjectorWorld world;
      StepRegistry reg;
      Register(reg, world);
      for (const auto& step : sc.steps) reg.Run(step.text);
    }
  }
}
