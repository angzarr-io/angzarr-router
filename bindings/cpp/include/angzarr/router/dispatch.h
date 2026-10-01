#pragma once

// The four dispatch surfaces. Each holds the user's typed thunks + metadata; the
// Router converts them to type-erased Invokers + a descriptor at registration.
// Generic in the component state message (templates) so the generated wiring is
// cast-free; saga is stateless (non-generic).

#include <google/protobuf/any.pb.h>

#include <functional>
#include <map>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include "angzarr/router/support.h"
#include "io/angzarr/v1/process_manager.pb.h"
#include "io/angzarr/v1/projector.pb.h"

namespace angzarr::router {

// A command handler or compensator returning std::nullopt produced no result
// (STATUS_OK_EMPTY): the core answers with an empty book / no compensation.
template <class TState>
class AggregateDispatch {
 public:
  using CommandFn = std::function<std::optional<io::angzarr::v1::EventBook>(
      const google::protobuf::Any&, TState&, const CommandContext&)>;
  using RejectionFn = std::function<std::optional<io::angzarr::v1::BusinessResponse>(
      const io::angzarr::v1::Notification&, const io::angzarr::v1::RejectionNotification&, TState&,
      const CommandContext&)>;

  AggregateDispatch(std::string name, std::string domain, Rebuilder<TState> rebuilder)
      : name(std::move(name)), domain(std::move(domain)), rebuilder(std::move(rebuilder)) {}

  AggregateDispatch& OnCommand(std::string full_name, CommandFn fn) {
    commands.emplace_back(std::move(full_name), std::move(fn));
    return *this;
  }
  // compensates is the declared entry: the rejected command's fully-qualified
  // type ("fq.Type", sent to any domain) or "domain:fq.Type" (only when it was
  // sent to that domain). Several compensators for one entry run in
  // registration order.
  AggregateDispatch& OnRejected(std::string compensates, RejectionFn fn) {
    rejections[compensates].push_back(std::move(fn));
    return *this;
  }

  std::string name;
  std::string domain;
  Rebuilder<TState> rebuilder;
  std::vector<std::pair<std::string, CommandFn>> commands;
  std::map<std::string, std::vector<RejectionFn>> rejections;
};

// A stateless translator: declared source events emit commands (deferred; the
// router stamps their provenance) and/or fact events. Sagas receive no
// rejections. The Destinations a handler sees are the declared targets.
class SagaDispatch {
 public:
  // sourceCover is the source book's cover, so the saga can route emitted
  // commands by the trigger's identity (root, ext).
  using EventFn = std::function<SagaEmission(const google::protobuf::Any&, const Destinations&,
                                             const io::angzarr::v1::Cover&)>;

  SagaDispatch(std::string name, std::string input_domain, std::vector<std::string> targets)
      : name(std::move(name)), input_domain(std::move(input_domain)), targets(std::move(targets)) {}

  SagaDispatch& OnEvent(std::string full_name, EventFn fn) {
    events.emplace_back(std::move(full_name), std::move(fn));
    return *this;
  }

  std::string name;
  std::string input_domain;
  std::vector<std::string> targets;
  std::vector<std::pair<std::string, EventFn>> events;
};

// Observes the type URL of a delivered event that has no projector fold.
using ProjectorUnknownFn = std::function<void(const std::string& type_url)>;

template <class TState>
class ProjectorDispatch {
 public:
  using EventFn = std::function<void(TState&, const google::protobuf::Any&)>;
  using FinishFn =
      std::function<io::angzarr::v1::Projection(TState&, const io::angzarr::v1::EventBook&)>;
  using UnknownFn = ProjectorUnknownFn;

  explicit ProjectorDispatch(std::string name) : name(std::move(name)) {}

  ProjectorDispatch& ForDomains(std::vector<std::string> ds) {
    domains = std::move(ds);
    return *this;
  }
  ProjectorDispatch& OnEvent(std::string full_name, EventFn fn) {
    events.emplace_back(std::move(full_name), std::move(fn));
    return *this;
  }
  ProjectorDispatch& Finish(FinishFn fn) {
    finish = std::move(fn);
    return *this;
  }
  ProjectorDispatch& OnUnknown(UnknownFn fn) {
    unknown = std::move(fn);
    return *this;
  }

  std::string name;
  std::vector<std::string> domains;
  std::vector<std::pair<std::string, EventFn>> events;
  FinishFn finish;
  UnknownFn unknown;
};

template <class TState>
class ProcessManagerDispatch {
 public:
  using EventFn = std::function<io::angzarr::v1::ProcessManagerHandleResponse(
      const google::protobuf::Any&, TState&, const Destinations&)>;
  using RejectionFn =
      std::function<PmRejection(const io::angzarr::v1::Notification&,
                                const io::angzarr::v1::RejectionNotification&, TState&)>;

  struct Handler {
    std::string source_domain;
    std::string full_name;
    EventFn fn;
  };

  ProcessManagerDispatch(std::string name, std::string pm_domain, Rebuilder<TState> rebuilder)
      : name(std::move(name)), pm_domain(std::move(pm_domain)), rebuilder(std::move(rebuilder)) {}
  // targets are the PM's declared output domains (its command targets): the
  // Destinations its handlers see.
  ProcessManagerDispatch(std::string name, std::string pm_domain, std::vector<std::string> targets,
                         Rebuilder<TState> rebuilder)
      : name(std::move(name)),
        pm_domain(std::move(pm_domain)),
        targets(std::move(targets)),
        rebuilder(std::move(rebuilder)) {}

  ProcessManagerDispatch& OnEvent(std::string source_domain, std::string full_name, EventFn fn) {
    handlers.push_back({std::move(source_domain), std::move(full_name), std::move(fn)});
    return *this;
  }
  // compensates is "fq.Type" or "domain:fq.Type", as for aggregates.
  ProcessManagerDispatch& OnRejected(std::string compensates, RejectionFn fn) {
    rejections[compensates].push_back(std::move(fn));
    return *this;
  }

  std::string name;
  std::string pm_domain;
  std::vector<std::string> targets;
  Rebuilder<TState> rebuilder;
  std::vector<Handler> handlers;
  std::map<std::string, std::vector<RejectionFn>> rejections;
};

}  // namespace angzarr::router
