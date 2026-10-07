package io.angzarr.router;

import java.lang.foreign.MemorySegment;

/**
 * The binding's raw registration entry point, reached directly with a hand-built descriptor: the
 * shape a hand-written dispatch API refuses to produce (a saga declaring rejection handlers).
 */
public final class RawRegistration {
  private RawRegistration() {}

  /** A registration's return code and the coded error it surfaces (null on success). */
  public record Outcome(int ret, CodedError error) {}

  /**
   * Registers descriptor (a serialized SagaDescriptor) on a fresh native router through the
   * binding's FFI and reports the return code and the coded error it surfaces (null on success).
   */
  public static Outcome registerSaga(byte[] descriptor) {
    MemorySegment router = Ffi.routerNew();
    try {
      int ret = Ffi.registerSaga(router, descriptor);
      return new Outcome(ret, ret == 0 ? null : Statuses.fromStatusBytes(null, ret));
    } finally {
      Ffi.routerFree(router);
    }
  }
}
