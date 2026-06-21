package io.angzarr.router.conformance.projector;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;

import io.angzarr.EventBook;
import io.angzarr.Projection;
import io.angzarr.router.CodedError;
import io.angzarr.router.Router;
import io.angzarr.router.conformance.Builders;
import io.cucumber.java.After;
import io.cucumber.java.Before;
import io.cucumber.java.en.Given;
import io.cucumber.java.en.Then;
import io.cucumber.java.en.When;
import test.counter.counter_angzarr;

/** Step definitions for projector.feature — the CounterProjector read-side fold. */
public class ProjectorSteps {

  private Router router;
  private Projection proj;
  private CodedError err;

  @Before
  public void before() {
    router = new Router();
    counter_angzarr.registerCounterProjector(router, new ProjectorFixture());
    proj = null;
    err = null;
  }

  @After
  public void after() {
    if (router != null) {
      router.close();
    }
  }

  private void dispatch(EventBook book) {
    try {
      proj = router.dispatchProjector(book);
      err = null;
    } catch (CodedError e) {
      err = e;
      proj = null;
    }
  }

  @Given("a counter projection")
  public void aCounterProjection() {
    // The fixture is registered in @Before.
  }

  @When("{int} events are delivered in domain {string}")
  public void eventsDelivered(int n, String domain) {
    dispatch(Builders.deliveryBook(domain, n));
  }

  @When("a delivery arrives with no cover")
  public void deliveryNoCover() {
    dispatch(Builders.deliveryNoCover());
  }

  @Then("the projection records {int} events")
  public void recordsCount(int n) {
    assertNull(err, "dispatch failed");
    assertEquals(n, proj.getSequence(), "projection records");
  }

  @Then("the projection records nothing")
  public void recordsNothing() {
    assertNull(err, "dispatch failed");
    assertEquals(0, proj.getSequence(), "projection records nothing");
  }

  @Then("the delivery fails with {word}")
  public void deliveryFailsWith(String code) {
    assertNotNull(err, "expected coded error " + code);
    assertEquals(code, err.code);
  }
}
