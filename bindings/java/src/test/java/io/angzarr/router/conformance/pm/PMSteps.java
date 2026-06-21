package io.angzarr.router.conformance.pm;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.angzarr.ProcessManagerHandleRequest;
import io.angzarr.ProcessManagerHandleResponse;
import io.angzarr.router.CodedError;
import io.angzarr.router.Router;
import io.angzarr.router.conformance.Builders;
import io.cucumber.java.After;
import io.cucumber.java.Before;
import io.cucumber.java.en.Given;
import io.cucumber.java.en.Then;
import io.cucumber.java.en.When;
import java.util.List;
import java.util.Map;
import test.counter.counter_angzarr;

/** Step definitions for process_manager.feature — the OrderProcessManager
 * stateful trigger-side dispatch. */
public class PMSteps {

  private Router router;
  private ProcessManagerHandleResponse resp;
  private CodedError err;

  @Before
  public void before() {
    router = new Router();
    counter_angzarr.registerOrderProcessManager(router, new PMFixture());
    resp = null;
    err = null;
  }

  @After
  public void after() {
    if (router != null) {
      router.close();
    }
  }

  private void dispatch(ProcessManagerHandleRequest req) {
    try {
      resp = router.dispatchProcessManager(req);
      err = null;
    } catch (CodedError e) {
      err = e;
      resp = null;
    }
  }

  @Given("an order process-manager")
  public void anOrderProcessManager() {
    // The fixture is registered in @Before.
  }

  @When("an Increased trigger in domain {string} is dispatched with destination inventory sequence {int}")
  public void increasedWithDestination(String domain, int seq) {
    dispatch(
        Builders.pmTrigger(
            domain, List.of("test.counter.Increased"), null, Map.of("inventory", seq)));
  }

  @When("an Increased trigger in domain {string} is dispatched")
  public void increasedInDomain(String domain) {
    dispatch(Builders.pmTrigger(domain, List.of("test.counter.Increased"), null, null));
  }

  @When("a trigger whose newest page is an undeclared event is dispatched")
  public void newestUndeclared() {
    dispatch(
        Builders.pmTrigger(
            "counter", List.of("test.counter.Increased", "test.counter.Unwatched"), null, null));
  }

  @When("an Increased trigger is dispatched over a prior state of {int} events")
  public void increasedOverState(int n) {
    dispatch(
        Builders.pmTrigger("counter", List.of("test.counter.Increased"), Builders.pmStateOf(n), null));
  }

  @When("a request with no trigger is dispatched")
  public void noTrigger() {
    dispatch(Builders.pmNoTrigger());
  }

  @When("a trigger with no pages is dispatched")
  public void emptyTrigger() {
    dispatch(Builders.pmEmptyTrigger());
  }

  @When("a rejection of Reserve is dispatched")
  public void rejectionReserve() {
    dispatch(Builders.pmRejection("test.counter.Reserve"));
  }

  @Then("the process-manager emits one command to {string}")
  public void emitsOneCommand(String target) {
    assertNull(err, "dispatch failed");
    assertEquals(1, resp.getCommandsList().size(), "emitted commands");
    assertEquals(target, resp.getCommandsList().get(0).getCover().getDomain(), "command target");
  }

  @Then("the command carries destination sequence {int}")
  public void commandCarriesSequence(int seq) {
    assertEquals(seq, resp.getCommandsList().get(0).getPages(0).getHeader().getSequence());
  }

  @Then("the process-manager emits no commands")
  public void emitsNoCommands() {
    assertNull(err, "dispatch failed");
    assertEquals(0, resp.getCommandsList().size(), "expected no commands");
  }

  @Then("the process-manager rebuilt {int} prior state events")
  public void rebuiltN(int n) {
    assertNull(err, "dispatch failed");
    assertEquals(n, resp.getFactsList().size(), "rebuilt prior state events");
  }

  @Then("the dispatch fails with {word}")
  public void dispatchFailsWith(String code) {
    assertNotNull(err, "expected coded error " + code);
    assertEquals(code, err.code);
  }

  @Then("the process-manager emits one process event")
  public void emitsOneProcessEvent() {
    assertNull(err, "dispatch failed");
    assertEquals(1, resp.getProcessEventsList().size(), "process events");
  }

  @Then("the process-manager escalates")
  public void escalates() {
    assertNull(err, "dispatch failed");
    assertTrue(resp.hasNotification(), "expected an escalation");
  }
}
