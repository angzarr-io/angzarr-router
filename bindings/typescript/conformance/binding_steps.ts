import assert from "node:assert/strict";

import { create, fromBinary } from "@bufbuild/protobuf";
import { AnySchema } from "@bufbuild/protobuf/wkt";
import { After, Before, Given, Then, When } from "@cucumber/cucumber";

import {
  AggregateDispatch,
  type BusinessResponse,
  BusinessResponseSchema,
  CodedError,
  type Cover,
  type Destinations,
  type EventBook,
  EventBookSchema,
  EventPageSchema,
  FactRecord,
  type FactThunk,
  GrpcCode,
  ProcessManagerDispatch,
  ProcessManagerHandleResponseSchema,
  Rebuilder,
  type ReplayResponse,
  Router,
  SagaDispatch,
} from "@angzarr/router";
import { newCounterAggregateDispatch } from "../gen/test/counter/counter_aggregate_angzarr";
import {
  type CounterState,
  CounterStateSchema,
} from "../gen/test/counter/counter_pb";
import { checkAbiVersion, hostCallback } from "../src/ffi";
import { CounterFixture } from "./fixtures";
import { fromStatusBytes, type Outcome } from "../src/statuses";
import * as B from "./builders";

// Binding-local scenarios (bindings/typescript/conformance/features): behavior
// of the TypeScript binding's own surface that the shared suite cannot express.
interface BindingCtx {
  router?: Router;
  resp?: BusinessResponse;
  err?: unknown;
  calls: string[];
  dests?: Destinations;
  sourceCover?: Cover;
  covers: (Cover | undefined)[];
  facts?: EventBook;
  replayed?: ReplayResponse;
}

let bctx: BindingCtx;

Before({ tags: "@binding" }, () => {
  bctx = { calls: [], covers: [] };
});

After({ tags: "@binding" }, () => {
  bctx.router?.close();
});

function counterDispatch(): AggregateDispatch<CounterState> {
  return new AggregateDispatch<CounterState>(
    "CounterAggregate",
    "counter",
    new Rebuilder(() => create(CounterStateSchema)),
  );
}

function markerPage(name: string) {
  return create(EventPageSchema, {
    payload: {
      case: "event",
      value: create(AnySchema, { typeUrl: `/test.counter.${name}` }),
    },
  });
}

function capture(run: () => BusinessResponse): void {
  try {
    bctx.resp = run();
    bctx.err = undefined;
  } catch (e) {
    bctx.err = e;
    bctx.resp = undefined;
  }
}

Given("a binding router with a counter aggregate", function () {
  bctx.router = new Router();
  bctx.router.registerAggregate(
    counterDispatch().onCommand("test.counter.IncreaseBy", () =>
      create(EventBookSchema, { pages: [markerPage("Increased")] }),
    ),
  );
});

// --- router lifecycle -------------------------------------------------------

function assertCoded(err: unknown, code: string, grpc: GrpcCode): void {
  assert.ok(err instanceof CodedError, `expected a CodedError, got ${err}`);
  assert.equal(err.code, code, "coded reason");
  assert.equal(err.grpc, grpc, "gRPC code");
}

When("the router is closed", function () {
  bctx.router!.close();
});

When("the router is closed twice", function () {
  bctx.router!.close();
  bctx.router!.close();
});

Then(
  "dispatching through the router fails with {word}",
  function (code: string) {
    capture(() => bctx.router!.dispatch(B.increaseCommand(1)));
    assertCoded(bctx.err, code, GrpcCode.FailedPrecondition);
  },
);

Then("registering on the router fails with {word}", function (code: string) {
  let err: unknown;
  try {
    bctx.router!.registerAggregate(counterDispatch());
  } catch (e) {
    err = e;
  }
  assertCoded(err, code, GrpcCode.FailedPrecondition);
});

// --- coded error with gRPC OK -----------------------------------------------

