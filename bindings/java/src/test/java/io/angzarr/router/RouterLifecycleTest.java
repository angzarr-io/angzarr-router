package io.angzarr.router;

import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.angzarr.router.conformance.Builders;
import org.junit.jupiter.api.Test;
import test.counter.Counter;

/**
 * The native router is released exactly once; a closed router refuses work with a clear error
 * instead of touching freed memory.
 */
class RouterLifecycleTest {

  private static AggregateDispatch counter() {
    return new AggregateDispatch(
        "LifecycleAggregate", "counter", new Rebuilder(Counter.CounterState::newBuilder));
  }

  @Test
  void dispatchAfterCloseFailsCleanly() {
    Router router = new Router();
    router.registerAggregate(counter());
    router.close();

    IllegalStateException e =
        assertThrows(
            IllegalStateException.class, () -> router.dispatch(Builders.increaseCommand(1)));
    assertTrue(
        e.getMessage().contains("closed"), "error names the closed router: " + e.getMessage());
  }

  @Test
  void registerAfterCloseFailsCleanly() {
    Router router = new Router();
    router.close();

    IllegalStateException e =
        assertThrows(IllegalStateException.class, () -> router.registerAggregate(counter()));
    assertTrue(
        e.getMessage().contains("closed"), "error names the closed router: " + e.getMessage());
  }

  @Test
  void doubleCloseReleasesOnceAndStaysClosed() {
    Router router = new Router();
    router.close();
    router.close();

    assertThrows(
        IllegalStateException.class,
        () -> router.dispatchProjector(Builders.deliveryBook("counter", 1)));
  }
}
