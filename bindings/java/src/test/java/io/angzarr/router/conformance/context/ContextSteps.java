package io.angzarr.router.conformance.context;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.google.protobuf.Any;
import com.google.protobuf.ByteString;
import io.angzarr.BusinessResponse;
import io.angzarr.CommandPage;
import io.angzarr.Cover;
import io.angzarr.EventBook;
import io.angzarr.EventPage;
import io.angzarr.ProcessManagerHandleRequest;
import io.angzarr.ProcessManagerHandleResponse;
import io.angzarr.router.AggregateDispatch;
import io.angzarr.router.CodedError;
import io.angzarr.router.FactRecord;
import io.angzarr.router.GrpcCode;
import io.angzarr.router.Pack;
import io.angzarr.router.ProcessManagerDispatch;
import io.angzarr.router.ProjectorDispatch;
import io.angzarr.router.Rebuilder;
import io.angzarr.router.Router;
import io.angzarr.router.conformance.Builders;
import io.cucumber.java.After;
import io.cucumber.java.Before;
import io.cucumber.java.en.Given;
import io.cucumber.java.en.Then;
import io.cucumber.java.en.When;
import java.util.ArrayList;
import java.util.List;
import test.counter.Counter;

/**
 * Step definitions for context.feature — facts, replay, cover access, PM compensator commands and
 * projector page context, over the ledger aggregate, reserving process-manager and tracking
 * projector built through the hand-written APIs.
 */
public class ContextSteps {

  private static final String INCREASED = "test.counter.Increased";

  /** A (root, sequence) pair a projector fold observed. */
  record Seen(ByteString root, long sequence) {}

  private Router router;
  private final List<Cover> covers = new ArrayList<>();
  private final List<Seen> pages = new ArrayList<>();
  private final List<Long> applied = new ArrayList<>();
  private EventBook facts;
  private Counter.CounterState replayed;
  private ProcessManagerHandleResponse pmResp;
  private BusinessResponse command;
  private CodedError err;

  @Before
  public void before() {
    router = new Router();
    covers.clear();
    pages.clear();
    applied.clear();
    facts = null;
    replayed = null;
    pmResp = null;
    command = null;
    err = null;
  }

  @After
  public void after() {
    if (router != null) {
      router.close();
    }
  }

  // --- components ------------------------------------------------------------

  @Given("a ledger aggregate")
  public void ledger() {
    Rebuilder rebuilder =
        new Rebuilder(Counter.CounterState::newBuilder)
            .applyWithContext(
                INCREASED,
                (state, event, ctx) -> {
                  Counter.CounterState.Builder s = (Counter.CounterState.Builder) state;
                  s.setCount(s.getCount() + 1);
                  applied.add(ctx.sequence());
                })
            .withSnapshot(
                (state, snapshot) ->
                    ((Counter.CounterState.Builder) state).clear().mergeFrom(snapshot.getValue()));
    router.registerAggregate(
        new AggregateDispatch("Ledger", "ledger", rebuilder)
            .onCommand(
                "test.counter.IncreaseBy",
                (cmd, state, cctx) -> {
                  covers.add(cctx.cover());
                  return EventBook.newBuilder()
                      .setCover(Cover.newBuilder().setExt(Builders.ledgerLinkage()))
                      .addPages(
                          EventPage.newBuilder()
                              .setEvent(Pack.pack(Counter.Increased.getDefaultInstance())))
                      .build();
                })
            .onFact(
                INCREASED,
                (fact, state) ->
                    FactRecord.of(
                        fact,
                        Pack.pack(
                            Counter.CounterState.newBuilder()
                                .setCount(((Counter.CounterState.Builder) state).getCount() + 1)
                                .build()))));
  }

