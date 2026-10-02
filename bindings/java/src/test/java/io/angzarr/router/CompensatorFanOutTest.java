package io.angzarr.router;

import static org.junit.jupiter.api.Assertions.assertEquals;

import com.google.protobuf.Any;
import io.angzarr.BusinessResponse;
import io.angzarr.EventBook;
import io.angzarr.EventPage;
import io.angzarr.router.conformance.Builders;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.Test;
import test.counter.Counter;

/**
 * Ordered compensator fan-out across the FFI: two compensators registered for one rejected command
 * type on one aggregate both run, in registration order, and their events merge in that order.
 */
class CompensatorFanOutTest {

  private static final String RESERVE = "test.counter.Reserve";

  private static Thunks.RejectionThunk compensator(List<String> ran, String marker) {
    return (n, rejection, state, cctx) -> {
      ran.add(marker);
      return BusinessResponse.newBuilder()
          .setEvents(
              EventBook.newBuilder()
                  .addPages(
                      EventPage.newBuilder()
                          .setEvent(Any.newBuilder().setTypeUrl(Builders.typeUrl(marker)))))
          .build();
    };
  }

  @Test
  void twoCompensatorsRunInRegistrationOrderAndMergeTheirEvents() {
    List<String> ran = new ArrayList<>();
    AggregateDispatch d =
        new AggregateDispatch(
                "FanOutAggregate", "counter", new Rebuilder(Counter.CounterState::newBuilder))
            .onRejected(RESERVE, compensator(ran, "test.counter.CompensatedFirst"))
            .onRejected(RESERVE, compensator(ran, "test.counter.CompensatedSecond"));

    try (Router router = new Router()) {
      router.registerAggregate(d);
      BusinessResponse resp = router.dispatch(Builders.rejectionCommand(RESERVE));

      assertEquals(
          List.of("test.counter.CompensatedFirst", "test.counter.CompensatedSecond"),
          ran,
          "both compensators ran in registration order");
      List<String> merged = new ArrayList<>();
      for (EventPage page : resp.getEvents().getPagesList()) {
        merged.add(page.getEvent().getTypeUrl());
      }
      assertEquals(
          List.of("/test.counter.CompensatedFirst", "/test.counter.CompensatedSecond"),
          merged,
          "compensation events merged in registration order");
    }
  }
}
