package io.angzarr.router.conformance.saga;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.google.protobuf.CodedOutputStream;
import com.google.protobuf.DiscardUnknownFieldsParser;
import io.angzarr.AngzarrDeferredSequence;
import io.angzarr.CommandBook;
import io.angzarr.CommandPage;
import io.angzarr.SagaHandleRequest;
import io.angzarr.SagaResponse;
import io.angzarr.router.CodedError;
import io.angzarr.router.GrpcCode;
import io.angzarr.router.RawRegistration;
import io.angzarr.router.Router;
import io.angzarr.router.SagaDispatch;
import io.angzarr.router.Thunks.SagaEmission;
import io.angzarr.router.conformance.Builders;
import io.angzarr.router.ffi.v1.Abi;
import io.cucumber.java.After;
import io.cucumber.java.Before;
import io.cucumber.java.en.Given;
import io.cucumber.java.en.Then;
import io.cucumber.java.en.When;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.HexFormat;
import java.util.List;

/** Step definitions for saga.feature — the OrderSaga translation-side dispatch. */
public class SagaSteps {

  private Router router;
  private SagaResponse resp;
  private CodedError err;
  private RawRegistration.Outcome registration;
  private final List<Long> seen = new ArrayList<>();

  @Before
  public void before() {
    router = new Router();
    resp = null;
    err = null;
    registration = null;
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

  @Given("a parity saga emitting the parity command twice")
  public void aParitySaga() {
    router.registerSaga(
        new SagaDispatch("parity-saga", "order", List.of("inventory"))
            .onEvent(
                "test.counter.Increased",
                (eventAny, dests, sourceCover) ->
                    new SagaEmission(
                        List.of(Builders.parityCommand(), Builders.parityCommand()), List.of())));
  }

  @When("an Increased event of order root {string} at sequence {int} is dispatched")
  public void rootedIncreased(String label, int seq) {
    dispatch(Builders.sagaRootedSource(label, seq));
  }

  @When("the parity source event at sequence {int} is dispatched")
  public void paritySource(int seq) {
    dispatch(Builders.paritySource(seq));
  }

  @When("a saga declaring a compensation for Reserve is registered")
  public void registerCompensatingSaga() {
    byte[] descriptor =
        Abi.SagaDescriptor.newBuilder()
            .setName("order-saga")
            .setInputDomain("order")
            .addTargetDomains("inventory")
            .addRejections(
                Abi.RejectionEntry.newBuilder()
                    .setCompensates("test.counter.Reserve")
                    .addCallbackIds(1))
            .build()
            .toByteArray();
    registration = RawRegistration.registerSaga(descriptor);
  }

  @Then("the registration is refused as INVALID_ARGUMENT")
  public void registrationRefused() {
    assertNotNull(registration, "a saga was registered");
    assertEquals(-3, registration.ret(), "register returns -INVALID_ARGUMENT");
    assertNotNull(registration.error(), "the binding surfaces a coded error");
    assertEquals(GrpcCode.INVALID_ARGUMENT, registration.error().grpc, "grpc");
  }

  private static AngzarrDeferredSequence deferred(CommandPage page) {
    assertTrue(page.getHeader().hasAngzarrDeferred(), "command page is not deferred");
    return page.getHeader().getAngzarrDeferred();
  }

  @Then("the command leaves its source component to the coordinator")
  public void noSourceComponent() {
    assertNull(err, "dispatch failed");
    for (CommandPage page : resp.getCommands(0).getPagesList()) {
      assertEquals("", deferred(page).getSourceComponent(), "the coordinator stamps the component");
    }
  }

  @Then("the command is deferred from order root {string}")
  public void deferredFromRoot(String label) {
    assertNull(err, "dispatch failed");
    assertEquals(1, resp.getCommandsCount(), "emitted commands");
    for (CommandPage page : resp.getCommands(0).getPagesList()) {
      assertEquals(
          Builders.coverOf("order", label),
          deferred(page).getSource(),
          "the source is the triggering book's whole cover");
    }
  }

  @Then("the command at index {int} hashes to SHA-256 {string}")
  public void commandHash(int index, String hash) throws IOException, NoSuchAlgorithmException {
    assertNull(err, "dispatch failed");
    assertEquals(hash, HexFormat.of().formatHex(sha256(resp.getCommands(index))));
  }

  /** SHA-256 of the command's deterministic encoding, unknown fields discarded. */
  private static byte[] sha256(CommandBook emitted) throws IOException, NoSuchAlgorithmException {
    CommandBook command =
        DiscardUnknownFieldsParser.wrap(CommandBook.parser()).parseFrom(emitted.toByteString());
    ByteArrayOutputStream bytes = new ByteArrayOutputStream();
    CodedOutputStream out = CodedOutputStream.newInstance(bytes);
    out.useDeterministicSerialization();
    command.writeTo(out);
    out.flush();
    return MessageDigest.getInstance("SHA-256").digest(bytes.toByteArray());
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
