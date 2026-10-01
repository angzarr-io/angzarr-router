package io.angzarr.router.conformance.saga;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.angzarr.AngzarrDeferredSequence;
import io.angzarr.CommandPage;
import io.angzarr.SagaHandleRequest;
import io.angzarr.SagaResponse;
import io.angzarr.router.CodedError;
import io.angzarr.router.Router;
import io.angzarr.router.conformance.Builders;
import io.cucumber.java.After;
import io.cucumber.java.Before;
import io.cucumber.java.en.Given;
import io.cucumber.java.en.Then;
import io.cucumber.java.en.When;
import java.util.ArrayList;
import java.util.List;

/** Step definitions for saga.feature — the OrderSaga translation-side dispatch. */
public class SagaSteps {

  private Router router;
  private SagaResponse resp;
  private CodedError err;
  private final List<Long> seen = new ArrayList<>();

  @Before
  public void before() {
    router = new Router();
    resp = null;
    err = null;
    seen.clear();
  }

  @After
  public void after() {
    if (router != null) {
      router.close();
    }
  }

  private void dispatch(SagaHandleRequest req) {
    try {
      resp = router.dispatchSaga(req);
      err = null;
    } catch (CodedError e) {
      err = e;
      resp = null;
    }
  }

  @Given("an order saga delivering to {string}")
  public void anOrderSaga(String target) {
    router.registerSaga(SagaFixture.recording(new SagaFixture(), target, seen));
  }

  @When("an Increased event at sequence {int} is dispatched")
  public void increasedAt(int seq) {
    dispatch(Builders.sagaEventSource("test.counter.Increased", seq));
  }

  @When("a Reserve event is dispatched")
  public void reserveEvent() {
    dispatch(Builders.sagaEventSource("test.counter.Reserve", null));
  }

  @When("a source with no pages is dispatched")
  public void sourceNoPages() {
    dispatch(Builders.sagaSourceNoPages());
  }

  @When("a request with no source is dispatched")
  public void requestNoSource() {
    dispatch(Builders.sagaRequestNoSource());
  }

  @When("a rejection of Reserve is dispatched")
  public void rejectionReserve() {
    dispatch(Builders.sagaRejectionSource("test.counter.Reserve"));
  }

  @Then("the saga emits one command to {string}")
  public void emitsOneCommand(String target) {
    assertNull(err, "dispatch failed");
    assertEquals(1, resp.getCommandsList().size(), "emitted commands");
    assertEquals(target, resp.getCommandsList().get(0).getCover().getDomain(), "command target");
  }

  @Then("the command is deferred from source sequence {int} at command index {int}")
  public void commandIsDeferred(int seq, int index) {
    assertNull(err, "dispatch failed");
    for (CommandPage page : resp.getCommands(0).getPagesList()) {
      assertTrue(page.getHeader().hasAngzarrDeferred(), "command page is not deferred");
      AngzarrDeferredSequence d = page.getHeader().getAngzarrDeferred();
      assertEquals(seq, d.getSourceSeq(), "source_seq is the triggering event's");
      assertEquals(index, d.getCommandIndex(), "command_index is the emission position");
      assertEquals("order", d.getSource().getDomain(), "the source cover is the triggering book's");
    }
  }

  @Then("the saga handler saw source sequence {int}")
  public void handlerSawSequence(int seq) {
    assertNull(err, "dispatch failed");
    assertEquals(List.of((long) seq), seen, "source sequences the handler saw");
  }

  @Then("the saga emits no commands")
  public void emitsNoCommands() {
    assertNull(err, "dispatch failed");
    assertEquals(0, resp.getCommandsList().size(), "expected no commands");
  }

  @Then("the dispatch fails with {word}")
  public void dispatchFailsWith(String code) {
    assertNotNull(err, "expected coded error " + code);
    assertEquals(code, err.code);
  }

  @Then("the saga injects no events")
  public void injectsNoEvents() {
    assertNull(err, "dispatch failed");
    assertEquals(0, resp.getEventsList().size(), "expected no events");
  }
}
