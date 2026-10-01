package io.angzarr.router.conformance.compensation;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.angzarr.BusinessResponse;
import io.angzarr.ContextualCommand;
import io.angzarr.EventPage;
import io.angzarr.router.AggregateDispatch;
import io.angzarr.router.CodedError;
import io.angzarr.router.GrpcCode;
import io.angzarr.router.Rebuilder;
import io.angzarr.router.Router;
import io.angzarr.router.conformance.Builders;
import io.cucumber.java.After;
import io.cucumber.java.Before;
import io.cucumber.java.en.Given;
import io.cucumber.java.en.Then;
import io.cucumber.java.en.When;
import java.util.List;
import test.counter.Counter;

/**
 * Step definitions for compensation.feature — rejection routing by compensates entry and Compensate
 * routing to undo handlers, over payment and inventory aggregates built through the hand-written
 * aggregate API.
 */
public class CompensationSteps {

  private static final String RESERVE = "test.counter.Reserve";

  private Router router;
  private BusinessResponse resp;
  private CodedError err;

  @Before
  public void before() {
    router = new Router();
    resp = null;
    err = null;
  }

  @After
  public void after() {
    if (router != null) {
      router.close();
    }
  }

  private void dispatch(ContextualCommand cc) {
    try {
      resp = router.dispatch(cc);
      err = null;
    } catch (CodedError e) {
      err = e;
      resp = null;
    }
  }

  /** The payment aggregate: one compensator per (compensates entry, emitted event) pair. */
  private void payment(String... entryThenEvent) {
    AggregateDispatch agg =
        new AggregateDispatch(
            "Payment", "payment", new Rebuilder(Counter.CounterState::newBuilder));
    for (int i = 0; i < entryThenEvent.length; i += 2) {
      String event = entryThenEvent[i + 1];
      agg.onRejected(entryThenEvent[i], (n, rejection, state, cctx) -> Builders.oneEvent(event));
    }
    router.registerAggregate(agg);
  }

  @Given("a payment aggregate compensating Reserve from any domain with {word}")
  public void paymentUnqualified(String event) {
    payment(RESERVE, event);
  }

  @Given("a second payment aggregate compensating Reserve from any domain with {word}")
  public void secondPayment(String event) {
    payment(RESERVE, event);
  }

  @Given(
      "a payment aggregate compensating Reserve from {string} with {word} and from {string} with"
          + " {word}")
  public void paymentQualified(
      String firstDomain, String firstEvent, String secondDomain, String secondEvent) {
    payment(firstDomain + ":" + RESERVE, firstEvent, secondDomain + ":" + RESERVE, secondEvent);
  }

  @Given(
      "an inventory aggregate undoing AdjustStock with StockAdjustmentReverted and Reserve with"
          + " StockReleased")
  public void inventory() {
    router.registerAggregate(
        new AggregateDispatch(
                "Inventory", "inventory", new Rebuilder(Counter.CounterState::newBuilder))
            .onUndo(
                "test.counter.AdjustStock",
                (n, compensate, state, cctx) -> Builders.oneEvent("StockAdjustmentReverted"))
            .onUndo(RESERVE, (n, compensate, state, cctx) -> Builders.oneEvent("StockReleased")));
  }

  @When("a rejection of {word} sent to {string} is dispatched to the payment aggregate")
  public void rejectionSentTo(String command, String domain) {
    dispatch(Builders.rejectionSentTo(command, domain, null));
  }

  @When(
      "a rejection of {word} sent to {string} is dispatched to the payment aggregate over history"
          + " ending at sequence {int}")
  public void rejectionOverHistory(String command, String domain, int last) {
    dispatch(Builders.rejectionSentTo(command, domain, last + 1));
  }

  @When("a Compensate for {word} is dispatched to the inventory aggregate")
  public void compensate(String command) {
    dispatch(Builders.compensateFor(command));
  }

  private List<EventPage> pages() {
    assertNull(err, () -> "dispatch failed: " + err);
    return resp.getEvents().getPagesList();
  }

  @Then("the aggregate emits one {word} event")
  public void emitsOne(String event) {
    List<EventPage> pages = pages();
    assertEquals(1, pages.size(), "exactly one event");
    assertTrue(pages.get(0).hasEvent(), "the page carries an event");
    assertEquals(
        "test.counter." + event, Builders.fqOf(pages.get(0).getEvent().getTypeUrl()), "event");
  }

  @Then("the aggregate emits nothing")
  public void emitsNothing() {
    assertEquals(0, pages().size(), "no events");
  }

  @Then("the emitted event takes sequence {int}")
  public void takesSequence(int seq) {
    EventPage page = pages().get(0);
    assertTrue(page.getHeader().hasSequence(), "the event carries an explicit sequence");
    assertEquals(seq, page.getHeader().getSequence(), "event sequence");
  }

  @Then("the aggregates emit {word} at sequence {int} then {word} at sequence {int}")
  public void emitInOrder(String first, int firstSeq, String second, int secondSeq) {
    List<EventPage> pages = pages();
    assertEquals(2, pages.size(), "two events");
    List<String> got =
        pages.stream()
            .map(
                p -> {
                  assertTrue(p.hasEvent(), "the page carries an event");
                  assertTrue(p.getHeader().hasSequence(), "the event carries an explicit sequence");
                  return Builders.fqOf(p.getEvent().getTypeUrl())
                      + "@"
                      + p.getHeader().getSequence();
                })
            .toList();
    assertEquals(
        List.of(
            "test.counter." + first + "@" + firstSeq, "test.counter." + second + "@" + secondSeq),
        got,
        "events in registration order with continuing sequences");
  }

  @Then("the dispatch fails with {word} as UNIMPLEMENTED")
  public void failsUnimplemented(String code) {
    assertNotNull(err, "expected coded error " + code);
    assertEquals(code, err.code, "reason");
    assertEquals(GrpcCode.UNIMPLEMENTED, err.grpc, "gRPC code");
  }
}