Given(
  "a binding counter aggregate whose handler throws {word} with gRPC code {int}",
  function (code: string, grpc: number) {
    bctx.router = new Router();
    bctx.router.registerAggregate(
      counterDispatch().onCommand("test.counter.IncreaseBy", () => {
        throw new CodedError(code, "handler refused", grpc as GrpcCode);
      }),
    );
  },
);

When("a command is dispatched through the binding router", function () {
  capture(() => bctx.router!.dispatch(B.increaseCommand(1)));
});

Then(
  "the binding dispatch fails with {word} as INVALID_ARGUMENT",
  function (code: string) {
    assertCoded(bctx.err, code, GrpcCode.InvalidArgument);
  },
);

// --- ordered compensator fan-out --------------------------------------------

Given("a binding counter aggregate with two Reserve compensators", function () {
  bctx.router = new Router();
  const compensator = (name: string) => (): BusinessResponse => {
    bctx.calls.push(name);
    return create(BusinessResponseSchema, {
      result: {
        case: "events",
        value: create(EventBookSchema, { pages: [markerPage(name)] }),
      },
    });
  };
  bctx.router.registerAggregate(
    counterDispatch()
      .onRejected("test.counter.Reserve", compensator("CompensatedFirst"))
      .onRejected("test.counter.Reserve", compensator("CompensatedSecond")),
  );
});

When(
  "a Reserve rejection is dispatched through the binding router",
  function () {
    capture(() =>
      bctx.router!.dispatch(B.rejectionCommand("test.counter.Reserve")),
    );
  },
);

Then("the compensators ran first then second", function () {
  assert.equal(bctx.err, undefined, `dispatch failed: ${bctx.err}`);
  assert.deepEqual(bctx.calls, ["CompensatedFirst", "CompensatedSecond"]);
});

Then(
  "the compensation merged {int} events, the first from the first compensator",
  function (n: number) {
    const r = bctx.resp!;
    assert.equal(r.result.case, "events", "compensation recorded events");
    const pages = r.result.case === "events" ? r.result.value.pages : [];
    assert.equal(pages.length, n, "merged compensation events");
    const urls = pages.map((p) =>
      p.payload.case === "event" ? p.payload.value.typeUrl : "",
    );
    assert.deepEqual(urls, [
      "/test.counter.CompensatedFirst",
      "/test.counter.CompensatedSecond",
    ]);
  },
);

// --- callback trampoline ----------------------------------------------------

let callbackOutcome: Outcome | undefined;

When("the host callback runs for an unregistered host context", function () {
  callbackOutcome = hostCallback(
    0n,
    1n,
    "/test.counter.Increased",
    new Uint8Array(0),
    new Uint8Array(0),
  );
});

Then("the callback fails INTERNAL with {word}", function (code: string) {
  const outcome = callbackOutcome!;
  assert.equal(outcome.status, -GrpcCode.Internal, "callback status");
  assert.ok(
    outcome.response && outcome.response.length > 0,
    "callback carried a Status payload",
  );
  const err = fromStatusBytes(outcome.response, outcome.status);
  assert.equal(err.code, code, "Status ErrorInfo reason");
  assert.equal(err.grpc, GrpcCode.Internal, "Status code");
});

// --- ABI version ------------------------------------------------------------

let abiFailure: unknown;

Then("the router reports ABI version {int}", function (version: number) {
  assert.equal(Router.abiVersion(), version);
});

When(
  "the binding checks a library reporting ABI version {int}",
  function (actual: number) {
    abiFailure = undefined;
    try {
      checkAbiVersion(actual);
    } catch (e) {
      abiFailure = e;
    }
  },
);

Then(
  "the check fails naming expected version {int} and actual version {int}",
  function (expected: number, actual: number) {
    assert.ok(abiFailure instanceof Error, "the drifted library was refused");
    assert.match(abiFailure.message, new RegExp(`expected ${expected}\\b`));
    assert.match(abiFailure.message, new RegExp(`actual ${actual}\\b`));
  },
);

// --- declared-output destinations -------------------------------------------

function captureDests(dests: Destinations): void {
  bctx.dests = dests;
}

