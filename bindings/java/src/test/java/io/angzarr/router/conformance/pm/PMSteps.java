package io.angzarr.router.conformance.pm;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.angzarr.AngzarrDeferredSequence;
import io.angzarr.CommandPage;
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
import test.counter.AuditProcessManagerAngzarr;
import test.counter.OrderProcessManagerAngzarr;

/**
 * Step definitions for process_manager.feature — the OrderProcessManager stateful trigger-side
 * dispatch, alone or co-resident with the AuditProcessManager on one router.
 */
public class PMSteps {

  private Router router;
  private ProcessManagerHandleResponse resp;
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
    OrderProcessManagerAngzarr.registerOrderProcessManager(router, new PMFixture());
  }

  @Given("co-resident order and audit process-managers")
  public void coResidentOrderAndAudit() {
    OrderProcessManagerAngzarr.registerOrderProcessManager(router, new PMFixture());
    AuditProcessManagerAngzarr.registerAuditProcessManager(router, new AuditFixture());
  }

  @When("an Increased trigger in domain {string} at sequence {int} is dispatched")
  public void increasedAt(String domain, int seq) {
    dispatch(Builders.pmTrigger(domain, List.of("test.counter.Increased"), null, seq));
  }

  @When("a Compensate for Reserve is dispatched to the order process-manager")
  public void compensateForReserve() {
    dispatch(Builders.pmCompensate("Reserve"));
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
        Builders.pmTrigger(
            "counter", List.of("test.counter.Increased"), Builders.pmStateOf(n), null));
  }

  @When("an Increased trigger is dispatched over a prior {string} state of {int} events")
  public void increasedOverOwnedState(String owner, int n) {
    dispatch(
        Builders.pmTrigger(
            "counter", List.of("test.counter.Increased"), Builders.pmStateIn(owner, n), null));
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

  @When("a rejection of Reserve issued by {string} is dispatched")
  public void rejectionReserveIssuedBy(String issuer) {
    dispatch(Builders.pmIssuedRejection("test.counter.Reserve", issuer));
  }

  @Then("the process-manager emits one command to {string}")
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
      assertEquals(seq, d.getSourceSeq(), "source_seq is the trigger's");
      assertEquals(index, d.getCommandIndex(), "command_index is the emission position");
      assertEquals("counter", d.getSource().getDomain(), "the source cover is the trigger book's");
    }
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

  @Then("the order process-manager rebuilt {int} prior state events")
  public void orderRebuilt(int n) {
    assertEquals(n, factsMarked(false), "order PM facts = its rebuilt events");
  }

  @Then("the audit process-manager rebuilt {int} prior state events")
  public void auditRebuilt(int n) {
    assertEquals(n, factsMarked(true), "audit PM facts = its rebuilt events");
  }

  @Then("the order process-manager did not react")
  public void orderDidNotReact() {
    assertNull(err, "dispatch failed");
    assertEquals(0, resp.getCommandsList().size(), "the order PM emitted no command");
    assertEquals(0, factsMarked(false), "the order PM emitted no fact");
  }

  @Then("only the audit process-manager compensates")
  public void onlyAuditCompensates() {
    assertNull(err, "dispatch failed");
    assertEquals(1, resp.getProcessEventsList().size(), "exactly one compensation");
    assertEquals(
        AuditFixture.AUDIT_MARK,
        resp.getProcessEvents(0).getCover().getDomain(),
        "the audit PM compensated");
    assertFalse(resp.hasNotification(), "the order PM did not escalate");
  }

  /** Counts response facts whose cover domain is (audit) or is not (!audit) the audit PM's mark. */
  private long factsMarked(boolean audit) {
    assertNull(err, "dispatch failed: " + err);
    return resp.getFactsList().stream()
        .filter(f -> AuditFixture.AUDIT_MARK.equals(f.getCover().getDomain()) == audit)
        .count();
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
