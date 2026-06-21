package io.angzarr.router;

/**
 * The numeric gRPC status code carried with a coded failure. Kept as a plain
 * enum over an int so the binding depends only on the protobuf runtime, not the
 * gRPC library.
 */
public enum GrpcCode {
  INVALID_ARGUMENT(3),
  NOT_FOUND(5),
  FAILED_PRECONDITION(9),
  UNIMPLEMENTED(12),
  INTERNAL(13),
  DATA_LOSS(15);

  public final int value;

  GrpcCode(int value) {
    this.value = value;
  }

  /** Wire code → GrpcCode; unknown codes degrade to INTERNAL (an unmapped code
   * is a binding bug). */
  public static GrpcCode fromWire(int code) {
    return switch (code) {
      case 3 -> INVALID_ARGUMENT;
      case 5 -> NOT_FOUND;
      case 9 -> FAILED_PRECONDITION;
      case 12 -> UNIMPLEMENTED;
      case 15 -> DATA_LOSS;
      default -> INTERNAL;
    };
  }
}
