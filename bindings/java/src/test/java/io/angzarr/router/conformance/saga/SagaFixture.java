package io.angzarr.router.conformance.saga;

import io.angzarr.router.conformance.Builders;
import io.angzarr.CommandBook;
import io.angzarr.EventBook;
import io.angzarr.Notification;
import io.angzarr.RejectionNotification;
import io.angzarr.router.Destinations;
import io.angzarr.router.Thunks.SagaEmission;
import java.util.List;
import test.counter.Counter;
import test.counter.counter_angzarr;

/** The conformance OrderSaga fixture, implementing the generated seam: a
 * declared source event emits a Reserve command stamped with the supplied
 * destination sequence; a rejection injects one fact event. */
final class SagaFixture implements counter_angzarr.OrderSagaHandler {

  @Override
  public SagaEmission increased(Counter.Increased event, Destinations dests) {
    CommandBook cmd = Builders.reserveCommand();
    if (dests.has("inventory")) {
      cmd = dests.stampCommand(cmd, "inventory");
    }
    return new SagaEmission(List.of(cmd), List.of());
  }

  @Override
  public List<EventBook> onReserveRejected(Notification n, RejectionNotification rejection) {
    return List.of(Builders.oneFact());
  }
}
