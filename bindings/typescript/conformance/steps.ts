import assert from "node:assert/strict";
import { createHash } from "node:crypto";

import { create, equals, toBinary } from "@bufbuild/protobuf";
import { AnySchema } from "@bufbuild/protobuf/wkt";
import { After, Before, Given, Then, When } from "@cucumber/cucumber";

import {
  CodedError,
  GrpcCode,
  Router,
  SagaDispatch,
  WILDCARD_DOMAIN,
} from "@angzarr/router";
import {
  CommandBookSchema,
  ContextualCommandSchema,
  CoverSchema,
} from "@angzarr/router";
import type {
  AngzarrDeferredSequence,
  BusinessResponse,
  CommandBook,
  ContextualCommand,
  EventBook,
  EventPage,
  PageHeader,
  ProcessManagerHandleResponse,
  Projection,
  SagaResponse,
} from "@angzarr/router";
import {
  RejectionEntrySchema,
  SagaDescriptorSchema,
} from "../gen/io/angzarr/router/ffi/v1/abi_pb";
import { registerAuditProcessManager } from "../gen/test/counter/audit_process_manager_angzarr";
import { registerCounterAggregate } from "../gen/test/counter/counter_aggregate_angzarr";
import {
  newCounterProjectorDispatch,
  registerCounterProjector,
} from "../gen/test/counter/counter_projector_angzarr";
import { registerOrderProcessManager } from "../gen/test/counter/order_process_manager_angzarr";
import { newOrderSagaDispatch } from "../gen/test/counter/order_saga_angzarr";
import * as B from "./builders";
import {
  AUDIT_MARK,
  AuditPmFixture,
  CounterFixture,
  type Observation,
  type RejectionSeen,
  PmFixture,
  ProjectorFixture,
  SagaFixture,
} from "./fixtures";

// One scenario's mutable state. cucumber-js shares no per-feature scoping, so a
// single context routes the saga/pm steps whose feature text collides.
interface Ctx {
  router?: Router;
  kind?: "counter" | "saga" | "projector" | "pm";
  prior?: EventBook;
  businessResp?: BusinessResponse;
  sagaResp?: SagaResponse;
  pmResp?: ProcessManagerHandleResponse;
  proj?: Projection;
  err?: CodedError;
  observed: Observation[];
  sagaSeen: number[];
  /** The (code, message) of each rejection the order PM compensated. */
  pmSeen: RejectionSeen[];
  registration?: unknown;
  registered?: boolean;
}

let ctx: Ctx;

Before(() => {
  ctx = { observed: [], sagaSeen: [], pmSeen: [] };
});

After(() => {
  ctx.router?.close();
});

function rethrowNonCoded(e: unknown): CodedError {
  if (e instanceof CodedError) {
    return e;
  }
  throw e;
}

// --- counter (aggregate) ----------------------------------------------------

function dispatchCounter(cc: ContextualCommand): void {
  if (!ctx.router) {
    startCounter();
  }
  if (ctx.prior) {
    cc.events = ctx.prior;
  }
  try {
    ctx.businessResp = ctx.router!.dispatch(cc);
    ctx.err = undefined;
  } catch (e) {
    ctx.err = rethrowNonCoded(e);
    ctx.businessResp = undefined;
  }
}

function counterEvents(): EventBook | undefined {
  const r = ctx.businessResp;
  return r && r.result.case === "events" ? r.result.value : undefined;
}

function fqFromUrl(url: string): string {
  const i = url.lastIndexOf("/");
  return i >= 0 ? url.slice(i + 1) : url;
}

function sequenceOf(header: PageHeader | undefined): number {
  return header?.sequenceType.case === "sequence"
    ? header.sequenceType.value
    : -1;
}

function eventTypeUrl(page: EventPage): string {
  return page.payload.case === "event" ? page.payload.value.typeUrl : "";
}

Given("a new counter", function () {
  startCounter();
  ctx.prior = undefined;
});

Given(
  "a counter that has already recorded {int} increase(s)",
  function (n: number) {
    startCounter();
    ctx.prior = B.priorIncreases(n);
  },
);

Given("a counter whose history holds a corrupt event", function () {
  startCounter();
  ctx.prior = B.corruptHistory();
});

Given(
  "a counter restored from a snapshot of 10 with one newer event",
  function () {
    startCounter();
    ctx.prior = B.snapshotHistory();
  },
);

function startCounter(): void {
  ctx.kind = "counter";
  ctx.router = new Router();
  registerCounterAggregate(ctx.router, new CounterFixture(ctx.observed));
}

