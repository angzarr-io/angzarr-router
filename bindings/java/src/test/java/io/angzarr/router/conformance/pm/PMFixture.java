package io.angzarr.router.conformance.pm;

import io.angzarr.CommandBook;
import io.angzarr.Cover;
import io.angzarr.EventBook;
import io.angzarr.Notification;
import io.angzarr.ProcessManagerHandleResponse;
import io.angzarr.RejectionNotification;
import io.angzarr.router.Destinations;
import io.angzarr.router.PageContext;
import io.angzarr.router.conformance.Builders;
import java.util.ArrayList;
import java.util.List;
import test.counter.Counter;
import test.counter.OrderProcessManagerAngzarr;

/**
 * The conformance OrderProcessManager fixture: the newest trigger reacts with a Reserve command
 * (stamped deferred by the router) plus one fact per rebuilt prior-state event; a rejection injects
 * one process event and escalates, recording the rejection's code and message in {@link #seen}.
 */
final class PMFixture implements OrderProcessManagerAngzarr.OrderProcessManagerHandler {

  /** The (code, rejection_reason) of each rejection the Reserve compensator handled. */
  final List<List<String>> seen = new ArrayList<>();

  @Override
  public ProcessManagerHandleResponse increased(
      Counter.Increased event,
      Counter.OrderProcessManagerState.Builder state,
      Destinations dests,
      Cover triggerCover) {
    CommandBook cmd = Builders.reserveCommand();
    List<EventBook> facts = new ArrayList<>();
    for (int i = 0; i < state.getCount(); i++) {
      facts.add(Builders.oneFact());
    }
    return ProcessManagerHandleResponse.newBuilder().addCommands(cmd).addAllFacts(facts).build();
  }

  @Override
  public void applyIncreased(
      Counter.OrderProcessManagerState.Builder state, Counter.Increased event, PageContext ctx) {
    state.setCount(state.getCount() + 1);
  }

  @Override
  public ProcessManagerHandleResponse onReserveRejected(
      Notification n,
      RejectionNotification rejection,
      Counter.OrderProcessManagerState.Builder state) {
    seen.add(List.of(rejection.getCode(), rejection.getRejectionReason()));
    Notification escalation =
        Notification.newBuilder().setCover(Cover.newBuilder().setDomain("escalated")).build();
    return ProcessManagerHandleResponse.newBuilder()
        .addProcessEvents(Builders.oneFact())
        .setNotification(escalation)
        .build();
  }
}
