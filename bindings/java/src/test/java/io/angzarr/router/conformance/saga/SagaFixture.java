package io.angzarr.router.conformance.saga;

import io.angzarr.Cover;
import io.angzarr.router.Destinations;
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
  public SagaEmission increased(Counter.Increased event, Destinations dests, Cover sourceCover) {
    return new SagaEmission(List.of(Builders.reserveCommand()), List.of());
  }
}
