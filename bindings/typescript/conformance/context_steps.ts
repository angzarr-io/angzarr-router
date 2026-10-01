import assert from "node:assert/strict";

import { fromBinary } from "@bufbuild/protobuf";
import { After, Before, Given, Then, When } from "@cucumber/cucumber";

import {
  type Cover,
  type EventBook,
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
  pages: [Uint8Array, number][];
  facts?: EventBook;
  replayed?: ReplayResponse;
  pm?: ProcessManagerHandleResponse;
}

let xctx: ContextCtx;

Before(() => {
  xctx = { covers: [], pages: [] };
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
  xctx.router.registerAggregate(ledgerAggregate(xctx.covers));
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
  xctx.facts = router().dispatchFact(B.factRequest("Reserve", 1, 0));
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
  "{int} facts are recorded, each annotated with a count of {int}",
  function (facts: number, count: number) {
    const book = xctx.facts!;
    assert.equal(book.pages.length, facts, "recorded facts");
    for (const page of book.pages) {
      const any = eventOf(page);
      assert.equal(fqOf(any.typeUrl), "test.counter.CounterState");
      assert.equal(fromBinary(CounterStateSchema, any.value).count, count);
    }
  },
);

Then("the fact is recorded unchanged", function () {
  const book = xctx.facts!;
  assert.equal(book.pages.length, 1, "one fact recorded");
  assert.equal(
    fqOf(eventOf(book.pages[0]).typeUrl),
    "test.counter.Reserve",
    "the fact's own type",
  );
});

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