  @Given("a reserving process-manager")
  public void reserving() {
    router.registerProcessManager(
        new ProcessManagerDispatch(
                "Reserving",
                "reserving-pm",
                List.of("inventory"),
                new Rebuilder(Counter.CounterState::newBuilder)
                    .apply(
                        INCREASED,
                        (state, event) -> {
                          Counter.CounterState.Builder s = (Counter.CounterState.Builder) state;
                          s.setCount(s.getCount() + 1);
                        }))
            .onEvent(
                "counter",
                INCREASED,
                (event, state, dests, triggerCover) -> {
                  covers.add(triggerCover);
                  return ProcessManagerHandleResponse.getDefaultInstance();
                })
            .onRejectedResponse(
                "test.counter.Reserve",
                (n, rejection, state) -> {
                  var release = Builders.reserveCommand().toBuilder();
                  release
                      .getPagesBuilder(0)
                      .setCommand(Any.newBuilder().setTypeUrl("/test.counter.Release"));
                  return ProcessManagerHandleResponse.newBuilder().addCommands(release).build();
                }));
  }

  @Given("a tracking projector")
  public void tracking() {
    router.registerProjector(
        new ProjectorDispatch("Tracker", Counter.CounterState::newBuilder)
            .onEvent(
                INCREASED,
                (projection, event, ctx) ->
                    pages.add(new Seen(ctx.cover().getRoot().getValue(), ctx.sequence()))));
  }

  // --- dispatch --------------------------------------------------------------

  @When("{int} Increased facts are handled over {int} prior Increased events")
  public void increasedFacts(int n, int prior) {
    dispatchFacts("Increased", n, prior);
  }

  @When("a Reserve fact is handled over no prior events")
  public void reserveFact() {
    dispatchFacts("Reserve", 1, 0);
  }

  private void dispatchFacts(String fact, int n, int prior) {
    try {
      facts = router.dispatchFact(Builders.factRequest(fact, n, prior));
      err = null;
    } catch (CodedError e) {
      err = e;
      facts = null;
    }
  }

  @When("the ledger replays a snapshot of {int} then {int} Increased events")
  public void replay(int count, int events) throws Exception {
    Any state = router.dispatchReplay("ledger", Builders.replayRequest(count, events)).getState();
    assertEquals("test.counter.CounterState", Builders.fqOf(state.getTypeUrl()), "state type");
    replayed = Counter.CounterState.parseFrom(state.getValue());
  }

  @When("the reserving process-manager replays {int} Increased events")
  public void pmReplay(int events) throws Exception {
    Any state =
        router.dispatchReplay("reserving-pm", Builders.eventsReplayRequest(events)).getState();
    assertEquals("test.counter.CounterState", Builders.fqOf(state.getTypeUrl()), "state type");
    replayed = Counter.CounterState.parseFrom(state.getValue());
  }

  @When("an IncreaseBy command for ledger root {string} is dispatched")
  public void ledgerCommand(String label) {
    command = router.dispatch(Builders.ledgerCommand(label));
  }

  @When("an IncreaseBy command for ledger root {string} on behalf of a parent is dispatched")
  public void ledgerCommandWithParent(String label) {
    command = router.dispatch(Builders.ledgerCommandWithLinkage(label));
  }

  @When(
      "an Increased trigger of counter root {string} at sequence {int} is dispatched to the"
          + " reserving process-manager")
  public void reservingTrigger(String label, int seq) {
    dispatchPm(Builders.reservingTrigger(label, seq));
  }

  @When(
      "a rejection of Reserve sent to {string} at sequence {int} is dispatched to the reserving"
          + " process-manager")
  public void reservingRejection(String domain, int seq) {
    dispatchPm(Builders.reservingRejection(domain, seq));
  }

  private void dispatchPm(ProcessManagerHandleRequest req) {
    try {
      pmResp = router.dispatchProcessManager(req);
      err = null;
    } catch (CodedError e) {
      err = e;
      pmResp = null;
    }
  }

  @When("Increased events of counter root {string} at sequences {int} and {int} are projected")
  public void projected(String label, int first, int second) {
    router.dispatchProjector(Builders.trackedBook(label, first, second));
  }

