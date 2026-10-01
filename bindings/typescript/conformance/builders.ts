import { createHash } from "node:crypto";

import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import { type Any, AnySchema } from "@bufbuild/protobuf/wkt";

import {
  AngzarrDeferredSequenceSchema,
  type BusinessResponse,
  BusinessResponseSchema,
  type CommandBook,
  CommandBookSchema,
  CommandPageSchema,
  CompensateSchema,
  type ContextualCommand,
  ContextualCommandSchema,
  type Cover,
  CoverSchema,
  type EventBook,
  EventBookSchema,
  EventPageSchema,
  type FactRequest,
  FactRequestSchema,
  type Notification,
  NotificationSchema,
  PageHeaderSchema,
  Pack,
  type ProcessManagerHandleRequest,
  ProcessManagerHandleRequestSchema,
  RejectionNotificationSchema,
  type ReplayRequest,
  ReplayRequestSchema,
  type SagaHandleRequest,
  SagaHandleRequestSchema,
  SnapshotSchema,
  UUIDSchema,
} from "@angzarr/router";
import {
  CounterStateSchema,
  IncreaseBySchema,
} from "../gen/test/counter/counter_pb";

// The framework's canonical bare-"/" type-URL prefix (the core keys dispatch on
// the suffix, so the prefix is immaterial).
const typeUrl = (fq: string): string => `/${fq}`;

const anyOf = (fq: string, value: Uint8Array): Any =>
  create(AnySchema, { typeUrl: typeUrl(fq), value });

const anyEmpty = (fq: string): Any =>
  create(AnySchema, { typeUrl: typeUrl(fq) });

/**
 * The shared conformance envelopes, built BY FIELD (protobuf-es has no
 * text-format parser) — each is an orthogonal envelope wrapping an empty inner
 * message, with the salient field set from the scenario. Byte-equivalent to the
 * conformance/fixtures/*.txtpb skeletons; the asserted behaviour is the same
 * cross-language contract.
 */

// --- commands ---------------------------------------------------------------

export function increaseCommand(n: number): ContextualCommand {
  const inner = create(IncreaseBySchema, { n });
  return create(ContextualCommandSchema, {
    command: create(CommandBookSchema, {
      cover: create(CoverSchema, { domain: "counter" }),
      pages: [
        create(CommandPageSchema, {
          payload: {
            case: "command",
            value: anyOf(
              "test.counter.IncreaseBy",
              toBinary(IncreaseBySchema, inner),
            ),
          },
        }),
      ],
    }),
  });
}

export function increaseCommandWithLinkage(n: number): ContextualCommand {
  const cc = increaseCommand(n);
  cc.command!.cover!.ext = parentLinkage();
  return cc;
}

export function failHardCommand(): ContextualCommand {
  return create(ContextualCommandSchema, {
    command: create(CommandBookSchema, {
      cover: create(CoverSchema, { domain: "counter" }),
      pages: [singleCommandPage(anyEmpty("test.counter.FailHard"))],
    }),
  });
}

export function unhandledCommand(): ContextualCommand {
  return create(ContextualCommandSchema, {
    command: create(CommandBookSchema, {
      cover: create(CoverSchema, { domain: "counter" }),
      pages: [singleCommandPage(anyEmpty("test.counter.Reserve"))],
    }),
  });
}

/** Wraps a rejection Notification for fqCommand into a ContextualCommand — the
 * core detects the notification type and takes the compensation path. */
export function rejectionCommand(fqCommand: string): ContextualCommand {
  return create(ContextualCommandSchema, {
    command: create(CommandBookSchema, {
      cover: create(CoverSchema, { domain: "counter" }),
      pages: [
        singleCommandPage(
          Pack.wrap(
            NotificationSchema,
            rejectionNotificationFor(fqCommand, "counter"),
          ),
        ),
      ],
    }),
  });
}

// --- envelope-guard negatives (one structural field cleared) ----------------

export function commandMissingBook(): ContextualCommand {
  const cc = increaseCommand(1);
  cc.command = undefined;
  return cc;
}

export function commandMissingPage(): ContextualCommand {
  const cc = increaseCommand(1);
  cc.command!.pages = [];
  return cc;
}

export function commandMissingPayload(): ContextualCommand {
  const cc = increaseCommand(1);
  cc.command!.pages[0].payload = { case: undefined };
  return cc;
}

