import assert from "node:assert/strict";

import { equals } from "@bufbuild/protobuf";
import { AnySchema } from "@bufbuild/protobuf/wkt";
import { After, Before, Given, Then, When } from "@cucumber/cucumber";

import { CodedError, Router } from "@angzarr/router";
import type {
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
  registerCounterAggregate,
  registerCounterProjector,
  registerOrderProcessManager,
  registerOrderSaga,
} from "../gen/test/counter/counter_angzarr";
import * as B from "./builders";
import {
  CounterFixture,
  type Observation,
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
}

let ctx: Ctx;

Before(() => {
  ctx = { observed: [] };
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
  registerOrderSaga(ctx.router, new SagaFixture());
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
  "an Increased event is dispatched with destination inventory sequence {int}",
  function (seq: number) {
    dispatchSaga(
      B.sagaEventSource("test.counter.Increased", { inventory: seq }),
    );
  },
);

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

Then("the saga emits no commands", function () {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  assert.equal(ctx.sagaResp!.commands.length, 0, "expected no commands");
});

Then("the saga injects one fact event", function () {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  assert.equal(ctx.sagaResp!.events.length, 1, "injected events");
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
  registerOrderProcessManager(ctx.router, new PmFixture());
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
  "an Increased trigger in domain {string} is dispatched with destination inventory sequence {int}",
  function (domain: string, seq: number) {
    dispatchPm(
      B.pmTrigger(domain, ["test.counter.Increased"], undefined, {
        inventory: seq,
      }),
    );
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

Then("the process-manager emits one process event", function () {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  assert.equal(ctx.pmResp!.processEvents.length, 1, "process events");
});

Then("the process-manager escalates", function () {
  assert.equal(ctx.err, undefined, "dispatch unexpectedly failed");
  assert.ok(ctx.pmResp!.notification, "expected an escalation");
});

// --- shared (saga + pm step text collides) ----------------------------------

When("a rejection of {word} is dispatched", function (cmd: string) {
  const fq = `test.counter.${cmd}`;
  if (ctx.kind === "saga") {
    dispatchSaga(B.sagaRejectionSource(fq));
  } else {
    dispatchPm(B.pmRejection(fq));
  }
});

Then("the command carries destination sequence {int}", function (seq: number) {
  const commands: CommandBook[] | undefined =
    ctx.sagaResp?.commands ?? ctx.pmResp?.commands;
  assert.equal(sequenceOf(commands![0].pages[0].header), seq);
});

Then("the dispatch fails with {word}", failsWith);
