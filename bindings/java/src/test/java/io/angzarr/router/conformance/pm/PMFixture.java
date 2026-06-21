package io.angzarr.router.conformance.pm;

import io.angzarr.CommandBook;
import io.angzarr.Cover;
import io.angzarr.EventBook;
import io.angzarr.Notification;
import io.angzarr.ProcessManagerHandleResponse;
import io.angzarr.RejectionNotification;
import io.angzarr.router.Destinations;
import io.angzarr.router.Thunks.PmRejection;
import io.angzarr.router.conformance.Builders;
import java.util.ArrayList;
import java.util.List;
import test.counter.Counter;
import test.counter.OrderProcessManagerAngzarr;

/** The conformance OrderProcessManager fixture: the newest trigger reacts with a
 * stamped Reserve command plus one fact per rebuilt prior-state event; a
 * rejection injects one process event and escalates. */
final class PMFixture implements OrderProcessManagerAngzarr.OrderProcessManagerHandler {

  @Override
  public ProcessManagerHandleResponse increased(
      Counter.Increased event, Counter.OrderProcessManagerState.Builder state, Destinations dests) {
    CommandBook cmd = Builders.reserveCommand();
    if (dests.has("inventory")) {
      cmd = dests.stampCommand(cmd, "inventory");
    }
    List<EventBook> facts = new ArrayList<>();
    for (int i = 0; i < state.getCount(); i++) {
      facts.add(Builders.oneFact());
    }
    return ProcessManagerHandleResponse.newBuilder().addCommands(cmd).addAllFacts(facts).build();
  }

  @Override
  public void applyIncreased(Counter.OrderProcessManagerState.Builder state, Counter.Increased event) {
    state.setCount(state.getCount() + 1);
  }

  @Override
  public PmRejection onReserveRejected(
      Notification n, RejectionNotification rejection, Counter.OrderProcessManagerState.Builder state) {
    Notification escalation = Notification.newBuilder().setCover(Cover.newBuilder().setDomain("escalated")).build();
    return new PmRejection(List.of(Builders.oneFact()), escalation);
  }
}