/** An opaque fill-only ext stamped on a command's cover, used to prove ext
 * propagation onto emitted events. */
export function parentLinkage(): Any {
  return anyOf("test.counter.Parent", new Uint8Array([1, 2, 3]));
}

// --- prior history ----------------------------------------------------------

/** Replays the Increased skeleton at sequences 0..n-1 (undefined if 0). */
export function priorIncreases(n: number): EventBook | undefined {
  if (n === 0) {
    return undefined;
  }
  return create(EventBookSchema, {
    nextSequence: n,
    pages: Array.from({ length: n }, (_, i) => increasedPageAt(i)),
  });
}

/** One Increased page whose payload is an undecodable varint
 * (PERSISTED_EVENT_CORRUPT on fold). */
export function corruptHistory(): EventBook {
  const page = create(EventPageSchema, {
    payload: {
      case: "event",
      value: anyOf(
        "test.counter.Increased",
        new Uint8Array([0xff, 0xff, 0xff]),
      ),
    },
    header: sequenceHeader(0),
  });
  return create(EventBookSchema, { pages: [page], nextSequence: 1 });
}

/** Seeds count 10 at sequence 10, plus a covered page (10, skipped) and an
 * uncovered page (11, applied) — a rebuild observes 11. */
export function snapshotHistory(): EventBook {
  return create(EventBookSchema, {
    snapshot: create(SnapshotSchema, {
      sequence: 10,
      state: Pack.wrap(
        CounterStateSchema,
        create(CounterStateSchema, { count: 10 }),
      ),
    }),
    pages: [increasedPageAt(10), increasedPageAt(11)],
    nextSequence: 12,
  });
}

function increasedPageAt(seq: number) {
  return create(EventPageSchema, {
    payload: { case: "event", value: anyEmpty("test.counter.Increased") },
    header: sequenceHeader(seq),
  });
}

function increasedEventPage() {
  return create(EventPageSchema, {
    payload: { case: "event", value: anyEmpty("test.counter.Increased") },
  });
}

function sequenceHeader(seq: number) {
  return create(PageHeaderSchema, {
    sequenceType: { case: "sequence", value: seq },
  });
}

function singleCommandPage(command: Any) {
  return create(CommandPageSchema, {
    payload: { case: "command", value: command },
  });
}

// --- saga / process-manager shared fixtures ---------------------------------

/** The one-page Reserve command the saga and PM emit for "inventory". */
export function reserveCommand(): CommandBook {
  return create(CommandBookSchema, {
    cover: create(CoverSchema, { domain: "inventory" }),
    pages: [singleCommandPage(anyEmpty("test.counter.Reserve"))],
  });
}

/** A single empty fact-event book the compensators inject. */
export function oneFact(): EventBook {
  return create(EventBookSchema, { pages: [create(EventPageSchema, {})] });
}

function rejectionNotificationFor(
  fqCommand: string,
  domain: string,
): Notification {
  const rejection = create(RejectionNotificationSchema, {
    rejectedCommand: create(CommandBookSchema, {
      cover: create(CoverSchema, { domain }),
      pages: [singleCommandPage(anyEmpty(fqCommand))],
    }),
  });
  return create(NotificationSchema, {
    payload: Pack.wrap(RejectionNotificationSchema, rejection),
  });
}

// --- saga dispatch requests -------------------------------------------------

/** A saga request whose source carries one event of `fq` in the "order"
 * domain, at sequence `seq` when given. */
export function sagaEventSource(fq: string, seq?: number): SagaHandleRequest {
  return create(SagaHandleRequestSchema, {
    source: create(EventBookSchema, {
      cover: create(CoverSchema, { domain: "order" }),
      pages: [
        create(EventPageSchema, {
          payload: { case: "event", value: anyEmpty(fq) },
          header: seq === undefined ? undefined : sequenceHeader(seq),
        }),
      ],
    }),
  });
}

export function sagaRejectionSource(fqCommand: string): SagaHandleRequest {
  return create(SagaHandleRequestSchema, {
    source: create(EventBookSchema, {
      cover: create(CoverSchema, { domain: "order" }),
      pages: [
        create(EventPageSchema, {
          payload: {
            case: "event",
            value: Pack.wrap(
              NotificationSchema,
              rejectionNotificationFor(fqCommand, "inventory"),
            ),
          },
        }),
      ],
    }),
  });
}