When("the operator increases the counter by {int}", function (n: number) {
  dispatchCounter(B.increaseCommand(n));
});

Given(
  "a counter that has already recorded {int} increases under the {string} type-URL prefix",
  function (n: number, prefix: string) {
    startCounter();
    const carrier = B.withTypeUrlPrefix(
      create(ContextualCommandSchema, { events: B.priorIncreases(n) }),
      prefix,
    );
    ctx.prior = carrier.events;
  },
);

When(
  "the operator increases the counter by {int} under the {string} type-URL prefix",
  function (n: number, prefix: string) {
    dispatchCounter(B.withTypeUrlPrefix(B.increaseCommand(n), prefix));
  },
);

When(
  "the operator increases the counter by {int} on behalf of a parent",
  function (n: number) {
    dispatchCounter(B.increaseCommandWithLinkage(n));
  },
);

When("the operator triggers a hard failure", function () {
  dispatchCounter(B.failHardCommand());
});

When("an unhandled command is dispatched", function () {
  dispatchCounter(B.unhandledCommand());
});

When("a command with no command book is dispatched", function () {
  dispatchCounter(B.commandMissingBook());
});

When("a command with an empty command book is dispatched", function () {
  dispatchCounter(B.commandMissingPage());
});

When("a command whose page carries no payload is dispatched", function () {
  dispatchCounter(B.commandMissingPayload());
});

When("a Reserve command is rejected", function () {
  dispatchCounter(B.rejectionCommand("test.counter.Reserve"));
});

When("an unregistered command is rejected", function () {
  dispatchCounter(B.rejectionCommand("test.counter.Undeclared"));
});

Then("{int} increases are recorded, starting at sequence {int}", recordedAt);
Then(
  "{int} increases are recorded, continuing from sequence {int}",
  recordedAt,
);

function recordedAt(count: number, start: number): void {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  const book = counterEvents();
  assert.equal(book?.pages.length, count, "recorded events");
  for (let i = 0; i < count; i++) {
    assert.equal(
      sequenceOf(book!.pages[i].header),
      start + i,
      `event ${i} sequence`,
    );
  }
}

Then("the command is rejected as {word}", failsWith);
Then("the command fails with {word}", failsWith);

function failsWith(code: string): void {
  assert.ok(ctx.err, `expected coded error ${code}`);
  assert.equal(ctx.err.code, code);
}

Then("no events are recorded", function () {
  assert.equal(counterEvents()?.pages.length ?? 0, 0, "expected no events");
});

Then("the recorded events carry the parent linkage", function () {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  const ext = counterEvents()!.cover?.ext;
  assert.ok(ext, "cover ext present");
  assert.ok(
    equals(AnySchema, ext, B.parentLinkage()),
    "cover ext = parent linkage",
  );
});

Then("the recorded events carry no parent linkage", function () {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  const book = counterEvents();
  assert.ok(book && book.pages.length > 0, "events were recorded");
  assert.equal(book.cover?.ext, undefined, "no cover ext");
});

Then("the compensations run first then second", function () {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  const book = counterEvents();
  const want = [
    "test.counter.CompensatedFirst",
    "test.counter.CompensatedSecond",
  ];
  assert.equal(book?.pages.length, want.length, "compensation events");
  for (let i = 0; i < want.length; i++) {
    assert.equal(
      fqFromUrl(eventTypeUrl(book!.pages[i])),
      want[i],
      `compensation ${i}`,
    );
  }
});

Then("no compensation is recorded", function () {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  assert.equal(
    counterEvents()?.pages.length ?? 0,
    0,
    "expected no compensation",
  );
});

Then(
  "the handler saw no prior history, at next sequence {int}",
  function (nextSeq: number) {
    assertHistory(false, nextSeq);
  },
);

Then(
  "the handler saw prior history, at next sequence {int}",
  function (nextSeq: number) {
    assertHistory(true, nextSeq);
  },
);

Then(
  "the handler saw a counter of {int}, at next sequence {int}",
  function (count: number, nextSeq: number) {
    const obs = lastObserved();
    assert.equal(obs.count, count, "observed counter");
    assert.equal(obs.nextSequence, nextSeq, "next sequence");
  },
);

function assertHistory(wantPrior: boolean, nextSeq: number): void {
  const obs = lastObserved();
  assert.equal(obs.hadPriorEvents, wantPrior, "had prior events");
  assert.equal(obs.nextSequence, nextSeq, "next sequence");
}

function lastObserved(): Observation {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  assert.ok(ctx.observed.length > 0, "the handler recorded no observation");
  return ctx.observed[ctx.observed.length - 1];
}

