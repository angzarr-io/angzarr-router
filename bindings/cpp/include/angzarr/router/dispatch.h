#pragma once

// The four dispatch surfaces. Each holds the user's typed thunks + metadata; the
// Router converts them to type-erased Invokers + a descriptor at registration.
// Generic in the component state message (templates) so the generated wiring is
// cast-free; saga is stateless (non-generic).

#include <google/protobuf/any.pb.h>

#include <functional>
#include <map>
#include <string>
#include <utility>
#include <vector>

#include "angzarr/router/support.h"
#include "io/angzarr/v1/process_manager.pb.h"
#include "io/angzarr/v1/projector.pb.h"

namespace angzarr::router {

template <class TState>
class AggregateDispatch {
 public:
  using CommandFn = std::function<io::angzarr::v1::EventBook(const google::protobuf::Any&, TState&,
                                                             const CommandContext&)>;
  using RejectionFn = std::function<io::angzarr::v1::BusinessResponse(
      const io::angzarr::v1::Notification&, const io::angzarr::v1::RejectionNotification&, TState&,
      const CommandContext&)>;

  AggregateDispatch(std::string name, std::string domain, Rebuilder<TState> rebuilder)
      : name(std::move(name)), domain(std::move(domain)), rebuilder(std::move(rebuilder)) {}

  AggregateDispatch& OnCommand(std::string full_name, CommandFn fn) {
    commands.emplace_back(std::move(full_name), std::move(fn));
    return *this;
  }
  AggregateDispatch& OnRejected(std::string fq_command, RejectionFn fn) {
    rejections[fq_command].push_back(std::move(fn));
    return *this;
  }

  std::string name;
  std::string domain;
  Rebuilder<TState> rebuilder;
  std::vector<std::pair<std::string, CommandFn>> commands;
  std::map<std::string, std::vector<RejectionFn>> rejections;
};

class SagaDispatch {
 public:
  // sourceCover is the source book's cover, so the saga can route emitted
  // commands by the trigger's identity (root, ext).
  using EventFn = std::function<SagaEmission(
      const google::protobuf::Any&, const Destinations&, const io::angzarr::v1::Cover&)>;
  using RejectionFn = std::function<std::vector<io::angzarr::v1::EventBook>(
      const io::angzarr::v1::Notification&, const io::angzarr::v1::RejectionNotification&)>;

  SagaDispatch(std::string name, std::string input_domain, std::vector<std::string> targets)
      : name(std::move(name)), input_domain(std::move(input_domain)), targets(std::move(targets)) {}

  SagaDispatch& OnEvent(std::string full_name, EventFn fn) {
    events.emplace_back(std::move(full_name), std::move(fn));
    return *this;
  }
  SagaDispatch& OnRejected(std::string fq_command, RejectionFn fn) {
    rejections[fq_command].push_back(std::move(fn));
    return *this;
  }

  std::string name;
  std::string input_domain;
  std::vector<std::string> targets;
  std::vector<std::pair<std::string, EventFn>> events;
  std::map<std::string, std::vector<RejectionFn>> rejections;
};

template <class TState>
class ProjectorDispatch {
 public:
  using EventFn = std::function<void(TState&, const google::protobuf::Any&)>;
  using FinishFn =
      std::function<io::angzarr::v1::Projection(TState&, const io::angzarr::v1::EventBook&)>;

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

  std::string name;
  std::vector<std::string> domains;
  std::vector<std::pair<std::string, EventFn>> events;
  FinishFn finish;
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

  ProcessManagerDispatch& OnEvent(std::string source_domain, std::string full_name, EventFn fn) {
    handlers.push_back({std::move(source_domain), std::move(full_name), std::move(fn)});
    return *this;
  }
  ProcessManagerDispatch& OnRejected(std::string fq_command, RejectionFn fn) {
    rejections[fq_command].push_back(std::move(fn));
    return *this;
  }

  std::string name;
  std::string pm_domain;
  Rebuilder<TState> rebuilder;
  std::vector<Handler> handlers;
  std::map<std::string, std::vector<RejectionFn>> rejections;
};

}  // namespace angzarr::router
