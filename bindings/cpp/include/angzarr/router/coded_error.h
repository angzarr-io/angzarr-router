#pragma once

// A stable cross-language coded failure. A handler throws one (via Reject) to
// fail a command with a code like VALUE_NOT_POSITIVE; the binding also produces
// one when decoding a coded failure the core returned. It crosses the FFI as
// google.rpc.Status carrying a google.rpc.ErrorInfo.

#include <google/protobuf/any.pb.h>
#include <google/protobuf/message.h>

#include <cstdint>
#include <map>
#include <stdexcept>
#include <string>

namespace angzarr::router {

// The numeric gRPC status code carried with a coded failure.
enum class GrpcCode : int32_t {
  kInvalidArgument = 3,
  kNotFound = 5,
  kFailedPrecondition = 9,
  kUnimplemented = 12,
  kInternal = 13,
  kDataLoss = 15,
};

inline GrpcCode GrpcFromWire(int32_t code) {
  switch (code) {
    case 3:
      return GrpcCode::kInvalidArgument;
    case 5:
      return GrpcCode::kNotFound;
    case 9:
      return GrpcCode::kFailedPrecondition;
    case 12:
      return GrpcCode::kUnimplemented;
    case 15:
      return GrpcCode::kDataLoss;
    default:
      return GrpcCode::kInternal;
  }
}

class CodedError : public std::runtime_error {
 public:
  std::string code;  // SCREAMING_SNAKE cross-language identifier (may be empty)
  GrpcCode grpc;
  std::map<std::string, std::string> extras;

  CodedError(std::string code, const std::string& message, GrpcCode grpc,
             std::map<std::string, std::string> extras = {})
      : std::runtime_error(message), code(std::move(code)), grpc(grpc), extras(std::move(extras)) {}

  // An invalid-argument business rejection — the common shape a command handler
  // throws to reject a command with a coded reason.
  static CodedError Reject(const std::string& code, const std::string& message) {
    return CodedError(code, message, GrpcCode::kInvalidArgument);
  }

  // An unclassified failure → UNHANDLED_HANDLER_ERROR / Internal.
  static CodedError Unhandled(const std::string& message) {
    return CodedError("UNHANDLED_HANDLER_ERROR", message, GrpcCode::kInternal);
  }

  // A malformed google.protobuf.Any payload — an invalid argument, not a bug.
  static CodedError AnyDecode(const std::string& type_url) {
    return CodedError("ANY_DECODE_FAILED", "decode Any " + type_url, GrpcCode::kInvalidArgument,
                      {{"type_url", type_url}});
  }

  // Parses an Any payload into its typed message, mapping a decode failure to a
  // coded AnyDecode — the generated dispatch wiring calls this for command/event.
  template <class T>
  static T Parse(const google::protobuf::Any& any) {
    T msg;
    if (!msg.ParseFromString(any.value())) {
      throw AnyDecode(any.type_url());
    }
    return msg;
  }

  // Folds a snapshot Any payload into a (fresh) state message during rebuild.
  static void Merge(google::protobuf::Message& state, const google::protobuf::Any& payload) {
    if (!state.MergeFromString(payload.value())) {
      throw AnyDecode(payload.type_url());
    }
  }
};

}  // namespace angzarr::router
