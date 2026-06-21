#pragma once

// The host-side value types the dispatch surfaces and Router use: the command
// context, destination stamping, Any packing, the saga/PM emission results, the
// per-dispatch Session (host_ctx), the type-erased Invoker, and the Rebuilder.

#include <google/protobuf/any.pb.h>
#include <google/protobuf/message.h>

#include <cstdint>
#include <functional>
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include "angzarr/router/coded_error.h"
#include "io/angzarr/v1/command_handler.pb.h"
#include "io/angzarr/v1/types.pb.h"

namespace angzarr::router {

class Router;  // defined in router.h

// The historical-state evidence a command handler sees. Host state never crosses
// the FFI, so the core reconstructs this from the prior-events book.
struct CommandContext {
  uint32_t next_sequence = 0;
  bool had_prior_events = false;
};

// The coordinator-supplied next-sequences for command stamping. Sagas and PMs
// are translators — they stamp emitted commands, they do not rebuild state.
class Destinations {
 public:
  explicit Destinations(std::map<std::string, uint32_t> sequences)
      : sequences_(std::move(sequences)) {}

  bool Has(const std::string& domain) const { return sequences_.count(domain) > 0; }

  // Returns a copy of cmd with every page stamped with the next sequence for
  // domain; a domain with no supplied sequence is MISSING_DESTINATION_SEQUENCE.
  io::angzarr::v1::CommandBook StampCommand(io::angzarr::v1::CommandBook cmd,
                                            const std::string& domain) const {
    auto it = sequences_.find(domain);
    if (it == sequences_.end()) {
      throw CodedError("MISSING_DESTINATION_SEQUENCE", "no sequence for destination domain",
                       GrpcCode::kInvalidArgument, {{"domain", domain}});
    }
    for (auto& page : *cmd.mutable_pages()) {
      page.mutable_header()->set_sequence(it->second);
    }
    return cmd;
  }

 private:
  std::map<std::string, uint32_t> sequences_;
};

// Wraps a message in a google.protobuf.Any using the framework's bare-"/"
// type-URL convention (NOT the type.googleapis.com prefix).
struct Pack {
  static google::protobuf::Any Wrap(const google::protobuf::Message& msg) {
    google::protobuf::Any any;
    any.set_type_url("/" + msg.GetDescriptor()->full_name());
    any.set_value(msg.SerializeAsString());
    return any;
  }
};

// A saga event's emission: commands to issue + fact events to inject.
struct SagaEmission {
  std::vector<io::angzarr::v1::CommandBook> commands;
  std::vector<io::angzarr::v1::EventBook> events;
};

// A PM rejection's result: process events to fold + an optional escalation.
struct PmRejection {
  std::vector<io::angzarr::v1::EventBook> process_events;
  std::optional<io::angzarr::v1::Notification> escalation;
};

// One dispatch's host-side state object, reached from callbacks via host_ctx.
// The rebuilt state is created lazily by the first stateful callback. State is a
// mutable protobuf message held as the base type; EnsureState<T> performs the
// single erasing cast — guaranteed correct because the same TState's invokers
// created it.
class Session {
 public:
  explicit Session(Router& router) : router_(router) {}

  Router& router() { return router_; }

  template <class TState>
  TState& EnsureState() {
    if (!state_) {
      state_ = std::make_unique<TState>();
    }
    return static_cast<TState&>(*state_);
  }

 private:
  Router& router_;
  std::unique_ptr<google::protobuf::Message> state_;
};

// A callback's outcome: response bytes (when has_response) and the ABI status.
struct InvokerResult {
  std::string response;
  int32_t status;
  bool has_response;
};

// The type-erased bridge from a callback_id to a registered thunk. Throwing is
// the failure path — the trampoline's catch is the exception firewall.
using Invoker = std::function<InvokerResult(Session&, const std::string& type_url,
                                            const std::string& payload, const std::string& aux)>;

// Folds a component's prior events (and optional snapshot) into state before a
// command runs. Generic in the state message so appliers stay typed.
template <class TState>
class Rebuilder {
 public:
  using ApplierFn = std::function<void(TState&, const google::protobuf::Any&)>;

  Rebuilder& WithSnapshot(ApplierFn fn) {
    snapshot = std::move(fn);
    return *this;
  }

  Rebuilder& Apply(std::string full_name, ApplierFn fn) {
    appliers.emplace_back(std::move(full_name), std::move(fn));
    return *this;
  }

  ApplierFn snapshot;
  std::vector<std::pair<std::string, ApplierFn>> appliers;
};

}  // namespace angzarr::router
