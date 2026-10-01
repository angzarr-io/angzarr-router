import {
  create,
  fromBinary,
  toBinary,
  type DescMessage,
  type MessageShape,
} from "@bufbuild/protobuf";
import { type Any, AnySchema } from "@bufbuild/protobuf/wkt";

import { routerClosed, unhandled } from "./codedError";
import { Destinations } from "./destinations";
import {
  AggregateDispatch,
  ProcessManagerDispatch,
  ProjectorDispatch,
  SagaDispatch,
} from "./dispatch";
import { Ffi, type Dispatched, type Surface } from "./ffi";
import { Pack } from "./pack";
import { type Session } from "./session";
import { errorResult, fromStatusBytes, type Outcome } from "./statuses";
import { CommandContext } from "./thunks";
import {
  type ApplierContextThunk,
  type ApplierThunk,
  type CommandThunk,
  type FactThunk,
  type PmEventThunk,
  type PmRejection,
  type PmRejectionThunk,
  type ProjectorEventThunk,
  type ProjectorFinishThunk,
  type ProjectorUnknownThunk,
  type RejectionThunk,
  type SagaEventContextThunk,
  type UndoThunk,
} from "./thunks";
import {
  AggregateDescriptorSchema,
  CallbackEntrySchema,
  type CommandContextAux,
  CommandContextAuxSchema,
  PmEventAuxSchema,
  PmEventEntrySchema,
  ProcessManagerDescriptorSchema,
  ProjectorDescriptorSchema,
  ProjectorEventAuxSchema,
  RejectionAuxSchema,
  RejectionEntrySchema,
  ReplayCallSchema,
  SagaDescriptorSchema,
  SagaEventAuxSchema,
  UndoAuxSchema,
} from "../gen/io/angzarr/router/ffi/v1/abi_pb";
import {
  BusinessResponseSchema,
  type FactRequest,
  FactRequestSchema,
  type ReplayRequest,
  type ReplayResponse,
  ReplayResponseSchema,
} from "../gen/io/angzarr/v1/command_handler_pb";
import {
  CompensateSchema,
  type ContextualCommand,
  ContextualCommandSchema,
  type EventBook,
  EventBookSchema,
  NotificationSchema,
  type Projection,
  ProjectionSchema,
  RejectionNotificationSchema,
} from "../gen/io/angzarr/v1/types_pb";
import {
  type ProcessManagerHandleRequest,
  ProcessManagerHandleRequestSchema,
  type ProcessManagerHandleResponse,
  ProcessManagerHandleResponseSchema,
} from "../gen/io/angzarr/v1/process_manager_pb";
import {
  type SagaHandleRequest,
  SagaHandleRequestSchema,
  type SagaResponse,
  SagaResponseSchema,
} from "../gen/io/angzarr/v1/saga_pb";

// A type-erased bridge from a callback_id to a registered thunk: it receives the
// per-dispatch session and the marshaled callback inputs and returns the
// response bytes + ABI status. Throwing is the failure path — the session's
// single catch is the exception firewall.
type Invoker = (
  session: DispatchSession,
  typeUrl: string,
  payload: Uint8Array,
  aux: Uint8Array,
) => Outcome;

let abiChecked = false;

/**
 * The TypeScript binding's router: wraps the native router plus the host-side
 * callback registry the core reaches through the single callback trampoline.
 * Register a component (assigning callback ids to its thunks and handing the
 * core a serialized descriptor), then dispatch books/commands through it.
 */
export class Router {
  // The native router; null once closed.
  private ptr: unknown;
  private readonly registry = new Map<number, Invoker>();
  private nextId = 0;
  private nextComponent = 0;

  constructor() {
    if (!abiChecked) {
      Ffi.init();
      abiChecked = true;
    }
    this.ptr = Ffi.routerNew();
  }

  /** The ABI version the loaded router-ffi library reports. */
  static abiVersion(): number {
    return Ffi.abiVersion();
  }

  /** Frees the underlying native router exactly once; closing a closed router
   * is a no-op. A dispatch or registration on a closed router throws
   * ROUTER_CLOSED. */
  close(): void {
    const ptr = this.ptr;
    if (ptr === null) {
      return;
    }
    this.ptr = null;
    Ffi.routerFree(ptr);
  }

