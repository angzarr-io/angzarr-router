package io.angzarr.router.conformance.saga;

import io.angzarr.router.CodedError;
import io.angzarr.router.Destinations;
import io.angzarr.router.PageContext;
import io.angzarr.router.SagaDispatch;
import io.angzarr.router.Thunks.SagaEmission;
import io.angzarr.router.conformance.Builders;
import java.util.List;
import test.counter.Counter;
import test.counter.OrderSagaAngzarr;

/**
 * The conformance OrderSaga fixture, implementing the generated seam: a declared source event emits
 * one Reserve command for "inventory", which the router stamps deferred.
 */
final class SagaFixture implements OrderSagaAngzarr.OrderSagaHandler {

  @Override
  public SagaEmission increased(Counter.Increased event, Destinations dests, PageContext source) {
    return new SagaEmission(List.of(Builders.reserveCommand()), List.of());
  }

  /**
   * The OrderSaga dispatch delivering to target, built through the context-taking registration: the
   * Increased handler records each triggering event's sequence in seen, then runs fixture.
   */
  static SagaDispatch recording(SagaFixture fixture, String target, List<Long> seen) {
    return new SagaDispatch("OrderSaga", "order", List.of(target))
        .onEventWithContext(
            "test.counter.Increased",
            (eventAny, dests, source) -> {
              seen.add(source.sequence());
              Counter.Increased event = CodedError.parse(Counter.Increased.parser(), eventAny);
              return fixture.increased(event, dests, source);
            });
  }
}