Given(
  "a binding saga targeting {string} and {string}",
  function (first: string, second: string) {
    bctx.router = new Router();
    bctx.router.registerSaga(
      new SagaDispatch("Binding", "order", [first, second]).onEvent(
        "test.counter.Increased",
        (_event, dests, sourceCover) => {
          captureDests(dests);
          bctx.sourceCover = sourceCover;
          return { commands: [], events: [] };
        },
      ),
    );
  },
);

function bindingPm(targets?: string[]): ProcessManagerDispatch<CounterState> {
  return new ProcessManagerDispatch<CounterState>(
    "Binding",
    "binding-pm",
    new Rebuilder(() => create(CounterStateSchema)),
    targets,
  ).onEvent("counter", "test.counter.Increased", (_event, _state, dests) => {
    captureDests(dests);
    return create(ProcessManagerHandleResponseSchema);
  });
}

Given(
  "a binding process-manager targeting {string}",
  function (target: string) {
    bctx.router = new Router();
    bctx.router.registerProcessManager(bindingPm([target]));
  },
);

Given("a binding process-manager with no targets", function () {
  bctx.router = new Router();
  bctx.router.registerProcessManager(bindingPm());
});

When("an order event is dispatched to the binding saga", function () {
  bctx.router!.dispatchSaga(B.sagaEventSource("test.counter.Increased", 1));
});

When(
  "a counter trigger is dispatched to the binding process-manager",
  function () {
    bctx.router!.dispatchProcessManager(
      B.pmTrigger("counter", ["test.counter.Increased"], undefined, 1),
    );
  },
);

Then(
  "the handler's destinations are {string} and {string}",
  function (first: string, second: string) {
    assert.deepEqual(bctx.dests!.domains(), [first, second]);
    assert.ok(bctx.dests!.has(first) && bctx.dests!.has(second));
  },
);

Then(
  "the saga handler saw the source cover of {string}",
  function (domain: string) {
    assert.equal(bctx.sourceCover?.domain, domain);
  },
);

Then("the handler's destinations are {string}", function (only: string) {
  assert.deepEqual(bctx.dests!.domains(), [only]);
  assert.ok(bctx.dests!.has(only));
});

Then(
  "the handler's destinations do not include {string}",
  function (domain: string) {
    assert.equal(bctx.dests!.has(domain), false);
  },
);

Then("the handler has no destinations", function () {
  assert.ok(bctx.dests, "the handler ran");
  assert.deepEqual(bctx.dests.domains(), []);
  assert.equal(bctx.dests.has("inventory"), false);
});

// --- handler results ----------------------------------------------------------

Given(
  "a binding inventory aggregate whose AdjustStock undo returns nothing",
  function () {
    bctx.router = new Router();
    bctx.router.registerAggregate(
      new AggregateDispatch<object>(
        "Inventory",
        "inventory",
        new Rebuilder(() => ({})),
      ).onUndo("test.counter.AdjustStock", (_n, compensate, _s, cctx) => {
        bctx.calls.push(compensate.commandType);
        bctx.covers.push(cctx.cover);
        return undefined;
      }),
    );
  },
);

When(
  "a Compensate for AdjustStock is dispatched through the binding router",
  function () {
    capture(() => bctx.router!.dispatch(B.compensateFor("AdjustStock")));
  },
);

Then("the binding dispatch records no events", function () {
  assert.equal(bctx.err, undefined, `dispatch failed: ${bctx.err}`);
  assert.deepEqual(bctx.calls, ["test.counter.AdjustStock"], "undo ran once");
  const r = bctx.resp!;
  const pages = r.result.case === "events" ? r.result.value.pages : [];
  assert.equal(pages.length, 0, "no events recorded");
});

Then("the undo handler saw the {string} cover", function (domain: string) {
  assert.equal(bctx.covers.length, 1, "the undo handler ran once");
  assert.equal(bctx.covers[0]?.domain, domain);
});

const increasedAny = () =>
  create(AnySchema, { typeUrl: "/test.counter.Increased" });