  /** The live native router; throws ROUTER_CLOSED once closed. */
  private native(): unknown {
    if (this.ptr === null) {
      throw routerClosed();
    }
    return this.ptr;
  }

  invokerFor(callbackId: number): Invoker | undefined {
    return this.registry.get(callbackId);
  }

  /** A fresh key identifying one registered component's host state. */
  private component(): number {
    return ++this.nextComponent;
  }

  private assign(invoker: Invoker): bigint {
    const id = ++this.nextId;
    this.registry.set(id, invoker);
    return BigInt(id);
  }

  // --- registration ----------------------------------------------------------

  registerAggregate<T>(d: AggregateDispatch<T>): void {
    const state = stateOf(this.component(), d.rebuilder.factory);
    const desc = create(AggregateDescriptorSchema, {
      name: d.name,
      domain: d.domain,
    });
    for (const [fq, thunk] of d.rebuilder.appliers) {
      desc.appliers.push(
        create(CallbackEntrySchema, {
          fqType: fq,
          callbackId: this.assign(applierContextInvoker(state, thunk)),
        }),
      );
    }
    if (d.rebuilder.snapshot) {
      desc.snapshotCallbackId = this.assign(
        applierInvoker(state, d.rebuilder.snapshot),
      );
    }
    for (const [fq, thunk] of d.commands) {
      desc.commands.push(
        create(CallbackEntrySchema, {
          fqType: fq,
          callbackId: this.assign(commandInvoker(state, thunk)),
        }),
      );
    }
    for (const [compensates, thunks] of d.rejections) {
      const entry = create(RejectionEntrySchema, { compensates });
      for (const thunk of thunks) {
        entry.callbackIds.push(this.assign(rejectionInvoker(state, thunk)));
      }
      desc.rejections.push(entry);
    }
    for (const [fq, thunk] of d.undoes) {
      desc.undoes.push(
        create(CallbackEntrySchema, {
          fqType: fq,
          callbackId: this.assign(undoInvoker(state, thunk)),
        }),
      );
    }
    for (const [fq, thunk] of d.facts) {
      desc.facts.push(
        create(CallbackEntrySchema, {
          fqType: fq,
          callbackId: this.assign(factInvoker(state, thunk)),
        }),
      );
    }
    const schema = d.rebuilder.stateSchema;
    if (schema) {
      desc.stateCallbackId = this.assign(stateInvoker(state, schema));
    }
    this.check(
      Ffi.register(
        "aggregate",
        this.native(),
        toBinary(AggregateDescriptorSchema, desc),
      ),
    );
  }

  registerSaga(d: SagaDispatch): void {
    const desc = create(SagaDescriptorSchema, {
      name: d.name,
      inputDomain: d.inputDomain,
    });
    desc.targetDomains.push(...d.targets);
    const dests = new Destinations(d.targets);
    for (const [fq, thunk] of d.events) {
      desc.events.push(
        create(CallbackEntrySchema, {
          fqType: fq,
          callbackId: this.assign(sagaEventInvoker(dests, thunk)),
        }),
      );
    }
    this.check(
      Ffi.register("saga", this.native(), toBinary(SagaDescriptorSchema, desc)),
    );
  }

  registerProjector<T>(d: ProjectorDispatch<T>): void {
    const state = stateOf(this.component(), d.factory);
    const desc = create(ProjectorDescriptorSchema, { name: d.name });
    desc.domains.push(...d.domains);
    for (const [fq, thunk] of d.events) {
      desc.events.push(
        create(CallbackEntrySchema, {
          fqType: fq,
          callbackId: this.assign(projectorEventInvoker(state, thunk)),
        }),
      );
    }
    if (d.unknown) {
      desc.unknownCallbackId = this.assign(projectorUnknownInvoker(d.unknown));
    }
    if (d.finisher) {
      desc.finishCallbackId = this.assign(
        projectorFinishInvoker(state, d.finisher),
      );
    }
    this.check(
      Ffi.register(
        "projector",
        this.native(),
        toBinary(ProjectorDescriptorSchema, desc),
      ),
    );
  }