export function sagaSourceNoPages(): SagaHandleRequest {
  return create(SagaHandleRequestSchema, {
    source: create(EventBookSchema, {}),
  });
}

export function sagaRequestNoSource(): SagaHandleRequest {
  return create(SagaHandleRequestSchema, {});
}

// --- projector deliveries ---------------------------------------------------

export function deliveryBook(domain: string, n: number): EventBook {
  return create(EventBookSchema, {
    cover: create(CoverSchema, { domain }),
    pages: Array.from({ length: n }, () => increasedEventPage()),
  });
}

export function deliveryNoCover(): EventBook {
  const book = deliveryBook("counter", 1);
  book.cover = undefined;
  return book;
}

// --- process-manager triggers -----------------------------------------------

/** A PM request whose trigger carries one page per `fqs` in `domain`, the
 * newest at sequence `newestSeq` when given; `state` is the PM's prior state. */
export function pmTrigger(
  domain: string,
  fqs: string[],
  state?: EventBook,
  newestSeq?: number,
): ProcessManagerHandleRequest {
  const pages = fqs.map((fq) =>
    create(EventPageSchema, {
      payload: { case: "event", value: anyEmpty(fq) },
    }),
  );
  if (newestSeq !== undefined && pages.length > 0) {
    pages[pages.length - 1].header = sequenceHeader(newestSeq);
  }
  return create(ProcessManagerHandleRequestSchema, {
    trigger: create(EventBookSchema, {
      cover: create(CoverSchema, { domain }),
      pages,
    }),
    processState: state,
  });
}

export function pmStateOf(n: number): EventBook {
  return create(EventBookSchema, {
    pages: Array.from({ length: n }, () => increasedEventPage()),
  });
}

/** A PM process-state book of `n` Increased events owned by `pmDomain` (its
 * cover addresses the owning PM). */
export function pmStateIn(pmDomain: string, n: number): EventBook {
  const book = pmStateOf(n);
  book.cover = create(CoverSchema, { domain: pmDomain });
  return book;
}

/** A PM request delivering the rejection of `fqCommand` that the PM owning
 * `issuerDomain` issued: the trigger cover is the issuer's domain and the
 * rejected command's angzarr_deferred header names it as the source. */
export function pmIssuedRejection(
  fqCommand: string,
  issuerDomain: string,
): ProcessManagerHandleRequest {
  const rejection = rejectionNotificationFor(fqCommand, "inventory");
  const rejected = fromBinary(
    RejectionNotificationSchema,
    rejection.payload!.value,
  );
  rejected.rejectedCommand!.pages[0].header = create(PageHeaderSchema, {
    sequenceType: {
      case: "angzarrDeferred",
      value: create(AngzarrDeferredSequenceSchema, {
        source: create(CoverSchema, { domain: issuerDomain }),
      }),
    },
  });
  const notification = create(NotificationSchema, {
    payload: Pack.wrap(RejectionNotificationSchema, rejected),
  });
  return create(ProcessManagerHandleRequestSchema, {
    trigger: create(EventBookSchema, {
      cover: create(CoverSchema, { domain: issuerDomain }),
      pages: [
        create(EventPageSchema, {
          payload: {
            case: "event",
            value: Pack.wrap(NotificationSchema, notification),
          },
        }),
      ],
    }),
  });
}

export function pmRejection(fqCommand: string): ProcessManagerHandleRequest {
  return create(ProcessManagerHandleRequestSchema, {
    trigger: create(EventBookSchema, {
      cover: create(CoverSchema, { domain: "counter" }),
      pages: [
        create(EventPageSchema, {
          payload: {
            case: "event",
            value: Pack.wrap(
              NotificationSchema,
              rejectionNotificationFor(fqCommand, "inventory"),
            ),
          },
        }),
      ],
    }),
  });
}

export function pmNoTrigger(): ProcessManagerHandleRequest {
  return create(ProcessManagerHandleRequestSchema, {});
}

export function pmEmptyTrigger(): ProcessManagerHandleRequest {
  return create(ProcessManagerHandleRequestSchema, {
    trigger: create(EventBookSchema, {}),
  });
}

// --- type-URL prefixes ------------------------------------------------------

