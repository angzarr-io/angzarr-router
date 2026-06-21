package io.angzarr.router;

/**
 * The type-erased bridge from a callback_id to a registered thunk: it receives
 * the per-dispatch {@link Session} and the marshaled callback inputs and returns
 * the response bytes + ABI status. Throwing is the failure path — the
 * trampoline's single catch is the exception firewall (a thrown exception
 * becomes a coded {@code <0} status, never an unwind across the boundary).
 */
@FunctionalInterface
interface Invoker {
  Result invoke(Session session, String typeUrl, byte[] payload, byte[] aux) throws Exception;

  /**
   * A callback's outcome: response bytes (possibly null/empty) and the ABI
   * status — {@code 0} ok with payload, {@code 1} ok with no payload.
   */
  record Result(byte[] response, int status) {}
}
