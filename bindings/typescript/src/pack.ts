import {
  create,
  fromBinary,
  toBinary,
  type DescMessage,
  type MessageShape,
} from "@bufbuild/protobuf";
import { type Any, AnySchema } from "@bufbuild/protobuf/wkt";

import {
  type EventBook,
  EventBookSchema,
  EventPageSchema,
} from "../gen/io/angzarr/v1/types_pb";

// The framework's Any type-URL convention: a bare "/" followed by the
// fully-qualified message name (NOT the type.googleapis.com prefix). The core
// keys event/command dispatch on it.
const FRAMEWORK_ANY_PREFIX = "/";

/**
 * Any/EventBook helpers the generated wiring uses to build framework messages
 * from the typed values a handler returns, and to merge a snapshot payload into
 * rebuilding state.
 */
export const Pack = {
  /** Wraps a message in a bare-"/" Any. */
  wrap<Desc extends DescMessage>(schema: Desc, msg: MessageShape<Desc>): Any {
    return create(AnySchema, {
      typeUrl: FRAMEWORK_ANY_PREFIX + schema.typeName,
      value: toBinary(schema, msg),
    });
  },

  /** Builds an EventBook whose pages each carry one of the given event Anys —
   * the typed-emit path's book assembly. */
  eventBook(events: Any[]): EventBook {
    return create(EventBookSchema, {
      pages: events.map((event) =>
        create(EventPageSchema, { payload: { case: "event", value: event } }),
      ),
    });
  },

  /** Merges a snapshot Any payload into the rebuilding state in place (the
   * snapshot seeds state before pages fold). */
  merge<Desc extends DescMessage>(
    schema: Desc,
    state: MessageShape<Desc>,
    payload: Any,
  ): void {
    const decoded = fromBinary(schema, payload.value) as Record<
      string,
      unknown
    >;
    const target = state as Record<string, unknown>;
    for (const key of Object.keys(decoded)) {
      if (key === "$typeName" || key === "$unknown") {
        continue;
      }
      target[key] = decoded[key];
    }
  },
};
