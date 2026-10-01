import assert from "node:assert/strict";

import { After, Before, Given, Then, When } from "@cucumber/cucumber";

import {
  type AggregateDispatch,
  type BusinessResponse,
  CodedError,
  type ContextualCommand,
  type EventPage,
  GrpcCode,
  Router,
} from "@angzarr/router";
import * as B from "./builders";
import { inventoryAggregate, paymentAggregate } from "./hand_built";

// compensation.feature: rejection and Compensate routing through aggregates
// built with the binding's hand-written aggregate API.
interface CompensationCtx {
  router?: Router;
  resp?: BusinessResponse;
  err?: unknown;
}

let cctx: CompensationCtx;

Before(() => {
  cctx = {};
});

After(() => {
  cctx.router?.close();
});

function build(agg: AggregateDispatch<object>): void {
  cctx.router ??= new Router();
  cctx.router.registerAggregate(agg);
}

function dispatch(cc: ContextualCommand): void {
  try {
    cctx.resp = cctx.router!.dispatch(cc);
    cctx.err = undefined;
  } catch (e) {
    cctx.err = e;
    cctx.resp = undefined;
  }
}

function pages(): EventPage[] {
  assert.equal(cctx.err, undefined, `dispatch failed: ${cctx.err}`);
  const r = cctx.resp!;
  if (r.result.case === undefined) {
    return [];
  }
  assert.equal(r.result.case, "events", "the response carries events");
  return r.result.case === "events" ? r.result.value.pages : [];
}

function fqOf(page: EventPage): string {
  assert.equal(page.payload.case, "event", "the page carries an event");
  const url = page.payload.case === "event" ? page.payload.value.typeUrl : "";
  return url.slice(url.lastIndexOf("/") + 1);
}

Given(
  "a payment aggregate compensating Reserve from any domain with {word}",
  function (event: string) {
    build(paymentAggregate([["test.counter.Reserve", event]]));
  },
);

Given(
  "a second payment aggregate compensating Reserve from any domain with {word}",
  function (event: string) {
    build(paymentAggregate([["test.counter.Reserve", event]]));
  },
);

Given(
  "a payment aggregate compensating Reserve from {string} with {word} and from {string} with {word}",
  function (d1: string, e1: string, d2: string, e2: string) {
    build(
      paymentAggregate([
        [`${d1}:test.counter.Reserve`, e1],
        [`${d2}:test.counter.Reserve`, e2],
      ]),
    );
  },
);

Given(
  "an inventory aggregate undoing AdjustStock with StockAdjustmentReverted and Reserve with StockReleased",
  function () {
    build(inventoryAggregate());
  },
);

When(
  "a rejection of {word} sent to {string} is dispatched to the payment aggregate",
  function (command: string, domain: string) {
    dispatch(B.rejectionSentTo(command, domain));
  },
);

When(
  "a rejection of {word} sent to {string} is dispatched to the payment aggregate over history ending at sequence {int}",
  function (command: string, domain: string, last: number) {
    dispatch(B.rejectionSentTo(command, domain, last + 1));
  },
);

When(
  "a Compensate for {word} is dispatched to the inventory aggregate",
  function (command: string) {
    dispatch(B.compensateFor(command));
  },
);

Then("the aggregate emits one {word} event", function (event: string) {
  const got = pages();
  assert.equal(got.length, 1, "exactly one event");
  assert.equal(fqOf(got[0]), `test.counter.${event}`);
});

Then(
  "the aggregates emit {word} at sequence {int} then {word} at sequence {int}",
  function (
    first: string,
    firstSeq: number,
    second: string,
    secondSeq: number,
  ) {
    const got = pages().map((page) => {
      assert.equal(
        page.header?.sequenceType.case,
        "sequence",
        "explicit sequence",
      );
      return [fqOf(page), page.header?.sequenceType.value];
    });
    assert.deepEqual(got, [
      [`test.counter.${first}`, firstSeq],
      [`test.counter.${second}`, secondSeq],
    ]);
  },
);

Then("the aggregate emits nothing", function () {
  assert.equal(pages().length, 0, "no events");
});

Then("the emitted event takes sequence {int}", function (seq: number) {
  const header = pages()[0].header;
  assert.equal(header?.sequenceType.case, "sequence", "explicit sequence");
  assert.equal(header?.sequenceType.value, seq);
});

Then(
  "the dispatch fails with {word} as UNIMPLEMENTED",
  function (code: string) {
    const err = cctx.err;
    assert.ok(err instanceof CodedError, `expected ${code}, got ${err}`);
    assert.equal(err.code, code);
    assert.equal(err.grpc, GrpcCode.Unimplemented);
  },
);