  registerProcessManager<T>(d: ProcessManagerDispatch<T>): void {
    const state = stateOf(this.component(), d.rebuilder.factory);
    const desc = create(ProcessManagerDescriptorSchema, {
      name: d.name,
      pmDomain: d.pmDomain,
    });
    desc.targetDomains.push(...d.targets);
    const dests = new Destinations(d.targets);
    for (const [fq, thunk] of d.rebuilder.appliers) {
      desc.appliers.push(
        create(CallbackEntrySchema, {
          fqType: fq,
          callbackId: this.assign(applierContextInvoker(state, thunk)),
        }),
      );
    }
    if (d.rebuilder.snapshot) {
      desc.snapshotCallbackId = this.assign(
        applierInvoker(state, d.rebuilder.snapshot),
      );
    }
    for (const [sourceDomain, byType] of d.handlers) {
      for (const [fq, thunk] of byType) {
        desc.events.push(
          create(PmEventEntrySchema, {
            inputDomain: sourceDomain,
            fqType: fq,
            callbackId: this.assign(pmEventInvoker(state, dests, thunk)),
          }),
        );
      }
    }
    for (const [compensates, thunks] of d.rejections) {
      const entry = create(RejectionEntrySchema, { compensates });
      for (const thunk of thunks) {
        entry.callbackIds.push(this.assign(pmRejectionInvoker(state, thunk)));
      }
      desc.rejections.push(entry);
    }
    const schema = d.rebuilder.stateSchema;
    if (schema) {
      desc.stateCallbackId = this.assign(stateInvoker(state, schema));
    }
    this.check(
      Ffi.register(
        "processManager",
        this.native(),
        toBinary(ProcessManagerDescriptorSchema, desc),
      ),
    );
  }

  private check(ret: number): void {
    if (ret !== 0) {
      throw fromStatusBytes(null, ret);
    }
  }

  // --- dispatch --------------------------------------------------------------

  dispatch(
    command: ContextualCommand,
  ): MessageShape<typeof BusinessResponseSchema> {
    return this.parse(
      this.dispatchVia("aggregate", toBinary(ContextualCommandSchema, command)),
      BusinessResponseSchema,
      "BusinessResponse",
    );
  }

  dispatchSaga(request: SagaHandleRequest): SagaResponse {
    return this.parse(
      this.dispatchVia("saga", toBinary(SagaHandleRequestSchema, request)),
      SagaResponseSchema,
      "SagaResponse",
    );
  }

  dispatchProjector(book: EventBook): Projection {
    return this.parse(
      this.dispatchVia("projector", toBinary(EventBookSchema, book)),
      ProjectionSchema,
      "Projection",
    );
  }

  dispatchProcessManager(
    request: ProcessManagerHandleRequest,
  ): ProcessManagerHandleResponse {
    return this.parse(
      this.dispatchVia(
        "processManager",
        toBinary(ProcessManagerHandleRequestSchema, request),
      ),
      ProcessManagerHandleResponseSchema,
      "ProcessManagerHandleResponse",
    );
  }

  /** Handles facts through the aggregate claiming the facts' cover domain;
   * returns the EventBook of facts to record. */
  dispatchFact(request: FactRequest): EventBook {
    return this.parse(
      this.dispatchVia("fact", toBinary(FactRequestSchema, request)),
      EventBookSchema,
      "EventBook",
    );
  }

  /** Replays history into the state of the aggregate claiming `domain` (empty
   * selects a sole registered aggregate), or of the process manager whose own
   * domain it is when no aggregate claims it; the response carries the state
   * packed as an Any. A component whose rebuilder declares no state schema
   * does not support Replay (NO_HANDLER_REGISTERED). */
  dispatchReplay(domain: string, request: ReplayRequest): ReplayResponse {
    const call = create(ReplayCallSchema, { domain, request });
    return this.parse(
      this.dispatchVia("replay", toBinary(ReplayCallSchema, call)),
      ReplayResponseSchema,
      "ReplayResponse",
    );
  }

  private dispatchVia(surface: Surface, request: Uint8Array): Dispatched {
    return Ffi.dispatch(
      surface,
      this.native(),
      new DispatchSession(this),
      request,
    );
  }