// --- saga -------------------------------------------------------------------

Given("an order saga delivering to {string}", function (_target: string) {
  ctx.kind = "saga";
  ctx.router = new Router();
  const saga = newOrderSagaDispatch(new SagaFixture());
  const generated = saga.events.get("test.counter.Increased")!;
  saga.onEventWithContext("test.counter.Increased", (event, dests, source) => {
    ctx.sagaSeen.push(source.sequence);
    return generated(event, dests, source);
  });
  ctx.router.registerSaga(saga);
});

Given("a parity saga emitting the parity command twice", function () {
  ctx.kind = "saga";
  ctx.router = new Router();
  ctx.router.registerSaga(
    new SagaDispatch("parity-saga", "order", ["inventory"]).onEvent(
      "test.counter.Increased",
      () => ({
        commands: [B.parityCommand(), B.parityCommand()],
        events: [],
      }),
    ),
  );
});

function dispatchSaga(req: Parameters<Router["dispatchSaga"]>[0]): void {
  try {
    ctx.sagaResp = ctx.router!.dispatchSaga(req);
    ctx.err = undefined;
  } catch (e) {
    ctx.err = rethrowNonCoded(e);
    ctx.sagaResp = undefined;
  }
}

When(
  "an Increased event at sequence {int} is dispatched",
  function (seq: number) {
    dispatchSaga(B.sagaEventSource("test.counter.Increased", seq));
  },
);

When(
  "an Increased event of order root {string} at sequence {int} is dispatched",
  function (label: string, seq: number) {
    dispatchSaga(B.sagaRootedSource(label, seq));
  },
);

When(
  "the parity source event at sequence {int} is dispatched",
  function (seq: number) {
    dispatchSaga(B.paritySource(seq));
  },
);

// The typed SagaDispatch has no way to declare a rejection handler, so the
// descriptor goes through the binding's low-level registration entry point.
When("a saga declaring a compensation for Reserve is registered", function () {
  const router = new Router();
  try {
    router.registerSagaDescriptor(
      create(SagaDescriptorSchema, {
        name: "order-saga",
        inputDomain: "order",
        targetDomains: ["inventory"],
        rejections: [
          create(RejectionEntrySchema, {
            compensates: "test.counter.Reserve",
            callbackIds: [1n],
          }),
        ],
      }),
    );
    ctx.registration = undefined;
  } catch (e) {
    ctx.registration = e;
  } finally {
    router.close();
  }
  ctx.registered = true;
});

Then(
  "the command is deferred from order root {string}",
  function (label: string) {
    assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
    assert.equal(ctx.sagaResp!.commands.length, 1, "emitted commands");
    for (const page of ctx.sagaResp!.commands[0].pages) {
      const st = page.header?.sequenceType;
      assert.equal(st?.case, "angzarrDeferred", "command page is deferred");
      const source = (st!.value as AngzarrDeferredSequence).source;
      assert.ok(source, "deferred source present");
      assert.ok(
        equals(CoverSchema, source, B.coverOf("order", label)),
        "the source is the triggering book's whole cover",
      );
    }
  },
);

Then(
  "the command at index {int} hashes to SHA-256 {string}",
  function (index: number, digest: string) {
    assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
    const command = ctx.sagaResp!.commands[index];
    assert.ok(command, `a command at index ${index}`);
    const encoded = toBinary(CommandBookSchema, command, {
      writeUnknownFields: false,
    });
    assert.equal(createHash("sha256").update(encoded).digest("hex"), digest);
  },
);

Then("the registration is refused as INVALID_ARGUMENT", function () {
  assert.ok(ctx.registered, "a saga registration was attempted");
  const err = ctx.registration;
  assert.ok(err instanceof CodedError, `expected a coded refusal, got ${err}`);
  assert.equal(err.grpc, GrpcCode.InvalidArgument, "gRPC code");
});

When("a Reserve event is dispatched", function () {
  dispatchSaga(B.sagaEventSource("test.counter.Reserve"));
});

When("a source with no pages is dispatched", function () {
  dispatchSaga(B.sagaSourceNoPages());
});

When("a request with no source is dispatched", function () {
  dispatchSaga(B.sagaRequestNoSource());
});

Then("the saga emits one command to {string}", function (target: string) {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  assert.equal(ctx.sagaResp!.commands.length, 1, "emitted commands");
  assert.equal(
    ctx.sagaResp!.commands[0].cover?.domain,
    target,
    "command target",
  );
});

