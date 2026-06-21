import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import { AnySchema } from "@bufbuild/protobuf/wkt";

import { CodedError, unhandled } from "./codedError";
import { GrpcCode, grpcFromWire } from "./grpcCode";
import { ErrorInfoSchema } from "../gen/google/rpc/error_details_pb";
import { StatusSchema } from "../gen/google/rpc/status_pb";

// The reverse-DNS error domain on every ErrorInfo the boundary emits (distinct
// from the io.angzarr proto package).
const ERROR_INFO_DOMAIN = "angzarr.io";
// The ErrorInfo detail Any uses the type.googleapis.com prefix the ABI pins (the
// shape gRPC serializes on the wire), NOT the framework's bare-"/" convention.
const ERROR_INFO_TYPE_URL = "type.googleapis.com/google.rpc.ErrorInfo";

/** One callback/dispatch outcome: response bytes (possibly null) + ABI status. */
export interface Outcome {
  response: Uint8Array | null;
  status: number;
}

/** Serializes a CodedError as google.rpc.Status bytes carrying a
 * google.rpc.ErrorInfo detail — the exact shape the core decodes. */
export function toStatusBytes(err: CodedError): Uint8Array {
  const info = create(ErrorInfoSchema, {
    reason: err.code,
    domain: ERROR_INFO_DOMAIN,
    metadata: { ...err.extras },
  });
  const status = create(StatusSchema, {
    code: err.grpc,
    message: err.message,
    details: [
      create(AnySchema, {
        typeUrl: ERROR_INFO_TYPE_URL,
        value: toBinary(ErrorInfoSchema, info),
      }),
    ],
  });
  return toBinary(StatusSchema, status);
}

/** Maps any thrown value to (Status bytes, negative gRPC code): a CodedError
 * keeps its code; anything else is UNHANDLED_HANDLER_ERROR / Internal. */
export function errorResult(err: unknown): Outcome {
  const ce =
    err instanceof CodedError
      ? err
      : unhandled(
          err instanceof Error && err.message ? err.message : String(err),
        );
  return { response: toStatusBytes(ce), status: -ce.grpc };
}

/** Decodes google.rpc.Status bytes into a CodedError; `ret` (the negative
 * callback/dispatch return) is the gRPC fallback when the bytes are absent or
 * undecodable. */
export function fromStatusBytes(
  bytes: Uint8Array | null,
  ret: number,
): CodedError {
  const fallback = grpcFromWire(-ret);
  if (!bytes || bytes.length === 0) {
    return new CodedError(
      "",
      "host callback failed without a status payload",
      fallback,
    );
  }
  let status;
  try {
    status = fromBinary(StatusSchema, bytes);
  } catch {
    return unhandled("host callback returned undecodable status bytes");
  }
  let code = "";
  let extras: Record<string, string> = {};
  for (const detail of status.details) {
    if (detail.typeUrl.endsWith("/google.rpc.ErrorInfo")) {
      try {
        const info = fromBinary(ErrorInfoSchema, detail.value);
        code = info.reason;
        extras = { ...info.metadata };
      } catch {
        // leave code/extras empty — message + fallback still classify it
      }
      break;
    }
  }
  const grpc = status.code !== 0 ? grpcFromWire(status.code) : fallback;
  return new CodedError(code, status.message, grpc as GrpcCode, extras);
}
