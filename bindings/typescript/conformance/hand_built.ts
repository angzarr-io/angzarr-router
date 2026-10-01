import { create } from "@bufbuild/protobuf";

import {
  AggregateDispatch,
  type Cover,
  CoverSchema,
  EventBookSchema,
  EventPageSchema,
  Pack,
  ProcessManagerDispatch,
  ProcessManagerHandleResponseSchema,
  ProjectorDispatch,
  Rebuilder,
} from "@angzarr/router";
import {
  type CounterState,
  CounterStateSchema,
  IncreasedSchema,
} from "../gen/test/counter/counter_pb";
import { ledgerLinkage, oneEvent, releaseCommand } from "./builders";

// Components built through the binding's hand-written API (compensation.feature
// and context.feature), mirroring the Rust reference fixtures.

/** The payment aggregate (domain "payment"): one compensator per
 * (compensates entry, emitted event name) pair. */
export function paymentAggregate(
  entries: [key: string, event: string][],
): AggregateDispatch<object> {
  const agg = new AggregateDispatch<object>(
    "Payment",
    "payment",
    new Rebuilder(() => ({})),
  );
  for (const [key, event] of entries) {
    agg.onRejected(key, () => oneEvent(event));
  }
  return agg;
}

/** The inventory aggregate (domain "inventory"): undoes AdjustStock with
 * StockAdjustmentReverted and Reserve with StockReleased. */
export function inventoryAggregate(): AggregateDispatch<object> {
  return new AggregateDispatch<object>(
    "Inventory",
    "inventory",
    new Rebuilder(() => ({})),
  )
    .onUndo("test.counter.AdjustStock", () =>
      oneEvent("StockAdjustmentReverted"),
    )
    .onUndo("test.counter.Reserve", () => oneEvent("StockReleased"));
}

/** The ledger aggregate (domain "ledger") over CounterState: Increased folds
 * count += 1 and records the page sequence it applied; a snapshot loads CounterState; IncreaseBy records the handled
 * cover and emits one Increased whose cover carries the ledger's own linkage
 * ({@link ledgerLinkage}); the only declared fact, Increased, is recorded as
 * received and flagged by a CounterState carrying the count it brings the
 * ledger to. */
export function ledgerAggregate(
  seen: (Cover | undefined)[],
  applied: number[],
): AggregateDispatch<CounterState> {
  const rebuilder = new Rebuilder<CounterState>(
    () => create(CounterStateSchema),
    CounterStateSchema,
  )
    .applyWithContext("test.counter.Increased", (state, _event, ctx) => {
      state.count += 1;
      applied.push(ctx.sequence);
    })
    .withSnapshot((state, any) => Pack.merge(CounterStateSchema, state, any));
  return new AggregateDispatch<CounterState>("Ledger", "ledger", rebuilder)
    .onCommand("test.counter.IncreaseBy", (_cmd, _state, cctx) => {
      seen.push(cctx.cover);
      return create(EventBookSchema, {
        cover: create(CoverSchema, { ext: ledgerLinkage() }),
        pages: [
          create(EventPageSchema, {
            payload: {
              case: "event",
              value: Pack.wrap(IncreasedSchema, create(IncreasedSchema)),
            },
          }),
        ],
      });
    })
    .onFact("test.counter.Increased", (fact, state) => ({
      fact,
      flags: [
        Pack.wrap(
          CounterStateSchema,
          create(CounterStateSchema, { count: state.count + 1 }),
        ),
      ],
    }));
}

/** The reserving process-manager (domain "reserving-pm", target "inventory")
 * over CounterState (Increased folds count += 1): an Increased trigger from "counter" records the trigger
 * cover and emits nothing; a rejected Reserve is compensated with a Release
 * command to "inventory". */
export function reservingPm(
  seen: (Cover | undefined)[],
): ProcessManagerDispatch<CounterState> {
  return new ProcessManagerDispatch<CounterState>(
    "Reserving",
    "reserving-pm",
    new Rebuilder<CounterState>(
      () => create(CounterStateSchema),
      CounterStateSchema,
    ).apply("test.counter.Increased", (state) => {
      state.count += 1;
    }),
    ["inventory"],
  )
    .onEvent(
      "counter",
      "test.counter.Increased",
      (_event, _state, _dests, triggerCover) => {
        seen.push(triggerCover);
        return create(ProcessManagerHandleResponseSchema);
      },
    )
    .onRejected("test.counter.Reserve", () =>
      create(ProcessManagerHandleResponseSchema, {
        commands: [releaseCommand()],
      }),
    );
}

/** The tracking projector: every Increased fold records its book's root and
 * the page's sequence. */
export function trackingProjector(
  seen: [root: Uint8Array, sequence: number][],
): ProjectorDispatch<object> {
  return new ProjectorDispatch<object>("Tracker", () => ({})).onEvent(
    "test.counter.Increased",
    (_projection, _event, ctx) => {
      seen.push([ctx.cover?.root?.value ?? new Uint8Array(0), ctx.sequence]);
    },
  );
}
