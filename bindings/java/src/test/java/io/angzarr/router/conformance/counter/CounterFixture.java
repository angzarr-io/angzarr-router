package io.angzarr.router.conformance.counter;

import io.angzarr.router.conformance.Builders;
import com.google.protobuf.Any;
import io.angzarr.BusinessResponse;
import io.angzarr.EventBook;
import io.angzarr.EventPage;
import io.angzarr.Notification;
import io.angzarr.RejectionNotification;
import io.angzarr.router.CodedError;
import io.angzarr.router.CommandContext;
import java.util.ArrayList;
import java.util.List;
import test.counter.Counter;
import test.counter.counter_angzarr;

/**
 * The conformance CounterAggregate fixture, implementing the angzarr-generated
 * handler seam. The behaviour is the contract the shared feature asserts; the
 * wiring is generated, so this fixture is the proof the generated seam is
 * faithful.
 */
final class CounterFixture implements counter_angzarr.CounterAggregateHandler {

  /** The historical-state evidence a command handler saw — what the suite
   * asserts, since state never crosses the boundary. */
  record Observation(boolean hadPriorEvents, long nextSequence, long count) {}

  private final List<Observation> observed;

  CounterFixture(List<Observation> observed) {
    this.observed = observed;
  }

  @Override
  public List<Counter.Increased> increaseBy(
      Counter.IncreaseBy cmd, Counter.CounterState.Builder state, CommandContext cctx) {
    observed.add(new Observation(cctx.hadPriorEvents(), cctx.nextSequence(), state.getCount()));
    if (cmd.getN() == 0) {
      throw CodedError.reject("VALUE_NOT_POSITIVE", "increase amount must be positive");
    }
    List<Counter.Increased> events = new ArrayList<>();
    for (int i = 0; i < cmd.getN(); i++) {
      events.add(Counter.Increased.getDefaultInstance());
    }
    return events;
  }

  @Override
  public EventBook failHard(
      Counter.FailHard cmd, Counter.CounterState.Builder state, CommandContext cctx) {
    throw new RuntimeException("hard failure");
  }

  @Override
  public void applyIncreased(Counter.CounterState.Builder state, Counter.Increased event) {
    state.setCount(state.getCount() + 1);
  }

  /** Appends both ordered markers in one response — the within-component
   * fan-out collapses to one compensator, preserving the observable two-marker
   * ordering the feature asserts. */
  @Override
  public BusinessResponse onReserveRejected(
      Notification n,
      RejectionNotification rejection,
      Counter.CounterState.Builder state,
      CommandContext cctx) {
    return BusinessResponse.newBuilder()
        .setEvents(
            EventBook.newBuilder()
                .addPages(marker("CompensatedFirst"))
                .addPages(marker("CompensatedSecond")))
        .build();
  }

  private static EventPage.Builder marker(String name) {
    return EventPage.newBuilder()
        .setEvent(Any.newBuilder().setTypeUrl(Builders.typeUrl("test.counter." + name)));
  }
}
