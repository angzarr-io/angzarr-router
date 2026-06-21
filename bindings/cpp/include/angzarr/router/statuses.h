#pragma once

// Serializes a CodedError as google.rpc.Status bytes carrying a
// google.rpc.ErrorInfo detail — the exact shape the core decodes (and that gRPC
// puts on the wire) — and back. google.rpc.* is generated for C++ (no runtime
// package as in Java/C#).

#include <google/protobuf/any.pb.h>
#include <google/rpc/error_details.pb.h>
#include <google/rpc/status.pb.h>

#include <map>
#include <string>

#include "angzarr/router/coded_error.h"

namespace angzarr::router {

inline constexpr char kErrorInfoDomain[] = "angzarr.io";
inline constexpr char kErrorInfoTypeUrl[] = "type.googleapis.com/google.rpc.ErrorInfo";

inline std::string ToStatusBytes(const CodedError& err) {
  google::rpc::ErrorInfo info;
  info.set_reason(err.code);
  info.set_domain(kErrorInfoDomain);
  for (const auto& [k, v] : err.extras) {
    (*info.mutable_metadata())[k] = v;
  }
  google::rpc::Status status;
  status.set_code(static_cast<int32_t>(err.grpc));
  status.set_message(err.what());
  auto* detail = status.add_details();
  detail->set_type_url(kErrorInfoTypeUrl);
  detail->set_value(info.SerializeAsString());
  return status.SerializeAsString();
}

// Decodes google.rpc.Status bytes into a CodedError; ret (the negative
// callback/dispatch return) is the gRPC fallback when bytes are absent/undecodable.
inline CodedError FromStatusBytes(const std::string& bytes, int32_t ret) {
  const GrpcCode fallback = GrpcFromWire(-ret);
  google::rpc::Status status;
  if (bytes.empty() || !status.ParseFromString(bytes)) {
    return CodedError("", "host callback failed without a status payload", fallback);
  }
  std::string code;
  std::map<std::string, std::string> extras;
  for (const auto& detail : status.details()) {
    if (detail.type_url() == kErrorInfoTypeUrl) {
      google::rpc::ErrorInfo info;
      if (info.ParseFromString(detail.value())) {
        code = info.reason();
        for (const auto& [k, v] : info.metadata()) {
          extras[k] = v;
        }
      }
      break;
    }
  }
  const GrpcCode grpc = status.code() != 0 ? GrpcFromWire(status.code()) : fallback;
  return CodedError(code, status.message(), grpc, extras);
}

}  // namespace angzarr::router
