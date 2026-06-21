// The numeric gRPC status code carried with a coded failure. Kept as a plain
// const enum over an int so the binding depends only on the protobuf runtime,
// not a gRPC library.
export enum GrpcCode {
  InvalidArgument = 3,
  NotFound = 5,
  FailedPrecondition = 9,
  Unimplemented = 12,
  Internal = 13,
  DataLoss = 15,
}

// Wire code → GrpcCode; an unmapped code degrades to Internal (an unmapped code
// is a binding bug, not a business signal).
export function grpcFromWire(code: number): GrpcCode {
  switch (code) {
    case 3:
      return GrpcCode.InvalidArgument;
    case 5:
      return GrpcCode.NotFound;
    case 9:
      return GrpcCode.FailedPrecondition;
    case 12:
      return GrpcCode.Unimplemented;
    case 15:
      return GrpcCode.DataLoss;
    default:
      return GrpcCode.Internal;
  }
}