Then("the saga handler saw source sequence {int}", function (seq: number) {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  assert.deepEqual(ctx.sagaSeen, [seq]);
});

Then("the saga emits no commands", function () {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  assert.equal(ctx.sagaResp!.commands.length, 0, "expected no commands");
});

Then("the saga injects no events", function () {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  assert.equal(ctx.sagaResp!.events.length, 0, "expected no events");
});

// --- projector --------------------------------------------------------------

Given("a counter projection", function () {
  ctx.kind = "projector";
  ctx.router = new Router();
  registerCounterProjector(ctx.router, new ProjectorFixture());
});

Given("a counter projection over every domain", function () {
  ctx.kind = "projector";
  ctx.router = new Router();
  ctx.router.registerProjector(
    newCounterProjectorDispatch(new ProjectorFixture()).forDomains(
      WILDCARD_DOMAIN,
    ),
  );
});

function dispatchProjector(book: EventBook): void {
  try {
    ctx.proj = ctx.router!.dispatchProjector(book);
    ctx.err = undefined;
  } catch (e) {
    ctx.err = rethrowNonCoded(e);
    ctx.proj = undefined;
  }
}

When(
  "{int} events are delivered in domain {string}",
  function (n: number, domain: string) {
    dispatchProjector(B.deliveryBook(domain, n));
  },
);

When("a delivery arrives with no cover", function () {
  dispatchProjector(B.deliveryNoCover());
});

Then("the projection records {int} events", function (n: number) {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  assert.equal(ctx.proj!.sequence, n, "projection records");
});

Then("the projection records nothing", function () {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  assert.equal(ctx.proj!.sequence, 0, "projection records nothing");
});

Then("the delivery fails with {word}", failsWith);

// --- process manager --------------------------------------------------------

Given("an order process-manager", function () {
  ctx.kind = "pm";
  ctx.router = new Router();
  registerOrderProcessManager(ctx.router, new PmFixture(ctx.pmSeen));
});

Given("co-resident order and audit process-managers", function () {
  ctx.kind = "pm";
  ctx.router = new Router();
  registerOrderProcessManager(ctx.router, new PmFixture(ctx.pmSeen));
  registerAuditProcessManager(ctx.router, new AuditPmFixture());
});

function dispatchPm(
  req: Parameters<Router["dispatchProcessManager"]>[0],
): void {
  try {
    ctx.pmResp = ctx.router!.dispatchProcessManager(req);
    ctx.err = undefined;
  } catch (e) {
    ctx.err = rethrowNonCoded(e);
    ctx.pmResp = undefined;
  }
}

When(
  "an Increased trigger in domain {string} at sequence {int} is dispatched",
  function (domain: string, seq: number) {
    dispatchPm(B.pmTrigger(domain, ["test.counter.Increased"], undefined, seq));
  },
);

When(
  "a Compensate for Reserve is dispatched to the order process-manager",
  function () {
    dispatchPm(B.pmCompensateRequest("Reserve"));
  },
);

When(
  "an Increased trigger in domain {string} is dispatched",
  function (domain: string) {
    dispatchPm(B.pmTrigger(domain, ["test.counter.Increased"]));
  },
);

When(
  "a trigger whose newest page is an undeclared event is dispatched",
  function () {
    dispatchPm(
      B.pmTrigger("counter", [
        "test.counter.Increased",
        "test.counter.Unwatched",
      ]),
    );
  },
);

When(
  "an Increased trigger is dispatched over a prior state of {int} events",
  function (n: number) {
    dispatchPm(
      B.pmTrigger("counter", ["test.counter.Increased"], B.pmStateOf(n)),
    );
  },
);

When(
  "an Increased trigger is dispatched over a prior {string} state of {int} events",
  function (owner: string, n: number) {
    dispatchPm(
      B.pmTrigger("counter", ["test.counter.Increased"], B.pmStateIn(owner, n)),
    );
  },
);

When(
  "a rejection of Reserve issued by {string} is dispatched",
  function (issuer: string) {
    dispatchPm(B.pmIssuedRejection("test.counter.Reserve", issuer));
  },
);

When("a request with no trigger is dispatched", function () {
  dispatchPm(B.pmNoTrigger());
});

When("a trigger with no pages is dispatched", function () {
  dispatchPm(B.pmEmptyTrigger());
});

Then(
  "the process-manager emits one command to {string}",
  function (target: string) {
    assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
    assert.equal(ctx.pmResp!.commands.length, 1, "emitted commands");
    assert.equal(
      ctx.pmResp!.commands[0].cover?.domain,
      target,
      "command target",
    );
  },
);