  // --- assertions ------------------------------------------------------------

  /** A recorded event: its fully-qualified type and, for a CounterState, its count. */
  record Recorded(String type, Integer count) {}

  @Then("each Increased fact is recorded, flagged by the counts {int} and {int}")
  public void recordedAndFlagged(int first, int second) throws Exception {
    assertNull(err, () -> "fact dispatch failed: " + err);
    assertNotNull(facts, "facts were handled");
    List<Recorded> recorded = new ArrayList<>();
    for (EventPage page : facts.getPagesList()) {
      String type = Builders.fqOf(page.getEvent().getTypeUrl());
      Integer count =
          type.equals("test.counter.CounterState")
              ? Counter.CounterState.parseFrom(page.getEvent().getValue()).getCount()
              : null;
      recorded.add(new Recorded(type, count));
    }
    assertEquals(
        List.of(
            new Recorded(INCREASED, null),
            new Recorded("test.counter.CounterState", first),
            new Recorded(INCREASED, null),
            new Recorded("test.counter.CounterState", second)),
        recorded);
  }

  @Then("the facts are refused with {word} as INVALID_ARGUMENT")
  public void factsRefused(String code) {
    assertNull(facts, "nothing is recorded");
    assertNotNull(err, "the facts are refused");
    assertEquals(code, err.code, "code");
    assertEquals(GrpcCode.INVALID_ARGUMENT, err.grpc, "grpc");
  }

  @Then("the replayed state has a count of {int}")
  public void replayedCount(int count) {
    assertNotNull(replayed, "replayed");
    assertEquals(count, replayed.getCount(), "replayed count");
  }

  @Then("the ledger applied Increased events at sequences {int} and {int}")
  public void appliedAt(int first, int second) {
    assertEquals(List.of((long) first, (long) second), applied, "applied sequences");
  }

  private ByteString singleRoot() {
    assertEquals(1, covers.size(), "the handler ran once");
    assertTrue(covers.get(0).hasRoot(), "the handler saw a cover with a root");
    return covers.get(0).getRoot().getValue();
  }

  @Then("the recorded event carries the ledger's own linkage")
  public void ledgerOwnLinkage() {
    assertNotNull(command, "a command was dispatched");
    assertTrue(command.hasEvents(), "expected events");
    assertEquals(1, command.getEvents().getPagesCount(), "recorded events");
    assertEquals(
        Builders.ledgerLinkage(),
        command.getEvents().getCover().getExt(),
        "the handler-set linkage is kept over the command's");
  }

  @Then("the ledger handler saw root {string}")
  public void ledgerSaw(String label) {
    assertEquals(ByteString.copyFrom(Builders.rootOf(label)), singleRoot());
  }

  @Then("the reserving process-manager saw trigger root {string}")
  public void pmSaw(String label) {
    assertEquals(ByteString.copyFrom(Builders.rootOf(label)), singleRoot());
  }

  @Then(
      "the reserving process-manager emits one Release command deferred from source sequence"
          + " {int}")
  public void release(int seq) {
    assertNull(err, () -> "PM dispatch failed: " + err);
    assertEquals(1, pmResp.getCommandsCount(), "commands");
    CommandPage page = pmResp.getCommands(0).getPages(0);
    assertEquals("test.counter.Release", Builders.fqOf(page.getCommand().getTypeUrl()), "type");
    assertTrue(page.getHeader().hasAngzarrDeferred(), "the Release command is deferred");
    assertEquals(seq, page.getHeader().getAngzarrDeferred().getSourceSeq(), "source_seq");
  }

  @Then("the projector saw root {string} at sequences {int} and {int}")
  public void projectorSaw(String label, int first, int second) {
    ByteString root = ByteString.copyFrom(Builders.rootOf(label));
    assertEquals(List.of(new Seen(root, first), new Seen(root, second)), pages);
  }
}