function bindingLedgerWithFactHandler(handler: FactThunk<CounterState>): void {
  bctx.router = new Router();
  bctx.router.registerAggregate(
    new AggregateDispatch<CounterState>(
      "Ledger",
      "ledger",
      new Rebuilder(() => create(CounterStateSchema)),
    ).onFact("test.counter.Increased", (fact, state) => {
      bctx.calls.push(fact.typeUrl);
      return handler(fact, state);
    }),
  );
}

Given(
  "a binding ledger aggregate whose Increased fact handler records it as received",
  function () {
    bindingLedgerWithFactHandler((fact) => FactRecord.asReceived(fact));
  },
);

Given(
  "a binding ledger aggregate whose Increased fact handler flags it with two Increased events",
  function () {
    bindingLedgerWithFactHandler((fact) => ({
      fact,
      flags: [increasedAny(), increasedAny()],
    }));
  },
);

Given(
  "a binding ledger aggregate whose Increased fact handler returns nothing",
  function () {
    bindingLedgerWithFactHandler(() => undefined as unknown as FactRecord);
  },
);

When("an Increased fact is dispatched through the binding router", function () {
  bctx.err = undefined;
  try {
    bctx.facts = bctx.router!.dispatchFact(B.factRequest("Increased", 1, 0));
  } catch (e) {
    bctx.err = e;
  }
});

Then(
  "the binding router records the Increased fact then {int} headerless Increased flags",
  function (flags: number) {
    assert.equal(bctx.err, undefined, `fact dispatch failed: ${bctx.err}`);
    const pages = bctx.facts!.pages;
    assert.equal(pages.length, 1 + flags, "the fact and its flags");
    for (const page of pages) {
      assert.equal(page.payload.case, "event");
      assert.equal(
        page.payload.case === "event" ? page.payload.value.typeUrl : "",
        "/test.counter.Increased",
      );
    }
    for (const flag of pages.slice(1)) {
      assert.equal(flag.header, undefined, "a flag carries no header");
    }
  },
);

Then("the binding fact dispatch fails with {word}", function (code: string) {
  assert.deepEqual(bctx.calls, ["/test.counter.Increased"], "handler ran");
  assertCoded(bctx.err, code, GrpcCode.Internal);
});

Then("the binding router records one Increased fact", function () {
  assert.equal(bctx.err, undefined, `fact dispatch failed: ${bctx.err}`);
  assert.deepEqual(bctx.calls, ["/test.counter.Increased"], "handler ran");
  const pages = bctx.facts!.pages;
  assert.equal(pages.length, 1, "one fact recorded");
  const p = pages[0].payload;
  assert.equal(p.case, "event");
  assert.equal(
    p.case === "event" ? p.value.typeUrl : "",
    "/test.counter.Increased",
  );
});

Given("a binding counter aggregate with no state schema", function () {
  bctx.router = new Router();
  bctx.router.registerAggregate(counterDispatch());
});

Given("a generated counter aggregate", function () {
  bctx.router = new Router();
  bctx.router.registerAggregate(
    newCounterAggregateDispatch(new CounterFixture([])),
  );
});

When(
  "the binding router replays a snapshot of {int} then {int} Increased events",
  function (count: number, events: number) {
    try {
      bctx.replayed = bctx.router!.dispatchReplay(
        "counter",
        B.replayRequest(count, events),
      );
      bctx.err = undefined;
    } catch (e) {
      bctx.err = e;
    }
  },
);

Then("the binding replay yields a counter of {int}", function (n: number) {
  assert.equal(bctx.err, undefined, `replay failed: ${bctx.err}`);
  const state = bctx.replayed!.state!;
  assert.equal(state.typeUrl, "/test.counter.CounterState");
  assert.equal(fromBinary(CounterStateSchema, state.value).count, n);
});

Then("the binding replay fails with {word}", function (code: string) {
  // NO_HANDLER_REGISTERED is UNIMPLEMENTED on the wire.
  assertCoded(bctx.err, code, GrpcCode.Unimplemented);
});
