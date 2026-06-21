import { type Any } from "@bufbuild/protobuf/wkt";

import { type Destinations } from "./destinations";
import {
  type CommandBook,
  type EventBook,
  type Notification,
  type Projection,
  type RejectionNotification,
} from "../gen/io/angzarr/v1/types_pb";
import { type BusinessResponse } from "../gen/io/angzarr/v1/command_handler_pb";
import { type ProcessManagerHandleResponse } from "../gen/io/angzarr/v1/process_manager_pb";

/**
 * The historical-state evidence a handler sees. Host state never crosses the
 * FFI, so the core reconstructs this from the prior-events book and hands it
 * back — the engine's CommandContext made to survive the seam.
 */
export class CommandContext {
  constructor(
    /** The aggregate's next event sequence, derived from the prior-events book. */
    readonly nextSequence: number,
    /** True when the prior-events book carried any history — the
     * "does this aggregate exist" signal a zero state cannot convey. */
    readonly hadPriorEvents: boolean,
  ) {}
}

// The typed business thunks the dispatch builders hold and the generated wiring
// provides. Stateful thunks are generic in the component's state message (T) —
// the generated wiring is cast-free. A thunk throws to fail; the trampoline
// catches and codes it.

/** Folds one event into rebuilding state. */
export type ApplierThunk<T> = (state: T, event: Any) => void;

/** Handles a command; returns the EventBook to persist, or undefined for nothing. */
export type CommandThunk<T> = (
  command: Any,
  state: T,
  cctx: CommandContext,
) => EventBook | undefined;

/** Compensates a rejected command; returns a BusinessResponse, or undefined. */
export type RejectionThunk<T> = (
  notification: Notification,
  rejection: RejectionNotification,
  state: T,
  cctx: CommandContext,
) => BusinessResponse | undefined;

/** Folds one event into a projection. */
export type ProjectorEventThunk<T> = (projection: T, event: Any) => void;

/** Produces the Projection from the folded projection state. */
export type ProjectorFinishThunk<T> = (
  projection: T,
  events: EventBook,
) => Projection;

/** Observes events outside the declared fold set. */
export type ProjectorUnknownThunk = (typeUrl: string) => void;

/** Translates one source event into a saga emission (stateless). */
export type SagaEventThunk = (event: Any, dests: Destinations) => SagaEmission;

/** Compensates a rejected command from a saga (stateless). */
export type SagaRejectionThunk = (
  notification: Notification,
  rejection: RejectionNotification,
) => EventBook[];

/** Handles one source event in a process manager. */
export type PmEventThunk<T> = (
  event: Any,
  state: T,
  dests: Destinations,
) => ProcessManagerHandleResponse;

/** Compensates a rejected command from a process manager. */
export type PmRejectionThunk<T> = (
  notification: Notification,
  rejection: RejectionNotification,
  state: T,
) => PmRejection;

/** A saga event's emission: commands to issue + fact events to inject. */
export interface SagaEmission {
  commands: CommandBook[];
  events: EventBook[];
}

/** A PM rejection's result: process events to fold + an optional escalation
 * notification (undefined for none). */
export interface PmRejection {
  processEvents: EventBook[];
  escalation?: Notification;
}
