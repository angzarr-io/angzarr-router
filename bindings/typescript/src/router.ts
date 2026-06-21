import {
  create,
  fromBinary,
  toBinary,
  type DescMessage,
  type MessageShape,
} from "@bufbuild/protobuf";
import { type Any, AnySchema } from "@bufbuild/protobuf/wkt";

import { unhandled } from "./codedError";
import { Destinations } from "./destinations";
import {
  AggregateDispatch,
  ProcessManagerDispatch,
  ProjectorDispatch,
  SagaDispatch,
} from "./dispatch";
import { Ffi, type Dispatched, type Surface } from "./ffi";
import { type Session } from "./session";
import { errorResult, fromStatusBytes, type Outcome } from "./statuses";
import { CommandContext } from "./thunks";
import {
  type ApplierThunk,
  type CommandThunk,
  type PmEventThunk,
  type PmRejectionThunk,
  type ProjectorEventThunk,
  type ProjectorFinishThunk,
  type ProjectorUnknownThunk,
  type RejectionThunk,
  type SagaEventThunk,
  type SagaRejectionThunk,
} from "./thunks";
import {
  AggregateDescriptorSchema,
  CallbackEntrySchema,
  CommandContextAuxSchema,
  PmEventAuxSchema,
  PmEventEntrySchema,
  ProcessManagerDescriptorSchema,
  ProjectorDescriptorSchema,
  RejectionAuxSchema,
  RejectionEntrySchema,
  SagaDescriptorSchema,
  SagaEventAuxSchema,
} from "../gen/io/angzarr/router/ffi/v1/abi_pb";
import { BusinessResponseSchema } from "../gen/io/angzarr/v1/command_handler_pb";
import {
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
  private readonly ptr: unknown;
  private readonly registry = new Map<number, Invoker>();
  private nextId = 0;

  constructor() {
    if (!abiChecked) {
      Ffi.init();
      abiChecked = true;
    }
    this.ptr = Ffi.routerNew();
  }

  /** The ABI version a compatible loaded cdylib reports. */
  static abiVersion(): number {
    return 1;
  }

  /** Frees the underlying native router. */
  close(): void {
    Ffi.routerFree(this.ptr);
  }

  invokerFor(callbackId: number): Invoker | undefined {
    return this.registry.get(callbackId);
  }

  private assign(invoker: Invoker): bigint {
    const id = ++this.nextId;
    this.registry.set(id, invoker);
    return BigInt(id);
  }

  // --- registration ----------------------------------------------------------

  registerAggregate<T>(d: AggregateDispatch<T>): void {
    const factory = d.rebuilder.factory;
    const desc = create(AggregateDescriptorSchema, {
      name: d.name,
      domain: d.domain,
    });
    for (const [fq, thunk] of d.rebuilder.appliers) {
      desc.appliers.push(
        create(CallbackEntrySchema, {
          fqType: fq,
          callbackId: this.assign(applierInvoker(factory, thunk)),
        }),
      );
    }
    if (d.rebuilder.snapshot) {
      desc.snapshotCallbackId = this.assign(
        applierInvoker(factory, d.rebuilder.snapshot),
      );
    }
    for (const [fq, thunk] of d.commands) {
      desc.commands.push(
        create(CallbackEntrySchema, {
          fqType: fq,
          callbackId: this.assign(commandInvoker(factory, thunk)),
        }),
      );
    }
    for (const [cmd, thunks] of d.rejections) {
      const entry = create(RejectionEntrySchema, { fqCommandType: cmd });
      for (const thunk of thunks) {
        entry.callbackIds.push(this.assign(rejectionInvoker(factory, thunk)));
      }
      desc.rejections.push(entry);
    }
    this.check(
      Ffi.register(
        "aggregate",
        this.ptr,
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
    for (const [fq, thunk] of d.events) {
      desc.events.push(
        create(CallbackEntrySchema, {
          fqType: fq,
          callbackId: this.assign(sagaEventInvoker(thunk)),
        }),
      );
    }
    for (const [cmd, thunks] of d.rejections) {
      const entry = create(RejectionEntrySchema, { fqCommandType: cmd });
      for (const thunk of thunks) {
        entry.callbackIds.push(this.assign(sagaRejectionInvoker(thunk)));
      }
      desc.rejections.push(entry);
    }
    this.check(
      Ffi.register("saga", this.ptr, toBinary(SagaDescriptorSchema, desc)),
    );
  }

  registerProjector<T>(d: ProjectorDispatch<T>): void {
    const factory = d.factory;
    const desc = create(ProjectorDescriptorSchema, { name: d.name });
    desc.domains.push(...d.domains);
    for (const [fq, thunk] of d.events) {
      desc.events.push(
        create(CallbackEntrySchema, {
          fqType: fq,
          callbackId: this.assign(projectorEventInvoker(factory, thunk)),
        }),
      );
    }
    if (d.unknown) {
      desc.unknownCallbackId = this.assign(projectorUnknownInvoker(d.unknown));
    }
    if (d.finisher) {
      desc.finishCallbackId = this.assign(
        projectorFinishInvoker(factory, d.finisher),
      );
    }
    this.check(
      Ffi.register(
        "projector",
        this.ptr,
        toBinary(ProjectorDescriptorSchema, desc),
      ),
    );
  }

  registerProcessManager<T>(d: ProcessManagerDispatch<T>): void {
    const factory = d.rebuilder.factory;
    const desc = create(ProcessManagerDescriptorSchema, {
      name: d.name,
      pmDomain: d.pmDomain,
    });
    for (const [fq, thunk] of d.rebuilder.appliers) {
      desc.appliers.push(
        create(CallbackEntrySchema, {
          fqType: fq,
          callbackId: this.assign(applierInvoker(factory, thunk)),
        }),
      );
    }
    if (d.rebuilder.snapshot) {
      desc.snapshotCallbackId = this.assign(
        applierInvoker(factory, d.rebuilder.snapshot),
      );
    }
    for (const [sourceDomain, byType] of d.handlers) {
      for (const [fq, thunk] of byType) {
        desc.events.push(
          create(PmEventEntrySchema, {
            inputDomain: sourceDomain,
            fqType: fq,
            callbackId: this.assign(pmEventInvoker(factory, thunk)),
          }),
        );
      }
    }
    for (const [cmd, thunks] of d.rejections) {
      const entry = create(RejectionEntrySchema, { fqCommandType: cmd });
      for (const thunk of thunks) {
        entry.callbackIds.push(this.assign(pmRejectionInvoker(factory, thunk)));
      }
      desc.rejections.push(entry);
    }
    this.check(
      Ffi.register(
        "processManager",
        this.ptr,
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

  private dispatchVia(surface: Surface, request: Uint8Array): Dispatched {
    return Ffi.dispatch(surface, this.ptr, new DispatchSession(this), request);
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
 * The rebuilt state is created lazily by the first stateful callback (all
 * callbacks in one dispatch share it).
 */
class DispatchSession implements Session {
  private state: unknown;
  private hasState = false;

  constructor(readonly router: Router) {}

  ensureState<T>(factory: () => T): T {
    if (!this.hasState) {
      this.state = factory();
      this.hasState = true;
    }
    return this.state as T;
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

const OK: Outcome = { response: null, status: Ffi.STATUS_OK };
const OK_EMPTY: Outcome = { response: null, status: Ffi.STATUS_OK_EMPTY };

function applierInvoker<T>(factory: () => T, thunk: ApplierThunk<T>): Invoker {
  return (session, typeUrl, payload) => {
    thunk(session.ensureState(factory), anyOf(typeUrl, payload));
    return OK;
  };
}

function commandInvoker<T>(factory: () => T, thunk: CommandThunk<T>): Invoker {
  return (session, typeUrl, payload, aux) => {
    const cax = fromBinary(CommandContextAuxSchema, aux);
    const cctx = new CommandContext(cax.nextSequence, cax.hadPriorEvents);
    const book = thunk(
      anyOf(typeUrl, payload),
      session.ensureState(factory),
      cctx,
    );
    return book === undefined
      ? OK_EMPTY
      : { response: toBinary(EventBookSchema, book), status: Ffi.STATUS_OK };
  };
}

function rejectionInvoker<T>(
  factory: () => T,
  thunk: RejectionThunk<T>,
): Invoker {
  return (session, _typeUrl, _payload, aux) => {
    const rax = fromBinary(RejectionAuxSchema, aux);
    const n = fromBinary(NotificationSchema, rax.notification);
    const rej = fromBinary(RejectionNotificationSchema, rax.rejection);
    const cctx = new CommandContext(
      rax.cctx?.nextSequence ?? 0,
      rax.cctx?.hadPriorEvents ?? false,
    );
    const resp = thunk(n, rej, session.ensureState(factory), cctx);
    return resp === undefined
      ? OK_EMPTY
      : {
          response: toBinary(BusinessResponseSchema, resp),
          status: Ffi.STATUS_OK,
        };
  };
}

function projectorEventInvoker<T>(
  factory: () => T,
  thunk: ProjectorEventThunk<T>,
): Invoker {
  return (session, typeUrl, payload) => {
    thunk(session.ensureState(factory), anyOf(typeUrl, payload));
    return OK;
  };
}

function projectorFinishInvoker<T>(
  factory: () => T,
  thunk: ProjectorFinishThunk<T>,
): Invoker {
  return (session, _typeUrl, payload) => {
    const book =
      payload.length > 0
        ? fromBinary(EventBookSchema, payload)
        : create(EventBookSchema);
    const proj = thunk(session.ensureState(factory), book);
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

function sagaEventInvoker(thunk: SagaEventThunk): Invoker {
  return (_session, typeUrl, payload, aux) => {
    const sax = fromBinary(SagaEventAuxSchema, aux);
    const dests = new Destinations(sax.destinationSequences);
    const emission = thunk(anyOf(typeUrl, payload), dests);
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

function sagaRejectionInvoker(thunk: SagaRejectionThunk): Invoker {
  return (_session, _typeUrl, _payload, aux) => {
    const rax = fromBinary(RejectionAuxSchema, aux);
    const n = fromBinary(NotificationSchema, rax.notification);
    const rej = fromBinary(RejectionNotificationSchema, rax.rejection);
    const resp = create(SagaResponseSchema, { events: thunk(n, rej) });
    return {
      response: toBinary(SagaResponseSchema, resp),
      status: Ffi.STATUS_OK,
    };
  };
}

function pmEventInvoker<T>(factory: () => T, thunk: PmEventThunk<T>): Invoker {
  return (session, typeUrl, payload, aux) => {
    const pax = fromBinary(PmEventAuxSchema, aux);
    const dests = new Destinations(pax.destinationSequences);
    const resp = thunk(
      anyOf(typeUrl, payload),
      session.ensureState(factory),
      dests,
    );
    return {
      response: toBinary(ProcessManagerHandleResponseSchema, resp),
      status: Ffi.STATUS_OK,
    };
  };
}

function pmRejectionInvoker<T>(
  factory: () => T,
  thunk: PmRejectionThunk<T>,
): Invoker {
  return (session, _typeUrl, _payload, aux) => {
    const rax = fromBinary(RejectionAuxSchema, aux);
    const n = fromBinary(NotificationSchema, rax.notification);
    const rej = fromBinary(RejectionNotificationSchema, rax.rejection);
    const r = thunk(n, rej, session.ensureState(factory));
    const resp = create(ProcessManagerHandleResponseSchema, {
      processEvents: r.processEvents,
    });
    if (r.escalation) {
      resp.notification = r.escalation;
    }
    return {
      response: toBinary(ProcessManagerHandleResponseSchema, resp),
      status: Ffi.STATUS_OK,
    };
  };
}
