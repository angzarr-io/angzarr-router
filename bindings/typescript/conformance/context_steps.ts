import assert from "node:assert/strict";

import { fromBinary } from "@bufbuild/protobuf";
import { After, Before, Given, Then, When } from "@cucumber/cucumber";

import {
  CodedError,
  type Cover,
  type EventBook,
  GrpcCode,
  type EventPage,
  type ProcessManagerHandleResponse,
  type ReplayResponse,
  Router,
} from "@angzarr/router";
import { CounterStateSchema } from "../gen/test/counter/counter_pb";
import * as B from "./builders";
import { ledgerAggregate, reservingPm, trackingProjector } from "./hand_built";

// context.feature: facts, replay, cover access, PM compensator commands and
// projector page context through the binding's hand-written APIs.
interface ContextCtx {
  router?: Router;
  covers: (Cover | undefined)[];
  applied: number[];
  pages: [Uint8Array, number][];
  facts?: EventBook;
  refusal?: unknown;
  replayed?: ReplayResponse;
  pm?: ProcessManagerHandleResponse;
}

let xctx: ContextCtx;

Before(() => {
  xctx = { covers: [], applied: [], pages: [] };
});

After(() => {
  xctx.router?.close();
});

function router(): Router {
  assert.ok(xctx.router, "a component was built");
  return xctx.router;
}

Given("a ledger aggregate", function () {
  xctx.router = new Router();
  xctx.router.registerAggregate(ledgerAggregate(xctx.covers, xctx.applied));
});

Given("a reserving process-manager", function () {
  xctx.router = new Router();
  xctx.router.registerProcessManager(reservingPm(xctx.covers));
});

Given("a tracking projector", function () {
  xctx.router = new Router();
  xctx.router.registerProjector(trackingProjector(xctx.pages));
});

When(
  "{int} Increased facts are handled over {int} prior Increased events",
  function (facts: number, prior: number) {
    xctx.facts = router().dispatchFact(
      B.factRequest("Increased", facts, prior),
    );
  },
);

When("a Reserve fact is handled over no prior events", function () {
  try {
    xctx.facts = router().dispatchFact(B.factRequest("Reserve", 1, 0));
  } catch (e) {
    xctx.refusal = e;
  }
});

When(
  "the ledger replays a snapshot of {int} then {int} Increased events",
  function (count: number, events: number) {
    xctx.replayed = router().dispatchReplay(
      "ledger",
      B.replayRequest(count, events),
    );
  },
);

When(
  "the reserving process-manager replays {int} Increased events",
  function (events: number) {
    xctx.replayed = router().dispatchReplay(
      "reserving-pm",
      B.eventsReplayRequest(events),
    );
  },
);

Then(
  "the ledger applied Increased events at sequences {int} and {int}",
  function (first: number, second: number) {
    assert.deepEqual(xctx.applied, [first, second]);
  },
);

When(
  "an IncreaseBy command for ledger root {string} is dispatched",
  function (label: string) {
    router().dispatch(B.ledgerCommand(label));
  },
);

When(
  "an Increased trigger of counter root {string} at sequence {int} is dispatched to the reserving process-manager",
  function (label: string, seq: number) {
    xctx.pm = router().dispatchProcessManager(B.reservingTrigger(label, seq));
  },
);

When(
  "a rejection of Reserve sent to {string} at sequence {int} is dispatched to the reserving process-manager",
  function (domain: string, seq: number) {
    xctx.pm = router().dispatchProcessManager(
      B.reservingRejection(domain, seq),
    );
  },
);

When(
  "Increased events of counter root {string} at sequences {int} and {int} are projected",
  function (label: string, first: number, second: number) {
    router().dispatchProjector(B.trackedBook(label, [first, second]));
  },
);

function eventOf(page: EventPage) {
  assert.equal(page.payload.case, "event", "the fact page carries an event");
  return page.payload.value!;
}

function fqOf(url: string): string {
  return url.slice(url.lastIndexOf("/") + 1);
}

Then(
  "each Increased fact is recorded, flagged by the counts {int} and {int}",
  function (first: number, second: number) {
    const recorded = xctx.facts!.pages.map((page) => {
      const any = eventOf(page);
      const name = fqOf(any.typeUrl);
      return name === "test.counter.CounterState"
        ? `${name}(${fromBinary(CounterStateSchema, any.value).count})`
        : name;
    });
    assert.deepEqual(recorded, [
      "test.counter.Increased",
      `test.counter.CounterState(${first})`,
      "test.counter.Increased",
      `test.counter.CounterState(${second})`,
    ]);
  },
);

Then(
  "the facts are refused with {word} as INVALID_ARGUMENT",
  function (code: string) {
    const err = xctx.refusal;
    assert.equal(xctx.facts, undefined, "nothing was recorded");
    assert.ok(err instanceof CodedError, `expected ${code}, got ${err}`);
    assert.equal(err.code, code, "coded reason");
    assert.equal(err.grpc, GrpcCode.InvalidArgument, "gRPC code");
  },
);

Then("the replayed state has a count of {int}", function (count: number) {
  const state = xctx.replayed?.state;
  assert.ok(state, "the replay returned a state");
  assert.equal(fqOf(state.typeUrl), "test.counter.CounterState");
  assert.equal(fromBinary(CounterStateSchema, state.value).count, count);
});

function singleRoot(): Uint8Array {
  assert.equal(xctx.covers.length, 1, "the handler ran once");
  const root = xctx.covers[0]?.root?.value;
  assert.ok(root, "the handler saw a cover with a root");
  return root;
}

Then("the ledger handler saw root {string}", function (label: string) {
  assert.deepEqual(singleRoot(), B.rootOf(label));
});

Then(
  "the reserving process-manager saw trigger root {string}",
  function (label: string) {
    assert.deepEqual(singleRoot(), B.rootOf(label));
  },
);

Then(
  "the reserving process-manager emits one Release command deferred from source sequence {int}",
  function (seq: number) {
    const resp = xctx.pm!;
    assert.equal(resp.commands.length, 1, "one command");
    const page = resp.commands[0].pages[0];
    assert.equal(page.payload.case, "command", "the page carries a command");
    assert.equal(
      fqOf(page.payload.case === "command" ? page.payload.value.typeUrl : ""),
      "test.counter.Release",
    );
    const st = page.header?.sequenceType;
    assert.equal(
      st?.case,
      "angzarrDeferred",
      "the Release command is deferred",
    );
    assert.equal(st?.case === "angzarrDeferred" ? st.value.sourceSeq : -1, seq);
  },
);

Then(
  "the projector saw root {string} at sequences {int} and {int}",
  function (label: string, first: number, second: number) {
    const root = B.rootOf(label);
    assert.deepEqual(xctx.pages, [
      [root, first],
      [root, second],
    ]);
  },
);
