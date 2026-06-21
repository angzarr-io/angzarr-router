import { type ApplierThunk } from "./thunks";

/**
 * Folds a component's prior events (and optional snapshot) into a state message
 * before a command runs. The factory produces a fresh state message; appliers
 * mutate it page by page. Generic in the state message so appliers stay typed.
 */
export class Rebuilder<T> {
  readonly appliers = new Map<string, ApplierThunk<T>>();
  snapshot?: ApplierThunk<T>;

  /** Starts a rebuilder from a zero-state factory (e.g. `() => create(CounterStateSchema)`). */
  constructor(readonly factory: () => T) {}

  /** Registers an applier for one fully-qualified event type. */
  apply(fullName: string, thunk: ApplierThunk<T>): this {
    this.appliers.set(fullName, thunk);
    return this;
  }

  /** Registers the snapshot loader that seeds state before pages. */
  withSnapshot(thunk: ApplierThunk<T>): this {
    this.snapshot = thunk;
    return this;
  }
}
