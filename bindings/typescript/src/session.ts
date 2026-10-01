import { type Outcome } from "./statuses";

/**
 * One dispatch's host-side state object, reached from the callback trampoline
 * via its host_ctx id. One dispatch may reach several components (co-resident
 * process managers subscribed to the same trigger), so rebuilt state is keyed
 * per component: each component's state is created lazily by its first
 * stateful callback and shared only by that component's callbacks. State never
 * crosses the FFI. A session routes a callback to its router's registered
 * invoker.
 */
export interface Session {
  /** Lazily creates the state of the component identified by `component` from
   * the factory on that component's first callback, then reuses it for that
   * component across the dispatch. */
  ensureState<T>(component: number, factory: () => T): T;

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
