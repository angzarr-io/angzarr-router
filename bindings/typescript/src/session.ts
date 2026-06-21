import { type Outcome } from "./statuses";

/**
 * One dispatch's host-side state object, reached from the callback trampoline
 * via its host_ctx id. The rebuilt state is created lazily by the first stateful
 * callback (all callbacks in one dispatch share it); state never crosses the
 * FFI. A session routes a callback to its router's registered invoker.
 */
export interface Session {
  /** Lazily creates the host state from the factory on first callback, then
   * reuses it across the dispatch. */
  ensureState<T>(factory: () => T): T;

  /** Routes one host callback to the registered invoker, marshaling inputs and
   * returning the response bytes + ABI status. Never throws — the firewall is
   * here, so a handler error becomes a coded negative status, never an unwind
   * across the FFI. */
  handleCallback(
    callbackId: number,
    typeUrl: string,
    payload: Uint8Array,
    aux: Uint8Array,
  ): Outcome;
}
