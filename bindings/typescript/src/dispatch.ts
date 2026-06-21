import { type Rebuilder } from "./rebuilder";
import {
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

/**
 * One aggregate component's registration: its name, domain, rebuilder, command
 * handlers, and ordered rejection compensators. Generic in the state message so
 * handler thunks see the concrete state — the generated wiring is cast-free.
 */
export class AggregateDispatch<T> {
  readonly commands = new Map<string, CommandThunk<T>>();
  readonly rejections = new Map<string, RejectionThunk<T>[]>();

  constructor(
    readonly name: string,
    readonly domain: string,
    readonly rebuilder: Rebuilder<T>,
  ) {}

  /** Registers a handler for one fully-qualified command type. */
  onCommand(fullName: string, thunk: CommandThunk<T>): this {
    this.commands.set(fullName, thunk);
    return this;
  }

  /** Appends a compensator for one fully-qualified command type; repeated calls
   * register an ordered fan-out. */
  onRejected(fqCommand: string, thunk: RejectionThunk<T>): this {
    appendOrdered(this.rejections, fqCommand, thunk);
    return this;
  }
}

/**
 * One saga component's registration: its name, the input domain it consumes,
 * the domains it issues commands to, its event handlers, and ordered rejection
 * compensators. A saga is stateless — no rebuilder, no state.
 */
export class SagaDispatch {
  readonly events = new Map<string, SagaEventThunk>();
  readonly rejections = new Map<string, SagaRejectionThunk[]>();

  constructor(
    readonly name: string,
    readonly inputDomain: string,
    readonly targets: string[],
  ) {}

  /** Registers the translation thunk for a fully-qualified event type. */
  onEvent(fullName: string, thunk: SagaEventThunk): this {
    this.events.set(fullName, thunk);
    return this;
  }

  /** Appends a compensator for one fully-qualified command type; repeated calls
   * register an ordered fan-out. */
  onRejected(fqCommand: string, thunk: SagaRejectionThunk): this {
    appendOrdered(this.rejections, fqCommand, thunk);
    return this;
  }
}

/**
 * One projector component's registration: its name, projection-state factory,
 * the domains it folds, per-event fold thunks, and the finisher that carries the
 * cover onto the Projection. Generic in the projection state message.
 */
export class ProjectorDispatch<T> {
  domains: string[] = [];
  readonly events = new Map<string, ProjectorEventThunk<T>>();
  finisher?: ProjectorFinishThunk<T>;
  unknown?: ProjectorUnknownThunk;

  constructor(
    readonly name: string,
    readonly factory: () => T,
  ) {}

  /** Declares the domains this projector folds. */
  forDomains(...domains: string[]): this {
    this.domains = domains;
    return this;
  }

  /** Registers the fold thunk for a fully-qualified event type. */
  onEvent(fullName: string, thunk: ProjectorEventThunk<T>): this {
    this.events.set(fullName, thunk);
    return this;
  }

  /** Registers the finisher that produces the Projection from the folded state. */
  finish(thunk: ProjectorFinishThunk<T>): this {
    this.finisher = thunk;
    return this;
  }

  /** Registers an optional observer for events outside the declared set. */
  onUnknown(thunk: ProjectorUnknownThunk): this {
    this.unknown = thunk;
    return this;
  }
}

/**
 * One process-manager component's registration: its name, the domain it issues
 * commands to, its rebuilder, per-(source-domain, event) handlers, and ordered
 * rejection compensators. A PM is stateful — its appliers fold process state
 * before a handler runs, exactly as an aggregate does. Generic in the state.
 */
export class ProcessManagerDispatch<T> {
  // source domain → fully-qualified event type → handler
  readonly handlers = new Map<string, Map<string, PmEventThunk<T>>>();
  readonly rejections = new Map<string, PmRejectionThunk<T>[]>();

  constructor(
    readonly name: string,
    readonly pmDomain: string,
    readonly rebuilder: Rebuilder<T>,
  ) {}

  /** Registers the handler for one source-domain event type. */
  onEvent(
    sourceDomain: string,
    fullName: string,
    thunk: PmEventThunk<T>,
  ): this {
    let byType = this.handlers.get(sourceDomain);
    if (!byType) {
      byType = new Map();
      this.handlers.set(sourceDomain, byType);
    }
    byType.set(fullName, thunk);
    return this;
  }

  /** Appends a compensator for one fully-qualified command type; repeated calls
   * register an ordered fan-out. */
  onRejected(fqCommand: string, thunk: PmRejectionThunk<T>): this {
    appendOrdered(this.rejections, fqCommand, thunk);
    return this;
  }
}

function appendOrdered<V>(map: Map<string, V[]>, key: string, value: V): void {
  const list = map.get(key);
  if (list) {
    list.push(value);
  } else {
    map.set(key, [value]);
  }
}
