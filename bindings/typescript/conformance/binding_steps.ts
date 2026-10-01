import assert from "node:assert/strict";

import { create } from "@bufbuild/protobuf";
import { AnySchema } from "@bufbuild/protobuf/wkt";
import { After, Before, Given, Then, When } from "@cucumber/cucumber";

import {
  AggregateDispatch,
  type BusinessResponse,
  BusinessResponseSchema,
  CodedError,
  EventBookSchema,
  EventPageSchema,
  GrpcCode,
  Rebuilder,
  Router,
} from "@angzarr/router";
import {
  type CounterState,
  CounterStateSchema,
} from "../gen/test/counter/counter_pb";
import { hostCallback } from "../src/ffi";
import { fromStatusBytes, type Outcome } from "../src/statuses";
import * as B from "./builders";

// Binding-local scenarios (bindings/typescript/conformance/features): behavior
// of the TypeScript binding's own surface that the shared suite cannot express.
interface BindingCtx {
  router?: Router;
  resp?: BusinessResponse;
  err?: unknown;
  calls: string[];
}

let bctx: BindingCtx;

Before({ tags: "@binding" }, () => {
  bctx = { calls: [] };
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
