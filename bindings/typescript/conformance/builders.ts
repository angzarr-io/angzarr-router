import { create, toBinary } from "@bufbuild/protobuf";
import { type Any, AnySchema } from "@bufbuild/protobuf/wkt";

import {
  type CommandBook,
  CommandBookSchema,
  CommandPageSchema,
  type ContextualCommand,
  ContextualCommandSchema,
  CoverSchema,
  type EventBook,
  EventBookSchema,
  EventPageSchema,
  type Notification,
  NotificationSchema,
  PageHeaderSchema,
  Pack,
  type ProcessManagerHandleRequest,
  ProcessManagerHandleRequestSchema,
  RejectionNotificationSchema,
  type SagaHandleRequest,
  SagaHandleRequestSchema,
  SnapshotSchema,
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

export function sagaEventSource(
  fq: string,
  dest?: Record<string, number>,
): SagaHandleRequest {
  return create(SagaHandleRequestSchema, {
    source: create(EventBookSchema, {
      cover: create(CoverSchema, { domain: "order" }),
      pages: [
        create(EventPageSchema, {
          payload: { case: "event", value: anyEmpty(fq) },
        }),
      ],
    }),
    destinationSequences: dest ?? {},
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

export function pmTrigger(
  domain: string,
  fqs: string[],
  state?: EventBook,
  dest?: Record<string, number>,
): ProcessManagerHandleRequest {
  return create(ProcessManagerHandleRequestSchema, {
    trigger: create(EventBookSchema, {
      cover: create(CoverSchema, { domain }),
      pages: fqs.map((fq) =>
        create(EventPageSchema, {
          payload: { case: "event", value: anyEmpty(fq) },
        }),
      ),
    }),
    processState: state,
    destinationSequences: dest ?? {},
  });
}

export function pmStateOf(n: number): EventBook {
  return create(EventBookSchema, {
    pages: Array.from({ length: n }, () => increasedEventPage()),
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
