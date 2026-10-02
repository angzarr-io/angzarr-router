import { type Rebuilder } from "./rebuilder";
import {
  type CommandThunk,
  type FactThunk,
  type PmEventThunk,
  type PmRejectionThunk,
  type ProjectorEventThunk,
  type ProjectorFinishThunk,
  type ProjectorUnknownThunk,
  type RejectionThunk,
  type SagaEventContextThunk,
  type SagaEventThunk,
  type UndoThunk,
} from "./thunks";

/** A projector domain that consumes every domain. */
export const WILDCARD_DOMAIN = "*";

/**
 * One aggregate component's registration: its name, domain, rebuilder, command
 * handlers, ordered rejection compensators, undo handlers and fact handlers.
 * Generic in the state message so handler thunks see the concrete state — the
 * generated wiring is cast-free.
 */
export class AggregateDispatch<T> {
  readonly commands = new Map<string, CommandThunk<T>>();
  readonly rejections = new Map<string, RejectionThunk<T>[]>();
  readonly undoes = new Map<string, UndoThunk<T>>();
  readonly facts = new Map<string, FactThunk<T>>();

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

  /** Appends a compensator for one compensates entry — the rejected command's
   * fully-qualified type ("fq.Type", sent to any domain) or "domain:fq.Type"
   * (only when sent to that domain); repeated calls register an ordered
   * fan-out. */
  onRejected(compensates: string, thunk: RejectionThunk<T>): this {
    appendOrdered(this.rejections, compensates, thunk);
    return this;
  }

  /** Registers the undo handler for the fully-qualified type of an executed
   * command a Compensate undoes. */
  onUndo(fqCommand: string, thunk: UndoThunk<T>): this {
    this.undoes.set(fqCommand, thunk);
    return this;
  }

  /** Registers the handler for one fully-qualified fact (event) type,
   * declaring that type. The core refuses a fact of an undeclared type with
   * NO_FACT_HANDLER (INVALID_ARGUMENT) before any handler runs. */
  onFact(fqFact: string, thunk: FactThunk<T>): this {
    this.facts.set(fqFact, thunk);
    return this;
  }
}

/**
 * One saga component's registration: its name, the input domain it consumes,
 * the domains it issues commands to, and its event handlers. A saga is
 * stateless — no rebuilder, no state — and receives no rejections.
 */
export class SagaDispatch {
  readonly events = new Map<string, SagaEventContextThunk>();

  constructor(
    readonly name: string,
    readonly inputDomain: string,
    readonly targets: string[],
  ) {}

  /** Registers the translation thunk for a fully-qualified event type. */
  onEvent(fullName: string, thunk: SagaEventThunk): this {
    return this.onEventWithContext(fullName, (event, dests, source) =>
      thunk(event, dests, source.cover),
    );
  }

  /** Registers the translation thunk for a fully-qualified event type; the
   * thunk receives the triggering event's page context (source cover and
   * sequence). */
  onEventWithContext(fullName: string, thunk: SagaEventContextThunk): this {
    this.events.set(fullName, thunk);
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

  /** Declares the domains this projector folds; WILDCARD_DOMAIN folds every
   * domain. */
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
 * One process-manager component's registration: its name, its own domain, its
 * rebuilder, the output domains it issues commands to, per-(source-domain,
 * event) handlers, and ordered rejection compensators. A PM is stateful — its
 * appliers fold process state before a handler runs, exactly as an aggregate
 * does. Generic in the state.
 */
export class ProcessManagerDispatch<T> {
  // source domain → fully-qualified event type → handler
  readonly handlers = new Map<string, Map<string, PmEventThunk<T>>>();
  readonly rejections = new Map<string, PmRejectionThunk<T>[]>();
  readonly targets: string[];

  constructor(
    readonly name: string,
    readonly pmDomain: string,
    readonly rebuilder: Rebuilder<T>,
    targets?: string[],
  ) {
    this.targets = [...(targets ?? [])];
  }

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

  /** Appends a compensator for one compensates entry ("fq.Type" or
   * "domain:fq.Type"); repeated calls register an ordered fan-out. */
  onRejected(compensates: string, thunk: PmRejectionThunk<T>): this {
    appendOrdered(this.rejections, compensates, thunk);
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
