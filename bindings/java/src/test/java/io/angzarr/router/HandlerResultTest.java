package io.angzarr.router;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.angzarr.BusinessResponse;
import io.angzarr.Cover;
import io.angzarr.EventBook;
import io.angzarr.ProcessManagerHandleResponse;
import io.angzarr.ReplayRequest;
import io.angzarr.router.conformance.Builders;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.Test;
import test.counter.Counter;

/**
 * The "no result" shapes of the undo, fact and PM-compensator handlers, the replay state packer
 * registered for every aggregate, and CommandContext's cover defaults.
 */
class HandlerResultTest {

  private static Rebuilder counterRebuilder() {
    return new Rebuilder(Counter.CounterState::newBuilder)
        .apply(
            "test.counter.Increased",
            (state, event) -> {
              Counter.CounterState.Builder s = (Counter.CounterState.Builder) state;
              s.setCount(s.getCount() + 1);
            });
  }

  @Test
  void anUndoHandlerReturningNullEmitsNothing() {
    List<String> undone = new ArrayList<>();
    AggregateDispatch inventory =
        new AggregateDispatch("Inventory", "inventory", counterRebuilder())
            .onUndo(
                "test.counter.Reserve",
                (n, compensate, state, cctx) -> {
                  undone.add(compensate.getCommandType() + "@" + cctx.cover().getDomain());
                  return null;
                });
    try (Router router = new Router()) {
      router.registerAggregate(inventory);
      BusinessResponse resp = router.dispatch(Builders.compensateFor("Reserve"));
      assertEquals(0, resp.getEvents().getPagesCount(), "no events");
    }
    assertEquals(List.of("test.counter.Reserve@inventory"), undone);
  }

  @Test
  void aFactHandlerReturningNullRecordsTheFactUnchanged() {
    List<Integer> sawCount = new ArrayList<>();
    AggregateDispatch ledger =
        new AggregateDispatch("Ledger", "ledger", counterRebuilder())
            .onFact(
                "test.counter.Increased",
                (fact, state) -> {
                  sawCount.add(((Counter.CounterState.Builder) state).getCount());
                  return null;
                });
    try (Router router = new Router()) {
      router.registerAggregate(ledger);
      EventBook recorded = router.dispatchFact(Builders.factRequest("Increased", 1, 2));
      assertEquals(1, recorded.getPagesCount(), "recorded facts");
      assertEquals(
          "test.counter.Increased",
          Builders.fqOf(recorded.getPages(0).getEvent().getTypeUrl()),
          "the fact is unchanged");
    }
    assertEquals(List.of(2), sawCount, "the handler saw the rebuilt state");
  }

  @Test
  void replayPacksTheRebuiltStateUnderTheBareSlashPrefix() throws Exception {
    try (Router router = new Router()) {
      router.registerAggregate(new AggregateDispatch("Ledger", "ledger", counterRebuilder()));
      ReplayRequest req =
          ReplayRequest.newBuilder()
              .addEvents(Builders.increasedAt(0))
              .addEvents(Builders.increasedAt(1))
              .addEvents(Builders.increasedAt(2))
              .build();
      var state = router.dispatchReplay("", req).getState();
      assertEquals("/test.counter.CounterState", state.getTypeUrl());
      assertEquals(3, Counter.CounterState.parseFrom(state.getValue()).getCount());
    }
  }

  @Test
  void aPmCompensatorReturningNullEmitsNothing() {
    List<String> ran = new ArrayList<>();
    ProcessManagerDispatch pm =
        new ProcessManagerDispatch(
                "Reserving",
                "reserving-pm",
                List.of("inventory"),
                new Rebuilder(Counter.CounterState::newBuilder))
            .onRejectedResponse(
                "test.counter.Reserve",
                (n, rejection, state) -> {
                  ran.add(rejection.getRejectedCommand().getCover().getDomain());
                  return null;
                });
    try (Router router = new Router()) {
      router.registerProcessManager(pm);
      ProcessManagerHandleResponse resp =
          router.dispatchProcessManager(Builders.reservingRejection("inventory", 3));
      assertEquals(0, resp.getCommandsCount(), "no commands");
      assertEquals(0, resp.getProcessEventsCount(), "no process events");
      assertFalse(resp.hasNotification(), "no escalation");
    }
    assertEquals(List.of("inventory"), ran);
  }

  @Test
  void commandContextDefaultsToAnEmptyCover() {
    assertEquals(Cover.getDefaultInstance(), new CommandContext(4, true).cover());
    assertEquals(Cover.getDefaultInstance(), new CommandContext(4, true, null).cover());
    Cover c = Cover.newBuilder().setDomain("ledger").build();
    CommandContext cctx = new CommandContext(4, true, c);
    assertEquals(c, cctx.cover());
    assertEquals(4, cctx.nextSequence());
    assertTrue(cctx.hadPriorEvents());
  }
}