  private parse<Desc extends DescMessage>(
    d: Dispatched,
    schema: Desc,
    what: string,
  ): MessageShape<Desc> {
    if (d.status !== 0) {
      throw fromStatusBytes(d.response, d.status);
    }
    try {
      return fromBinary(schema, d.response);
    } catch (e) {
      throw unhandled(
        `unmarshal ${what}: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
  }
}

// --- session ----------------------------------------------------------------

/**
 * One dispatch's host-side state object, reached from callbacks via host_ctx.
 * Rebuilt state is keyed per component: each component's state is created
 * lazily by its first stateful callback and reached only by that component's
 * callbacks, so co-resident components in one dispatch never share state.
 */
class DispatchSession implements Session {
  private readonly states = new Map<number, unknown>();

  constructor(readonly router: Router) {}

  ensureState<T>(component: number, factory: () => T): T {
    if (!this.states.has(component)) {
      this.states.set(component, factory());
    }
    return this.states.get(component) as T;
  }

  handleCallback(
    callbackId: number,
    typeUrl: string,
    payload: Uint8Array,
    aux: Uint8Array,
  ): Outcome {
    const invoker = this.router.invokerFor(callbackId);
    if (!invoker) {
      return errorResult(
        unhandled(`no host callback registered for id ${callbackId}`),
      );
    }
    try {
      return invoker(this, typeUrl, payload, aux);
    } catch (e) {
      return errorResult(e);
    }
  }
}

// --- invokers (type-erased bridges; thunks stay typed) -----------------------

function anyOf(typeUrl: string, payload: Uint8Array): Any {
  return create(AnySchema, { typeUrl, value: payload });
}

/** One registered component's state identity: the key its host state lives
 * under in a dispatch session, and the factory that creates it. */
interface StateOf<T> {
  component: number;
  factory: () => T;
}

function stateOf<T>(component: number, factory: () => T): StateOf<T> {
  return { component, factory };
}

function ensure<T>(session: DispatchSession, state: StateOf<T>): T {
  return session.ensureState(state.component, state.factory);
}

const OK: Outcome = { response: null, status: Ffi.STATUS_OK };
const OK_EMPTY: Outcome = { response: null, status: Ffi.STATUS_OK_EMPTY };

function applierInvoker<T>(state: StateOf<T>, thunk: ApplierThunk<T>): Invoker {
  return (session, typeUrl, payload) => {
    thunk(ensure(session, state), anyOf(typeUrl, payload));
    return OK;
  };
}

function applierContextInvoker<T>(
  state: StateOf<T>,
  thunk: ApplierContextThunk<T>,
): Invoker {
  return (session, typeUrl, payload, aux) => {
    const pax = fromBinary(ProjectorEventAuxSchema, aux);
    thunk(ensure(session, state), anyOf(typeUrl, payload), {
      cover: pax.cover,
      sequence: pax.sequence,
    });
    return OK;
  };
}

function commandContext(cax: CommandContextAux | undefined): CommandContext {
  return new CommandContext(
    cax?.nextSequence ?? 0,
    cax?.hadPriorEvents ?? false,
    cax?.cover,
  );
}

function businessOutcome(
  resp: MessageShape<typeof BusinessResponseSchema> | undefined,
): Outcome {
  return resp === undefined
    ? OK_EMPTY
    : {
        response: toBinary(BusinessResponseSchema, resp),
        status: Ffi.STATUS_OK,
      };
}

function commandInvoker<T>(state: StateOf<T>, thunk: CommandThunk<T>): Invoker {
  return (session, typeUrl, payload, aux) => {
    const cctx = commandContext(fromBinary(CommandContextAuxSchema, aux));
    const book = thunk(anyOf(typeUrl, payload), ensure(session, state), cctx);
    return book === undefined
      ? OK_EMPTY
      : { response: toBinary(EventBookSchema, book), status: Ffi.STATUS_OK };
  };
}

function rejectionInvoker<T>(
  state: StateOf<T>,
  thunk: RejectionThunk<T>,
): Invoker {
  return (session, _typeUrl, _payload, aux) => {
    const rax = fromBinary(RejectionAuxSchema, aux);
    const n = fromBinary(NotificationSchema, rax.notification);
    const rej = fromBinary(RejectionNotificationSchema, rax.rejection);
    const cctx = commandContext(rax.cctx);
    return businessOutcome(thunk(n, rej, ensure(session, state), cctx));
  };
}

function undoInvoker<T>(state: StateOf<T>, thunk: UndoThunk<T>): Invoker {
  return (session, _typeUrl, _payload, aux) => {
    const uax = fromBinary(UndoAuxSchema, aux);
    const n = fromBinary(NotificationSchema, uax.notification);
    const compensate = fromBinary(CompensateSchema, uax.compensate);
    const cctx = commandContext(uax.cctx);
    return businessOutcome(thunk(n, compensate, ensure(session, state), cctx));
  };
}

function factInvoker<T>(state: StateOf<T>, thunk: FactThunk<T>): Invoker {
  return (session, typeUrl, payload) => {
    const recorded = thunk(anyOf(typeUrl, payload), ensure(session, state));
    return recorded === undefined
      ? OK_EMPTY
      : { response: toBinary(AnySchema, recorded), status: Ffi.STATUS_OK };
  };
}

function stateInvoker<T>(state: StateOf<T>, schema: DescMessage): Invoker {
  return (session) => {
    const packed = Pack.wrap(
      schema,
      ensure(session, state) as MessageShape<DescMessage>,
    );
    return { response: toBinary(AnySchema, packed), status: Ffi.STATUS_OK };
  };
}

function projectorEventInvoker<T>(
  state: StateOf<T>,
  thunk: ProjectorEventThunk<T>,
): Invoker {
  return (session, typeUrl, payload, aux) => {
    const pax = fromBinary(ProjectorEventAuxSchema, aux);
    thunk(ensure(session, state), anyOf(typeUrl, payload), {
      cover: pax.cover,
      sequence: pax.sequence,
    });
    return OK;
  };
}

function projectorFinishInvoker<T>(
  state: StateOf<T>,
  thunk: ProjectorFinishThunk<T>,
): Invoker {
  return (session, _typeUrl, payload) => {
    const book =
      payload.length > 0
        ? fromBinary(EventBookSchema, payload)
        : create(EventBookSchema);
    const proj = thunk(ensure(session, state), book);
    return {
      response: toBinary(ProjectionSchema, proj),
      status: Ffi.STATUS_OK,
    };
  };
}

function projectorUnknownInvoker(thunk: ProjectorUnknownThunk): Invoker {
  return (_session, typeUrl) => {
    thunk(typeUrl);
    return OK;
  };
}

function sagaEventInvoker(
  dests: Destinations,
  thunk: SagaEventContextThunk,
): Invoker {
  return (_session, typeUrl, payload, aux) => {
    const sax = fromBinary(SagaEventAuxSchema, aux);
    const emission = thunk(anyOf(typeUrl, payload), dests, {
      cover: sax.sourceCover,
      sequence: sax.sourceSeq,
    });
    const resp = create(SagaResponseSchema, {
      commands: emission.commands,
      events: emission.events,
    });
    return {
      response: toBinary(SagaResponseSchema, resp),
      status: Ffi.STATUS_OK,
    };
  };
}

function pmEventInvoker<T>(
  state: StateOf<T>,
  dests: Destinations,
  thunk: PmEventThunk<T>,
): Invoker {
  return (session, typeUrl, payload, aux) => {
    const pax = fromBinary(PmEventAuxSchema, aux);
    const resp = thunk(
      anyOf(typeUrl, payload),
      ensure(session, state),
      dests,
      pax.triggerCover,
    );
    return {
      response: toBinary(ProcessManagerHandleResponseSchema, resp),
      status: Ffi.STATUS_OK,
    };
  };
}

function pmRejectionInvoker<T>(
  state: StateOf<T>,
  thunk: PmRejectionThunk<T>,
): Invoker {
  return (session, _typeUrl, _payload, aux) => {
    const rax = fromBinary(RejectionAuxSchema, aux);
    const n = fromBinary(NotificationSchema, rax.notification);
    const rej = fromBinary(RejectionNotificationSchema, rax.rejection);
    const resp = pmCompensation(thunk(n, rej, ensure(session, state)));
    return {
      response: toBinary(ProcessManagerHandleResponseSchema, resp),
      status: Ffi.STATUS_OK,
    };
  };
}

/** A PM compensator's result as the full response: a
 * ProcessManagerHandleResponse passes through whole; the PmRejection shape
 * becomes its process events + escalation. */
function pmCompensation(
  r: PmRejection | ProcessManagerHandleResponse,
): ProcessManagerHandleResponse {
  if ("$typeName" in r) {
    return r;
  }
  const resp = create(ProcessManagerHandleResponseSchema, {
    processEvents: r.processEvents,
  });
  if (r.escalation) {
    resp.notification = r.escalation;
  }
  return resp;
}
