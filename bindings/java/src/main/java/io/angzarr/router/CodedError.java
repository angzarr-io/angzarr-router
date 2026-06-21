package io.angzarr.router;

import com.google.protobuf.Any;
import com.google.protobuf.InvalidProtocolBufferException;
import com.google.protobuf.Message;
import com.google.protobuf.Parser;
import java.util.Map;

/**
 * A stable cross-language coded failure. A handler throws one (via {@link
 * #reject}) to fail a command with a code like {@code VALUE_NOT_POSITIVE}; the
 * binding also produces one when decoding a coded failure the core returned. It
 * crosses the FFI as {@code google.rpc.Status} carrying a {@code
 * google.rpc.ErrorInfo}.
 */
public final class CodedError extends RuntimeException {

  /** Code an unclassified handler failure surfaces as. */
  public static final String UNHANDLED_HANDLER_ERROR = "UNHANDLED_HANDLER_ERROR";

  /** Code an Any-decode failure carries. */
  public static final String ANY_DECODE_FAILED = "ANY_DECODE_FAILED";

  /** SCREAMING_SNAKE cross-language identifier (may be empty). */
  public final String code;

  public final GrpcCode grpc;
  public final transient Map<String, String> extras;

  public CodedError(String code, String message, GrpcCode grpc, Map<String, String> extras) {
    super(message);
    this.code = code == null ? "" : code;
    this.grpc = grpc == null ? GrpcCode.INTERNAL : grpc;
    this.extras = extras == null ? Map.of() : Map.copyOf(extras);
  }

  /** An invalid-argument business rejection — the common shape a command
   * handler throws to reject a command with a coded reason. */
  public static CodedError reject(String code, String message) {
    return new CodedError(code, message, GrpcCode.INVALID_ARGUMENT, Map.of());
  }

  /** Reports that a google.protobuf.Any payload failed to unmarshal — a
   * malformed payload is an invalid argument, not a handler bug. */
  public static CodedError anyDecodeError(String typeUrl, Throwable cause) {
    return new CodedError(
        ANY_DECODE_FAILED,
        "decode Any " + typeUrl + ": " + cause.getMessage(),
        GrpcCode.INVALID_ARGUMENT,
        Map.of("type_url", typeUrl));
  }

  /** An unclassified failure → UNHANDLED_HANDLER_ERROR / INTERNAL. */
  public static CodedError unhandled(String message) {
    return new CodedError(UNHANDLED_HANDLER_ERROR, message, GrpcCode.INTERNAL, Map.of());
  }

  /**
   * Parses an Any payload into its typed message, mapping a decode failure to a
   * coded {@link #anyDecodeError} — the generated dispatch wiring calls this when
   * unmarshalling a command or event Any.
   */
  public static <T extends Message> T parse(Parser<T> parser, Any any) {
    try {
      return parser.parseFrom(any.getValue());
    } catch (InvalidProtocolBufferException cause) {
      throw anyDecodeError(any.getTypeUrl(), cause);
    }
  }
}
