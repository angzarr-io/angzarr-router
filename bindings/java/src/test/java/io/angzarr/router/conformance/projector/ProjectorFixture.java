package io.angzarr.router.conformance.projector;

import io.angzarr.EventBook;
import io.angzarr.Projection;
import test.counter.Counter;
import test.counter.CounterProjectorAngzarr;

/** The conformance CounterProjector fixture: every delivered event folds into
 * one projection; the finisher carries the cover and the folded count. */
final class ProjectorFixture implements CounterProjectorAngzarr.CounterProjectorHandler {

  @Override
  public void increased(Counter.CounterProjectorState.Builder projection, Counter.Increased event) {
    projection.setCount(projection.getCount() + 1);
  }

  @Override
  public Projection finish(Counter.CounterProjectorState.Builder projection, EventBook events) {
    return Projection.newBuilder()
        .setCover(events.getCover())
        .setProjector("counter-projector")
        .setSequence(projection.getCount())
        .build();
  }
}
