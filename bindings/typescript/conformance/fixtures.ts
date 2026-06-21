import { create } from "@bufbuild/protobuf";
import { AnySchema } from "@bufbuild/protobuf/wkt";

import {
  type BusinessResponse,
  BusinessResponseSchema,
  type CommandContext,
  CoverSchema,
  type Destinations,
  type EventBook,
  EventBookSchema,
  EventPageSchema,
  type Notification,
  NotificationSchema,
  type PmRejection,
  type ProcessManagerHandleResponse,
  ProcessManagerHandleResponseSchema,
  type Projection,
  ProjectionSchema,
  type RejectionNotification,
  type SagaEmission,
  reject,
} from "@angzarr/router";
import { type CounterAggregateHandler } from "../gen/test/counter/counter_aggregate_angzarr";
import { type CounterProjectorHandler } from "../gen/test/counter/counter_projector_angzarr";
import { type OrderProcessManagerHandler } from "../gen/test/counter/order_process_manager_angzarr";
import { type OrderSagaHandler } from "../gen/test/counter/order_saga_angzarr";
import {
  type CounterProjectorState,
  type CounterState,
  type FailHard,
  type IncreaseBy,
  type Increased,
  IncreasedSchema,
  type OrderProcessManagerState,
} from "../gen/test/counter/counter_pb";
import { oneFact, reserveCommand } from "./builders";

/** The historical-state evidence a command handler saw — what the suite
 * asserts, since state never crosses the boundary. */
export interface Observation {
  hadPriorEvents: boolean;
  nextSequence: number;
  count: number;
}

/** The conformance CounterAggregate fixture, implementing the generated seam. */
export class CounterFixture implements CounterAggregateHandler {
  constructor(private readonly observed: Observation[]) {}

  increaseBy(
    cmd: IncreaseBy,
    state: CounterState,
    cctx: CommandContext,
  ): Increased[] {
    this.observed.push({
      hadPriorEvents: cctx.hadPriorEvents,
      nextSequence: cctx.nextSequence,
      count: state.count,
    });
    if (cmd.n === 0) {
      throw reject("VALUE_NOT_POSITIVE", "increase amount must be positive");
    }
    return Array.from({ length: cmd.n }, () => create(IncreasedSchema));
  }

  failHard(
    _cmd: FailHard,
    _state: CounterState,
    _cctx: CommandContext,
  ): EventBook {
    throw new Error("hard failure");
  }

  applyIncreased(state: CounterState, _ev: Increased): void {
    state.count += 1;
  }

  /** Appends both ordered markers in one response — the within-component
   * fan-out collapses to one compensator, preserving the observable
   * two-marker ordering the feature asserts. */
  onReserveRejected(
    _n: Notification,
    _rejection: RejectionNotification,
    _state: CounterState,
    _cctx: CommandContext,
  ): BusinessResponse {
    return create(BusinessResponseSchema, {
      result: {
        case: "events",
        value: create(EventBookSchema, {
          pages: [marker("CompensatedFirst"), marker("CompensatedSecond")],
        }),
      },
    });
  }
}

function marker(name: string) {
  return create(EventPageSchema, {
    payload: {
      case: "event",
      value: create(AnySchema, { typeUrl: `/test.counter.${name}` }),
    },
  });
}

/** The conformance OrderSaga fixture: a declared source event emits a Reserve
 * command stamped with the supplied destination sequence; a rejection injects
 * one fact event. */
export class SagaFixture implements OrderSagaHandler {
  increased(_ev: Increased, dests: Destinations): SagaEmission {
    let cmd = reserveCommand();
    if (dests.has("inventory")) {
      cmd = dests.stampCommand(cmd, "inventory");
    }
    return { commands: [cmd], events: [] };
  }

  onReserveRejected(
    _n: Notification,
    _rejection: RejectionNotification,
  ): EventBook[] {
    return [oneFact()];
  }
}

/** The conformance CounterProjector fixture: every delivered event folds into
 * one projection; the finisher carries the cover and folded count. */
export class ProjectorFixture implements CounterProjectorHandler {
  increased(projection: CounterProjectorState, _ev: Increased): void {
    projection.count += 1;
  }

  finish(projection: CounterProjectorState, events: EventBook): Projection {
    return create(ProjectionSchema, {
      cover: events.cover,
      projector: "counter-projector",
      sequence: projection.count,
    });
  }
}

/** The conformance OrderProcessManager fixture: the newest trigger reacts with a
 * stamped Reserve command plus one fact per rebuilt prior-state event; a
 * rejection injects one process event and escalates. */
export class PmFixture implements OrderProcessManagerHandler {
  increased(
    _ev: Increased,
    state: OrderProcessManagerState,
    dests: Destinations,
  ): ProcessManagerHandleResponse {
    let cmd = reserveCommand();
    if (dests.has("inventory")) {
      cmd = dests.stampCommand(cmd, "inventory");
    }
    return create(ProcessManagerHandleResponseSchema, {
      commands: [cmd],
      facts: Array.from({ length: state.count }, () => oneFact()),
    });
  }

  applyIncreased(state: OrderProcessManagerState, _ev: Increased): void {
    state.count += 1;
  }

  onReserveRejected(
    _n: Notification,
    _rejection: RejectionNotification,
    _state: OrderProcessManagerState,
  ): PmRejection {
    return {
      processEvents: [oneFact()],
      escalation: create(NotificationSchema, {
        cover: create(CoverSchema, { domain: "escalated" }),
      }),
    };
  }
}
