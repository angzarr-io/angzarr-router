package io.angzarr.router;

import com.google.protobuf.Any;
import com.google.protobuf.InvalidProtocolBufferException;
import com.google.rpc.ErrorInfo;
import com.google.rpc.Status;
import java.util.Map;

/**
 * Serializes a {@link CodedError} as {@code google.rpc.Status} bytes carrying a
 * {@code google.rpc.ErrorInfo} detail — the exact shape the core decodes (and
 * that gRPC puts on the wire) — and back.
 */
final class Statuses {
  private Statuses() {}

  private static final String ERROR_INFO_DOMAIN = "angzarr.io";
  private static final String ERROR_INFO_TYPE_URL = "type.googleapis.com/google.rpc.ErrorInfo";

  static byte[] toStatusBytes(CodedError err) {
    ErrorInfo info =
        ErrorInfo.newBuilder()
            .setReason(err.code)
            .setDomain(ERROR_INFO_DOMAIN)
            .putAllMetadata(err.extras)
            .build();
    String message = err.getMessage() == null ? "" : err.getMessage();
    return Status.newBuilder()
        .setCode(err.grpc.value)
        .setMessage(message)
        .addDetails(Any.newBuilder().setTypeUrl(ERROR_INFO_TYPE_URL).setValue(info.toByteString()))
        .build()
        .toByteArray();
  }

  /** Maps any throwable to (Status bytes, negative gRPC code): a CodedError
   * keeps its code; anything else is UNHANDLED_HANDLER_ERROR / INTERNAL. */
  static Invoker.Result errorResult(Throwable t) {
    CodedError ce =
        (t instanceof CodedError c)
            ? c
            : CodedError.unhandled(t.getMessage() == null ? t.toString() : t.getMessage());
    return new Invoker.Result(toStatusBytes(ce), -ce.grpc.value);
  }

  /** Decodes google.rpc.Status bytes into a CodedError; {@code ret} (the
   * negative callback/dispatch return) is the gRPC fallback when the bytes are
   * absent or undecodable. */
  static CodedError fromStatusBytes(byte[] bytes, int ret) {
    GrpcCode fallback = GrpcCode.fromWire(-ret);
    if (bytes == null || bytes.length == 0) {
      return new CodedError(
          "", "host callback failed without a status payload", fallback, Map.of());
    }
    Status status;
    try {
      status = Status.parseFrom(bytes);
    } catch (InvalidProtocolBufferException e) {
      return CodedError.unhandled("host callback returned undecodable status bytes");
    }
    String code = "";
    Map<String, String> extras = Map.of();
    for (Any detail : status.getDetailsList()) {
      if (detail.getTypeUrl().equals(ERROR_INFO_TYPE_URL)) {
        try {
          ErrorInfo info = ErrorInfo.parseFrom(detail.getValue());
          code = info.getReason();
          extras = info.getMetadataMap();
        } catch (InvalidProtocolBufferException ignored) {
          // leave code/extras empty — the message + fallback still classify it
        }
        break;
      }
    }
    GrpcCode grpc = status.getCode() != 0 ? GrpcCode.fromWire(status.getCode()) : fallback;
    return new CodedError(code, status.getMessage(), grpc, extras);
  }
}
