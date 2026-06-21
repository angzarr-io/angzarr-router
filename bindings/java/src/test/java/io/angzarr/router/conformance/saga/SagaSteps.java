package io.angzarr.router.conformance.saga;

import io.angzarr.router.conformance.Builders;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;

import io.angzarr.SagaHandleRequest;
import io.angzarr.SagaResponse;
import io.angzarr.router.CodedError;
import io.angzarr.router.Router;
import io.cucumber.java.After;
import io.cucumber.java.Before;
import io.cucumber.java.en.Given;
import io.cucumber.java.en.Then;
import io.cucumber.java.en.When;
import java.util.Map;
import test.counter.OrderSagaAngzarr;

/** Step definitions for saga.feature — the OrderSaga translation-side dispatch. */
public class SagaSteps {

  private Router router;
  private SagaResponse resp;
  private CodedError err;

  @Before
  public void before() {
    router = new Router();
    OrderSagaAngzarr.registerOrderSaga(router, new SagaFixture());
    resp = null;
    err = null;
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
    // The fixture is registered in @Before; the delivery target ("inventory")
    // is part of the declaration the generated wiring already carries.
  }

  @When("an Increased event is dispatched with destination inventory sequence {int}")
  public void increasedWithDestination(int seq) {
    dispatch(Builders.sagaEventSource("test.counter.Increased", Map.of("inventory", seq)));
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

  @When("a rejection of Unwatched is dispatched")
  public void rejectionUnwatched() {
    dispatch(Builders.sagaRejectionSource("test.counter.Unwatched"));
  }

  @Then("the saga emits one command to {string}")
  public void emitsOneCommand(String target) {
    assertNull(err, "dispatch failed");
    assertEquals(1, resp.getCommandsList().size(), "emitted commands");
    assertEquals(target, resp.getCommandsList().get(0).getCover().getDomain(), "command target");
  }

  @Then("the command carries destination sequence {int}")
  public void commandCarriesSequence(int seq) {
    assertEquals(seq, resp.getCommandsList().get(0).getPages(0).getHeader().getSequence());
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

  @Then("the saga injects one fact event")
  public void injectsOneEvent() {
    assertNull(err, "dispatch failed");
    assertEquals(1, resp.getEventsList().size(), "injected events");
  }

  @Then("the saga injects no events")
  public void injectsNoEvents() {
    assertNull(err, "dispatch failed");
    assertEquals(0, resp.getEventsList().size(), "expected no events");
  }
}
