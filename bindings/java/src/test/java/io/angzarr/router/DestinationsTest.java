package io.angzarr.router;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.angzarr.ProcessManagerHandleResponse;
import io.angzarr.router.Thunks.SagaEmission;
import io.angzarr.router.conformance.Builders;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.Test;
import test.counter.Counter;

/** Destinations are a saga's or process manager's declared output domains, nothing more. */
class DestinationsTest {

  private static final String INCREASED = "test.counter.Increased";

  @Test
  void reportsTheDeclaredDomainsInOrder() {
    Destinations d = new Destinations(List.of("inventory", "billing"));
    assertTrue(d.has("inventory"));
    assertTrue(d.has("billing"));
    assertFalse(d.has("shipping"));
    assertEquals(List.of("inventory", "billing"), d.domains());
  }

  @Test
  void nullIsNoDomains() {
    Destinations d = new Destinations(null);
    assertFalse(d.has("inventory"));
    assertEquals(List.of(), d.domains());
  }

  @Test
  void aSagaHandlerSeesTheSagasTargetDomains() {
    List<List<String>> seen = new ArrayList<>();
    SagaDispatch saga =
        new SagaDispatch("S", "order", List.of("inventory", "billing"))
            .onEvent(
                INCREASED,
                (event, dests, cover) -> {
                  seen.add(dests.domains());
                  return new SagaEmission(List.of(), List.of());
                });
    try (Router router = new Router()) {
      router.registerSaga(saga);
      router.dispatchSaga(Builders.sagaEventSource(INCREASED, 1));
    }
    assertEquals(List.of(List.of("inventory", "billing")), seen);
  }

  @Test
  void aProcessManagerHandlerSeesItsRegisteredTargetDomains() {
    List<List<String>> seen = new ArrayList<>();
    ProcessManagerDispatch pm =
        new ProcessManagerDispatch(
                "P",
                "p-pm",
                List.of("inventory"),
                new Rebuilder(Counter.OrderProcessManagerState::newBuilder))
            .onEvent(
                "counter",
                INCREASED,
                (event, state, dests) -> {
                  seen.add(dests.domains());
                  return ProcessManagerHandleResponse.getDefaultInstance();
                });
    try (Router router = new Router()) {
      router.registerProcessManager(pm);
      router.dispatchProcessManager(Builders.pmTrigger("counter", List.of(INCREASED), null, 1));
    }
    assertEquals(List.of(List.of("inventory")), seen);
  }

  @Test
  void aProcessManagerRegisteredWithoutTargetsHasNoDestinations() {
    List<List<String>> seen = new ArrayList<>();
    ProcessManagerDispatch pm =
        new ProcessManagerDispatch(
                "P", "p-pm", new Rebuilder(Counter.OrderProcessManagerState::newBuilder))
            .onEvent(
                "counter",
                INCREASED,
                (event, state, dests) -> {
                  seen.add(dests.domains());
                  return ProcessManagerHandleResponse.getDefaultInstance();
                });
    try (Router router = new Router()) {
      router.registerProcessManager(pm);
      router.dispatchProcessManager(Builders.pmTrigger("counter", List.of(INCREASED), null, 1));
    }
    assertEquals(List.of(List.of()), seen);
  }
}
