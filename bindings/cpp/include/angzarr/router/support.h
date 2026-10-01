#pragma once

// The host-side value types the dispatch surfaces and Router use: the command
// context, declared output destinations, Any packing, the saga/PM emission
// results, the per-dispatch Session (host_ctx), the type-erased Invoker, and the
// Rebuilder.

#include <google/protobuf/any.pb.h>
#include <google/protobuf/message.h>

#include <algorithm>
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

// The historical-state evidence a command handler (or compensator, or undo
// handler) sees. Host state never crosses the FFI, so the core reconstructs
// this from the prior-events book.
struct CommandContext {
  uint32_t next_sequence = 0;
  bool had_prior_events = false;
  // The cover of the command (or notification delivery) being handled: the
  // aggregate's own domain and root.
  io::angzarr::v1::Cover cover;
};

// Where a projected event sits: its book's cover and the page's explicit
// sequence (0 when absent).
struct PageContext {
  io::angzarr::v1::Cover cover;
  uint32_t sequence = 0;
};

// The declared output domains of one saga or process manager (its command
// targets), in declaration order. Emitted commands are deferred: the router
// stamps their angzarr_deferred provenance, so handlers never stamp sequences.
class Destinations {
 public:
  explicit Destinations(std::vector<std::string> domains) : domains_(std::move(domains)) {}

  // True when domain is a declared output domain.
  bool Has(const std::string& domain) const {
    return std::find(domains_.begin(), domains_.end(), domain) != domains_.end();
  }

  // The declared output domains, in declaration order.
  const std::vector<std::string>& Domains() const { return domains_; }

 private:
  std::vector<std::string> domains_;
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

// Identifies one registered component (aggregate, projector or process
// manager) within a Router. Every invoker registered for a component captures
// its key, so callbacks reach that component's state and no other.
using ComponentKey = uint64_t;

// One dispatch's host-side state, reached from callbacks via host_ctx. A single
// dispatch may run several components (co-resident process managers subscribed
// to one domain), so state is held per component key, each created lazily by
// that component's first stateful callback. State is a mutable protobuf message
// held as the base type; EnsureState<T> performs the single erasing cast —
// correct because only the invokers of the component that owns the key (all
// typed on that component's TState) create or reach its entry.
class Session {
 public:
  explicit Session(Router& router) : router_(router) {}

  Router& router() { return router_; }

  template <class TState>
  TState& EnsureState(ComponentKey key) {
    auto& slot = states_[key];
    if (!slot) {
      slot = std::make_unique<TState>();
    }
    return static_cast<TState&>(*slot);
  }

 private:
  Router& router_;
  std::map<ComponentKey, std::unique_ptr<google::protobuf::Message>> states_;
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
