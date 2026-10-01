// Public surface of the angzarr-router TypeScript binding. The generated wiring
// (*_angzarr.ts) and host handlers import runtime types, the Pack/parseAny
// helpers, and the framework message types/schemas the seam references from
// here.

export { Router } from "./router";
export {
  AggregateDispatch,
  ProcessManagerDispatch,
  ProjectorDispatch,
  SagaDispatch,
  WILDCARD_DOMAIN,
} from "./dispatch";
export { Rebuilder } from "./rebuilder";
export { Destinations } from "./destinations";
export { Pack } from "./pack";
export {
  CodedError,
  CODE_ANY_DECODE_FAILED,
  CODE_ROUTER_CLOSED,
  CODE_UNHANDLED_HANDLER_ERROR,
  anyDecodeError,
  parseAny,
  reject,
  routerClosed,
  unhandled,
} from "./codedError";
export { GrpcCode, grpcFromWire } from "./grpcCode";
export {
  CommandContext,
  type ApplierContextThunk,
  type ApplierThunk,
  type CommandThunk,
  type FactThunk,
  type PageContext,
  type PmEventThunk,
  type PmRejection,
  type PmRejectionThunk,
  type ProjectorEventThunk,
  type ProjectorFinishThunk,
  type ProjectorUnknownThunk,
  type RejectionThunk,
  type SagaEmission,
  type SagaEventContextThunk,
  type SagaEventThunk,
  type UndoThunk,
} from "./thunks";

// Framework message types + protobuf-es schemas the seam and host handlers use.
export * from "../gen/io/angzarr/v1/types_pb";
export * from "../gen/io/angzarr/v1/command_handler_pb";
export * from "../gen/io/angzarr/v1/saga_pb";
export * from "../gen/io/angzarr/v1/process_manager_pb";