Then("the process-manager emits no commands", function () {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  assert.equal(ctx.pmResp!.commands.length, 0, "expected no commands");
});

Then(
  "the process-manager rebuilt {int} prior state events",
  function (n: number) {
    assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
    assert.equal(ctx.pmResp!.facts.length, n, "rebuilt prior state events");
  },
);

/** Counts the PM response's facts emitted by the audit PM (cover domain
 * AUDIT_MARK) when `audit`, else those emitted by the order PM. */
function factsMarked(audit: boolean): number {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  return ctx.pmResp!.facts.filter(
    (f) => (f.cover?.domain === AUDIT_MARK) === audit,
  ).length;
}

Then(
  "the order process-manager rebuilt {int} prior state events",
  function (n: number) {
    assert.equal(factsMarked(false), n, "order PM facts = its rebuilt events");
  },
);

Then(
  "the audit process-manager rebuilt {int} prior state events",
  function (n: number) {
    assert.equal(factsMarked(true), n, "audit PM facts = its rebuilt events");
  },
);

Then("the order process-manager did not react", function () {
  assert.equal(factsMarked(false), 0, "the order PM emitted no fact");
  assert.equal(
    ctx.pmResp!.commands.length,
    0,
    "the order PM emitted no command",
  );
});

Then("only the audit process-manager compensates", function () {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  const resp = ctx.pmResp!;
  assert.equal(resp.processEvents.length, 1, "exactly one compensation");
  assert.equal(
    resp.processEvents[0].cover?.domain,
    AUDIT_MARK,
    "the audit PM compensated",
  );
  assert.equal(resp.notification, undefined, "the order PM did not escalate");
});

Then("the process-manager emits one process event", function () {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  assert.equal(ctx.pmResp!.processEvents.length, 1, "process events");
});

Then("the process event is addressed to {string}", function (domain: string) {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  const events = ctx.pmResp!.processEvents;
  assert.equal(events.length, 1, "exactly one process event");
  assert.equal(events[0].cover?.domain, domain);
});

Then("the process-manager escalates", function () {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  assert.ok(ctx.pmResp!.notification, "expected an escalation");
});

// --- shared (saga + pm step text collides) ----------------------------------

When(
  "a rejection of Reserve with code {string} and message {string} is dispatched",
  function (code: string, message: string) {
    dispatchPm(B.pmRejection("test.counter.Reserve", code, message));
  },
);

Then(
  "the process-manager compensator saw code {string} and message {string}",
  function (code: string, message: string) {
    assert.equal(ctx.err, undefined, `dispatch failed: ${ctx.err}`);
    assert.deepEqual(ctx.pmSeen, [[code, message]]);
  },
);

When("a rejection of {word} is dispatched", function (cmd: string) {
  const fq = `test.counter.${cmd}`;
  if (ctx.kind === "saga") {
    dispatchSaga(B.sagaRejectionSource(fq));
  } else {
    dispatchPm(B.pmRejection(fq));
  }
});

Then(
  "the command is deferred from source sequence {int} at command index {int}",
  function (seq: number, index: number) {
    assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
    const commands: CommandBook[] =
      ctx.kind === "saga" ? ctx.sagaResp!.commands : ctx.pmResp!.commands;
    const sourceDomain = ctx.kind === "saga" ? "order" : "counter";
    for (const page of commands[0].pages) {
      const st = page.header?.sequenceType;
      assert.equal(st?.case, "angzarrDeferred", "command page is deferred");
      const d = st!.value as AngzarrDeferredSequence;
      assert.equal(d.sourceSeq, seq, "source_seq is the trigger's");
      assert.equal(
        d.commandIndex,
        index,
        "command_index is the emission position",
      );
      assert.equal(
        d.source?.domain,
        sourceDomain,
        "source cover is the trigger book's",
      );
    }
  },
);

Then("the command leaves its source component to the coordinator", function () {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  const commands: CommandBook[] =
    ctx.kind === "saga" ? ctx.sagaResp!.commands : ctx.pmResp!.commands;
  assert.ok(commands.length > 0, "a command was emitted");
  assert.ok(commands[0].pages.length > 0, "the command has pages");
  for (const page of commands[0].pages) {
    const st = page.header?.sequenceType;
    assert.equal(st?.case, "angzarrDeferred", "command page is deferred");
    assert.equal(
      (st!.value as AngzarrDeferredSequence).sourceComponent,
      "",
      "the coordinator stamps the component",
    );
  }
});

Then("the dispatch fails with {word}", failsWith);