/** Rewrites every Any type URL in the command and its prior history to
 * `prefix` + the fully-qualified name (the text after the last "/"). */
export function withTypeUrlPrefix(
  cc: ContextualCommand,
  prefix: string,
): ContextualCommand {
  const rewrite = (any: Any): void => {
    any.typeUrl = prefix + any.typeUrl.slice(any.typeUrl.lastIndexOf("/") + 1);
  };
  for (const page of cc.command?.pages ?? []) {
    if (page.payload.case === "command") {
      rewrite(page.payload.value);
    }
  }
  for (const page of cc.events?.pages ?? []) {
    if (page.payload.case === "event") {
      rewrite(page.payload.value);
    }
  }
  return cc;
}

// --- compensation routing ---------------------------------------------------

/** A business response carrying one header-less event page of
 * `test.counter.<name>`. */
export function oneEvent(name: string): BusinessResponse {
  return create(BusinessResponseSchema, {
    result: {
      case: "events",
      value: create(EventBookSchema, {
        pages: [
          create(EventPageSchema, {
            payload: { case: "event", value: anyEmpty(`test.counter.${name}`) },
          }),
        ],
      }),
    },
  });
}

/** A Notification command wrapping `payload`, addressed to `domain`, over
 * prior history whose next sequence is `nextSequence` when given. */
function notificationCommand(
  domain: string,
  payload: Any,
  nextSequence?: number,
): ContextualCommand {
  const notification = create(NotificationSchema, { payload });
  return create(ContextualCommandSchema, {
    command: create(CommandBookSchema, {
      cover: create(CoverSchema, { domain }),
      pages: [singleCommandPage(Pack.wrap(NotificationSchema, notification))],
    }),
    events:
      nextSequence === undefined
        ? undefined
        : create(EventBookSchema, {
            nextSequence,
            pages: [
              create(EventPageSchema, {
                header: sequenceHeader(Math.max(nextSequence - 1, 0)),
                payload: {
                  case: "event",
                  value: anyEmpty("test.counter.Unrelated"),
                },
              }),
            ],
          }),
  });
}

/** The rejection of a `test.counter.<command>` sent to `targetDomain`,
 * delivered to the payment aggregate. */
export function rejectionSentTo(
  command: string,
  targetDomain: string,
  nextSequence?: number,
): ContextualCommand {
  const rejection = create(RejectionNotificationSchema, {
    rejectedCommand: create(CommandBookSchema, {
      cover: create(CoverSchema, { domain: targetDomain }),
      pages: [singleCommandPage(anyEmpty(`test.counter.${command}`))],
    }),
  });
  return notificationCommand(
    "payment",
    Pack.wrap(RejectionNotificationSchema, rejection),
    nextSequence,
  );
}

/** The Compensate payload for an executed `test.counter.<command>`. */
export function compensatePayload(command: string): Any {
  return Pack.wrap(
    CompensateSchema,
    create(CompensateSchema, {
      commandType: `test.counter.${command}`,
      sequences: [0],
      reason: "aborted",
    }),
  );
}

/** A Compensate for an executed `test.counter.<command>`, delivered to the
 * inventory aggregate. */
export function compensateFor(command: string): ContextualCommand {
  return notificationCommand("inventory", compensatePayload(command));
}

/** A PM request whose trigger (in the order PM's own domain) is a Compensate
 * for an executed `test.counter.<command>`. */
export function pmCompensateRequest(
  command: string,
): ProcessManagerHandleRequest {
  const notification = create(NotificationSchema, {
    payload: compensatePayload(command),
  });
  return create(ProcessManagerHandleRequestSchema, {
    trigger: create(EventBookSchema, {
      cover: create(CoverSchema, { domain: "order-pm" }),
      pages: [
        create(EventPageSchema, {
          payload: {
            case: "event",
            value: Pack.wrap(NotificationSchema, notification),
          },
        }),
      ],
    }),
  });
}

// --- context: roots, facts, replay, covers ----------------------------------

// The RFC 4122 OID namespace UUID.
const NAMESPACE_OID = "6ba7b8129dad11d180b400c04fd430c8";

/** The root bytes for a label: UUID v5 in the OID namespace. */
export function rootOf(label: string): Uint8Array {
  const digest = createHash("sha1")
    .update(Buffer.from(NAMESPACE_OID, "hex"))
    .update(Buffer.from(label, "utf8"))
    .digest();
  const bytes = new Uint8Array(digest.subarray(0, 16));
  bytes[6] = (bytes[6] & 0x0f) | 0x50;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  return bytes;
}

