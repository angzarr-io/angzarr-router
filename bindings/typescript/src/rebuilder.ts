import { type DescMessage } from "@bufbuild/protobuf";

import { type ApplierThunk } from "./thunks";

/**
 * Folds a component's prior events (and optional snapshot) into a state message
 * before a handler runs. The factory produces a fresh state message; appliers
 * mutate it page by page. Generic in the state message so appliers stay typed.
 * When the state's protobuf schema is known, an aggregate registered with this
 * rebuilder supports Replay (the router packs the replayed state with it).
 */
export class Rebuilder<T> {
  readonly appliers = new Map<string, ApplierThunk<T>>();
  snapshot?: ApplierThunk<T>;
  stateSchema?: DescMessage;

  /** Starts a rebuilder from a zero-state factory (e.g.
   * `() => create(CounterStateSchema)`), optionally with the state's schema. */
  constructor(
    readonly factory: () => T,
    stateSchema?: DescMessage,
  ) {
    this.stateSchema = stateSchema;
  }

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

  /** Declares the protobuf schema of the state message, enabling Replay. */
  withStateSchema(schema: DescMessage): this {
    this.stateSchema = schema;
    return this;
  }
}
