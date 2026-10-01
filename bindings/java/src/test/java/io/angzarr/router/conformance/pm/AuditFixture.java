package io.angzarr.router.conformance.pm;

import io.angzarr.Cover;
import io.angzarr.EventBook;
import io.angzarr.Notification;
import io.angzarr.ProcessManagerHandleResponse;
import io.angzarr.RejectionNotification;
import io.angzarr.router.Destinations;
import io.angzarr.router.Thunks.PmRejection;
import java.util.ArrayList;
import java.util.List;
import test.counter.AuditProcessManagerAngzarr;
import test.counter.Counter;

/**
 * The conformance AuditProcessManager fixture, co-resident with the order PM over its own state
 * type: the newest Increased trigger reacts with one "audit" fact per rebuilt prior-state event and
 * no commands; a rejected Reserve is compensated with one "audit" process event and no escalation.
 */
final class AuditFixture implements AuditProcessManagerAngzarr.AuditProcessManagerHandler {

  /** Cover domain stamped on every audit fact and process event. */
  static final String AUDIT_MARK = "audit";

  @Override
  public ProcessManagerHandleResponse increased(
      Counter.Increased event, Counter.AuditProcessManagerState.Builder state, Destinations dests) {
    List<EventBook> facts = new ArrayList<>();
    for (int i = 0; i < state.getSeenCount(); i++) {
      facts.add(auditBook());
    }
    return ProcessManagerHandleResponse.newBuilder().addAllFacts(facts).build();
  }

  @Override
  public void applyIncreased(
      Counter.AuditProcessManagerState.Builder state, Counter.Increased event) {
    state.addSeen("Increased");
  }

  @Override
  public PmRejection onReserveRejected(
      Notification n,
      RejectionNotification rejection,
      Counter.AuditProcessManagerState.Builder state) {
    return new PmRejection(List.of(auditBook()), null);
  }

  private static EventBook auditBook() {
    return EventBook.newBuilder().setCover(Cover.newBuilder().setDomain(AUDIT_MARK)).build();
  }
}