/** A cover in `domain` with the root for `label`. */
export function coverOf(domain: string, label: string): Cover {
  return create(CoverSchema, {
    domain,
    root: create(UUIDSchema, { value: rootOf(label) }),
  });
}

function increasedPage(seq?: number) {
  return create(EventPageSchema, {
    payload: { case: "event", value: anyEmpty("test.counter.Increased") },
    header: seq === undefined ? undefined : sequenceHeader(seq),
  });
}

/** A FactRequest of `facts` pages of `test.counter.<fact>` in "ledger" over
 * `prior` Increased events. */
export function factRequest(
  fact: string,
  facts: number,
  prior: number,
): FactRequest {
  return create(FactRequestSchema, {
    facts: create(EventBookSchema, {
      cover: create(CoverSchema, { domain: "ledger" }),
      pages: Array.from({ length: facts }, () =>
        create(EventPageSchema, {
          payload: { case: "event", value: anyEmpty(`test.counter.${fact}`) },
        }),
      ),
    }),
    priorEvents: create(EventBookSchema, {
      pages: Array.from({ length: prior }, (_, i) => increasedPage(i)),
      nextSequence: prior,
    }),
  });
}

/** A ReplayRequest: a snapshot of `count` at sequence 1, then `events`
 * Increased events at sequences 2... */
export function replayRequest(count: number, events: number): ReplayRequest {
  return create(ReplayRequestSchema, {
    baseSnapshot: create(SnapshotSchema, {
      sequence: 1,
      state: Pack.wrap(
        CounterStateSchema,
        create(CounterStateSchema, { count }),
      ),
    }),
    events: Array.from({ length: events }, (_, i) => increasedPage(2 + i)),
  });
}

/** An IncreaseBy command for the ledger root `label`. */
export function ledgerCommand(label: string): ContextualCommand {
  return create(ContextualCommandSchema, {
    command: create(CommandBookSchema, {
      cover: coverOf("ledger", label),
      pages: [
        singleCommandPage(
          Pack.wrap(IncreaseBySchema, create(IncreaseBySchema, { n: 1 })),
        ),
      ],
    }),
  });
}

/** The Release command the reserving process-manager's compensator issues to
 * "inventory". */
export function releaseCommand(): CommandBook {
  return create(CommandBookSchema, {
    cover: create(CoverSchema, { domain: "inventory" }),
    pages: [singleCommandPage(anyEmpty("test.counter.Release"))],
  });
}

/** An Increased trigger from "counter" root `label` at sequence `seq`. */
export function reservingTrigger(
  label: string,
  seq: number,
): ProcessManagerHandleRequest {
  return create(ProcessManagerHandleRequestSchema, {
    trigger: create(EventBookSchema, {
      cover: coverOf("counter", label),
      pages: [increasedPage(seq)],
    }),
  });
}

/** The rejection of a Reserve sent to `targetDomain`, delivered to the
 * reserving process-manager's own domain at sequence `seq`. */
export function reservingRejection(
  targetDomain: string,
  seq: number,
): ProcessManagerHandleRequest {
  const rejection = create(RejectionNotificationSchema, {
    rejectedCommand: create(CommandBookSchema, {
      cover: create(CoverSchema, { domain: targetDomain }),
      pages: [singleCommandPage(anyEmpty("test.counter.Reserve"))],
    }),
  });
  const notification = create(NotificationSchema, {
    payload: Pack.wrap(RejectionNotificationSchema, rejection),
  });
  return create(ProcessManagerHandleRequestSchema, {
    trigger: create(EventBookSchema, {
      cover: create(CoverSchema, { domain: "reserving-pm" }),
      pages: [
        create(EventPageSchema, {
          header: sequenceHeader(seq),
          payload: {
            case: "event",
            value: Pack.wrap(NotificationSchema, notification),
          },
        }),
      ],
    }),
  });
}

/** A book of Increased events of "counter" root `label` at `sequences`. */
export function trackedBook(label: string, sequences: number[]): EventBook {
  return create(EventBookSchema, {
    cover: coverOf("counter", label),
    pages: sequences.map((s) => increasedPage(s)),
  });
}
