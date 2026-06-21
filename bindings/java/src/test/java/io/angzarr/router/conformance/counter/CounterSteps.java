package io.angzarr.router.conformance.counter;

import io.angzarr.router.conformance.Builders;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;

import com.google.protobuf.Any;
import io.angzarr.ContextualCommand;
import io.angzarr.EventBook;
import io.angzarr.router.conformance.counter.CounterFixture.Observation;
import io.angzarr.router.CodedError;
import io.angzarr.router.Router;
import io.cucumber.java.After;
import io.cucumber.java.Before;
import io.cucumber.java.en.Given;
import io.cucumber.java.en.Then;
import io.cucumber.java.en.When;
import java.util.ArrayList;
import java.util.List;
import test.counter.counter_angzarr;

/**
 * Step definitions for counter.feature — the shared cross-language behavior
 * suite, run against the Java binding via Cucumber-JVM. Only this step layer is
 * new; the features and fixtures are the same the Rust harness runs.
 */
public class CounterSteps {

  private Router router;
  private EventBook prior;
  private io.angzarr.BusinessResponse resp;
  private CodedError err;
  private final List<Observation> observed = new ArrayList<>();

  @Before
  public void before() {
    observed.clear();
    router = new Router();
    counter_angzarr.registerCounterAggregate(router, new CounterFixture(observed));
    prior = null;
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
    if (prior != null) {
      cc = cc.toBuilder().setEvents(prior).build();
    }
    try {
      resp = router.dispatch(cc);
      err = null;
    } catch (CodedError e) {
      err = e;
      resp = null;
    }
  }

  // --- Given: prior history --------------------------------------------------

  @Given("a new counter")
  public void aNewCounter() {
    prior = null;
  }

  @Given("a counter that has already recorded {int} increase(s)")
  public void recordedIncreases(int n) {
    prior = Builders.priorIncreases(n);
  }

  @Given("a counter whose history holds a corrupt event")
  public void historyHoldsCorrupt() {
    prior = Builders.corruptHistory();
  }

  @Given("a counter restored from a snapshot of 10 with one newer event")
  public void restoredFromSnapshot() {
    prior = Builders.snapshotHistory();
  }

  // --- When: dispatch --------------------------------------------------------

  @When("the operator increases the counter by {int}")
  public void increaseBy(int n) {
    dispatch(Builders.increaseCommand(n));
  }

  @When("the operator increases the counter by {int} on behalf of a parent")
  public void increaseOnBehalf(int n) {
    dispatch(Builders.increaseCommandWithLinkage(n));
  }

  @When("the operator triggers a hard failure")
  public void triggerHardFailure() {
    dispatch(Builders.failHardCommand());
  }

  @When("an unhandled command is dispatched")
  public void unhandledDispatched() {
    dispatch(Builders.unhandledCommand());
  }

  @When("a command with no command book is dispatched")
  public void commandNoBook() {
    dispatch(Builders.commandMissingBook());
  }

  @When("a command with an empty command book is dispatched")
  public void commandEmptyBook() {
    dispatch(Builders.commandMissingPage());
  }

  @When("a command whose page carries no payload is dispatched")
  public void commandNoPayload() {
    dispatch(Builders.commandMissingPayload());
  }

  @When("a Reserve command is rejected")
  public void reserveRejected() {
    dispatch(Builders.rejectionCommand("test.counter.Reserve"));
  }

  @When("an unregistered command is rejected")
  public void unregisteredRejected() {
    dispatch(Builders.rejectionCommand("test.counter.Undeclared"));
  }

  // --- Then: assertions ------------------------------------------------------

  @Then("{int} increases are recorded, starting at sequence {int}")
  public void recordedStartingAt(int count, int start) {
    recorded(count, start);
  }

  @Then("{int} increases are recorded, continuing from sequence {int}")
  public void recordedContinuingFrom(int count, int start) {
    recorded(count, start);
  }

  private void recorded(int count, int start) {
    assertNull(err, () -> "dispatch failed: " + (err == null ? "" : err.getMessage()));
    EventBook book = resp.getEvents();
    assertEquals(count, book.getPagesCount(), "recorded events");
    for (int i = 0; i < count; i++) {
      assertEquals(start + i, book.getPages(i).getHeader().getSequence(), "event " + i + " sequence");
    }
  }

  @Then("the command is rejected as {word}")
  public void rejectedAs(String code) {
    failsWith(code);
  }

  @Then("the command fails with {word}")
  public void failsWithCode(String code) {
    failsWith(code);
  }

  private void failsWith(String code) {
    assertNotNull(err, "expected coded error " + code);
    assertEquals(code, err.code);
  }

  @Then("no events are recorded")
  public void noEventsRecorded() {
    // After a rejection the response is absent — that is zero events (Go's
    // nil-safe getters return 0; Java must guard the null explicitly).
    int pages = resp == null ? 0 : resp.getEvents().getPagesCount();
    assertEquals(0, pages, "expected no events");
  }

  @Then("the recorded events carry the parent linkage")
  public void eventsCarryParentLinkage() {
    assertNull(err, "dispatch failed");
    Any ext = resp.getEvents().getCover().getExt();
    assertEquals(Builders.parentLinkage(), ext, "cover ext = parent linkage");
  }

  @Then("the compensations run first then second")
  public void compensationsFirstThenSecond() {
    assertNull(err, "dispatch failed");
    EventBook book = resp.getEvents();
    List<String> want = List.of("test.counter.CompensatedFirst", "test.counter.CompensatedSecond");
    assertEquals(want.size(), book.getPagesCount(), "compensation events");
    for (int i = 0; i < want.size(); i++) {
      assertEquals(want.get(i), fqFromUrl(book.getPages(i).getEvent().getTypeUrl()), "compensation " + i);
    }
  }

  @Then("no compensation is recorded")
  public void noCompensation() {
    assertNull(err, "dispatch failed");
    assertEquals(0, resp.getEvents().getPagesCount(), "expected no compensation");
  }

  @Then("the handler saw prior history, at next sequence {int}")
  public void handlerSawPriorHistory(int nextSeq) {
    assertHistory(true, nextSeq);
  }

  @Then("the handler saw no prior history, at next sequence {int}")
  public void handlerSawNoPriorHistory(int nextSeq) {
    assertHistory(false, nextSeq);
  }

  private void assertHistory(boolean wantPrior, int nextSeq) {
    Observation obs = lastObserved();
    assertEquals(wantPrior, obs.hadPriorEvents(), "HadPriorEvents");
    assertEquals(nextSeq, obs.nextSequence(), "NextSequence");
  }

  @Then("the handler saw a counter of {int}, at next sequence {int}")
  public void handlerSawCounter(int count, int nextSeq) {
    Observation obs = lastObserved();
    assertEquals(count, obs.count(), "observed counter");
    assertEquals(nextSeq, obs.nextSequence(), "NextSequence");
  }

  private Observation lastObserved() {
    assertNull(err, () -> "dispatch failed: " + (err == null ? "" : err.getMessage()));
    assertNotNull(observed.isEmpty() ? null : observed, "the handler recorded no observation");
    return observed.get(observed.size() - 1);
  }

  private static String fqFromUrl(String url) {
    int i = url.lastIndexOf('/');
    return i >= 0 ? url.substring(i + 1) : url;
  }
}
