package io.angzarr.router;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
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
 * The "no result" shapes of the undo and PM-compensator handlers, the fact record shapes, the
 * replay state packer registered for every aggregate, and CommandContext's cover defaults.
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
  void aFactRecordOfTheFactAloneRecordsItAsReceived() {
    List<Integer> sawCount = new ArrayList<>();
    AggregateDispatch ledger =
        new AggregateDispatch("Ledger", "ledger", counterRebuilder())
            .onFact(
                "test.counter.Increased",
                (fact, state) -> {
                  sawCount.add(((Counter.CounterState.Builder) state).getCount());
                  return FactRecord.of(fact);
                });
    try (Router router = new Router()) {
      router.registerAggregate(ledger);
      EventBook recorded = router.dispatchFact(Builders.factRequest("Increased", 2, 2));
      assertEquals(2, recorded.getPagesCount(), "recorded facts, no flags");
      for (var page : recorded.getPagesList()) {
        assertEquals(
            "test.counter.Increased",
            Builders.fqOf(page.getEvent().getTypeUrl()),
            "the fact as received");
      }
    }
    assertEquals(List.of(2, 3), sawCount, "each fact folds into the state the next fact sees");
  }

  @Test
  void aFactHandlerReplacesTheFactAndAppendsItsFlagsInOrder() throws Exception {
    AggregateDispatch ledger =
        new AggregateDispatch("Ledger", "ledger", counterRebuilder())
            .onFact(
                "test.counter.Increased",
                (fact, state) ->
                    FactRecord.of(
                        Pack.pack(Counter.CounterState.newBuilder().setCount(7).build()),
                        Pack.pack(Counter.Reserve.getDefaultInstance()),
                        Pack.pack(Counter.CounterState.newBuilder().setCount(9).build())));
    try (Router router = new Router()) {
      router.registerAggregate(ledger);
      EventBook recorded = router.dispatchFact(Builders.factRequest("Increased", 1, 0));
      List<String> types = new ArrayList<>();
      for (var page : recorded.getPagesList()) {
        types.add(Builders.fqOf(page.getEvent().getTypeUrl()));
      }
      assertEquals(
          List.of("test.counter.CounterState", "test.counter.Reserve", "test.counter.CounterState"),
          types);
      assertEquals(
          7, Counter.CounterState.parseFrom(recorded.getPages(0).getEvent().getValue()).getCount());
      assertEquals(
          9, Counter.CounterState.parseFrom(recorded.getPages(2).getEvent().getValue()).getCount());
    }
  }

  @Test
  void aFactHandlerReturningNullIsAnUnhandledError() {
    AggregateDispatch ledger =
        new AggregateDispatch("Ledger", "ledger", counterRebuilder())
            .onFact("test.counter.Increased", (fact, state) -> null);
    try (Router router = new Router()) {
      router.registerAggregate(ledger);
      CodedError e =
          assertThrows(
              CodedError.class, () -> router.dispatchFact(Builders.factRequest("Increased", 1, 0)));
      assertEquals(CodedError.UNHANDLED_HANDLER_ERROR, e.code);
      assertEquals(GrpcCode.INTERNAL, e.grpc);
    }
  }

  @Test
  void aFactOfAnUndeclaredTypeIsRefusedBeforeAnyHandlerRuns() {
    List<String> ran = new ArrayList<>();
    AggregateDispatch ledger =
        new AggregateDispatch("Ledger", "ledger", counterRebuilder())
            .onFact(
                "test.counter.Increased",
                (fact, state) -> {
                  ran.add("Increased");
                  return FactRecord.of(fact);
                });
    try (Router router = new Router()) {
      router.registerAggregate(ledger);
      CodedError e =
          assertThrows(
              CodedError.class, () -> router.dispatchFact(Builders.factRequest("Reserve", 1, 0)));
      assertEquals("NO_FACT_HANDLER", e.code);
      assertEquals(GrpcCode.INVALID_ARGUMENT, e.grpc);
    }
    assertEquals(List.of(), ran);
  }

  @Test
  void aFactRecordRequiresTheFact() {
    assertThrows(NullPointerException.class, () -> FactRecord.of(null));
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
