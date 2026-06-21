import {
  fromBinary,
  type DescMessage,
  type MessageShape,
} from "@bufbuild/protobuf";
import { type Any } from "@bufbuild/protobuf/wkt";

import { GrpcCode } from "./grpcCode";

// The code an unclassified handler failure surfaces as — the binding's job to
// classify, mirroring every other angzarr binding.
export const CODE_UNHANDLED_HANDLER_ERROR = "UNHANDLED_HANDLER_ERROR";

// The code an Any-decode failure carries.
export const CODE_ANY_DECODE_FAILED = "ANY_DECODE_FAILED";

/**
 * A stable cross-language coded failure. A handler throws one (via
 * {@link reject}) to fail a command with a code like `VALUE_NOT_POSITIVE`; the
 * binding also throws one when decoding a coded failure the core returned. It
 * crosses the FFI as `google.rpc.Status` carrying a `google.rpc.ErrorInfo`.
 */
export class CodedError extends Error {
  /** SCREAMING_SNAKE cross-language identifier (may be empty). */
  readonly code: string;
  readonly grpc: GrpcCode;
  readonly extras: Readonly<Record<string, string>>;

  constructor(
    code: string,
    message: string,
    grpc: GrpcCode,
    extras?: Record<string, string>,
  ) {
    super(message);
    this.name = "CodedError";
    this.code = code;
    this.grpc = grpc;
    this.extras = extras ?? {};
  }
}

/** An invalid-argument business rejection — the common shape a command handler
 * throws to reject a command with a coded reason. */
export function reject(code: string, message: string): CodedError {
  return new CodedError(code, message, GrpcCode.InvalidArgument);
}

/** Reports that a google.protobuf.Any payload failed to unmarshal — a malformed
 * payload is an invalid argument, not a handler bug. */
export function anyDecodeError(typeUrl: string, cause: unknown): CodedError {
  const reason = cause instanceof Error ? cause.message : String(cause);
  return new CodedError(
    CODE_ANY_DECODE_FAILED,
    `decode Any ${typeUrl}: ${reason}`,
    GrpcCode.InvalidArgument,
    { type_url: typeUrl },
  );
}

/** An unclassified failure → UNHANDLED_HANDLER_ERROR / Internal. */
export function unhandled(message: string): CodedError {
  return new CodedError(
    CODE_UNHANDLED_HANDLER_ERROR,
    message,
    GrpcCode.Internal,
  );
}

/**
 * Parses an Any payload into its typed message, mapping a decode failure to a
 * coded {@link anyDecodeError} — the generated dispatch wiring calls this when
 * unmarshalling a command or event Any.
 */
export function parseAny<Desc extends DescMessage>(
  schema: Desc,
  any: Any,
): MessageShape<Desc> {
  try {
    return fromBinary(schema, any.value);
  } catch (cause) {
    throw anyDecodeError(any.typeUrl, cause);
  }
}
