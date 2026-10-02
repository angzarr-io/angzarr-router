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

// What a fact handler records: the fact (as received, or annotated) followed by
// the events that flag it, in order. Each recorded event folds into the state
// the next fact sees. A fact cannot be refused, so there is no way to record
// nothing. An Any converts implicitly to the fact recorded as received, with no
// flags.
struct FactRecord {
  FactRecord(google::protobuf::Any fact,  // NOLINT(google-explicit-constructor)
             std::vector<google::protobuf::Any> flags = {})
      : fact(std::move(fact)), flags(std::move(flags)) {}

  google::protobuf::Any fact;
  std::vector<google::protobuf::Any> flags;
};

// A command handler, compensator or undo handler returning std::nullopt
// produced no result (STATUS_OK_EMPTY): the core answers with an empty book / no
// compensation. Every aggregate also answers Replay: its state message is
// packed as the replayed state.
template <class TState>
class AggregateDispatch {
 public:
  using CommandFn = std::function<std::optional<io::angzarr::v1::EventBook>(
      const google::protobuf::Any&, TState&, const CommandContext&)>;
  using RejectionFn = std::function<std::optional<io::angzarr::v1::BusinessResponse>(
      const io::angzarr::v1::Notification&, const io::angzarr::v1::RejectionNotification&, TState&,
      const CommandContext&)>;
  using UndoFn = std::function<std::optional<io::angzarr::v1::BusinessResponse>(
      const io::angzarr::v1::Notification&, const io::angzarr::v1::Compensate&, TState&,
      const CommandContext&)>;
  using FactFn = std::function<FactRecord(const google::protobuf::Any&, const TState&)>;

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
  // Undoes an executed command: a Compensate whose command_type is fq_command
  // reaches fn with the rebuilt state.
  AggregateDispatch& OnUndo(std::string fq_command, UndoFn fn) {
    undoes.emplace_back(std::move(fq_command), std::move(fn));
    return *this;
  }
  // Declares the fact type fq_fact: fn handles it against the rebuilt state,
  // returning the fact to record and its flagging events. The router refuses a
  // fact of an undeclared type with NO_FACT_HANDLER before any handler runs.
  AggregateDispatch& OnFact(std::string fq_fact, FactFn fn) {
    facts.emplace_back(std::move(fq_fact), std::move(fn));
    return *this;
  }

  std::string name;
  std::string domain;
  Rebuilder<TState> rebuilder;
  std::vector<std::pair<std::string, CommandFn>> commands;
  std::map<std::string, std::vector<RejectionFn>> rejections;
  std::vector<std::pair<std::string, UndoFn>> undoes;
  std::vector<std::pair<std::string, FactFn>> facts;
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
  // A handler that also reads where the triggering event sits: the source
  // book's cover and the event's sequence (0 when the page carries none).
  using EventWithContextFn = std::function<SagaEmission(const google::protobuf::Any&,
                                                        const Destinations&, const PageContext&)>;

  SagaDispatch(std::string name, std::string input_domain, std::vector<std::string> targets)
      : name(std::move(name)), input_domain(std::move(input_domain)), targets(std::move(targets)) {}

  SagaDispatch& OnEvent(std::string full_name, EventFn fn) {
    return OnEventWithContext(std::move(full_name), CoverOnly{std::move(fn)});
  }
  SagaDispatch& OnEventWithContext(std::string full_name, EventWithContextFn fn) {
    events.emplace_back(std::move(full_name), std::move(fn));
    return *this;
  }

  std::string name;
  std::string input_domain;
  std::vector<std::string> targets;
  std::vector<std::pair<std::string, EventWithContextFn>> events;

 private:
  // A cover-only handler in the context-taking shape.
  struct CoverOnly {
    EventFn fn;
    SagaEmission operator()(const google::protobuf::Any& event, const Destinations& dests,
                            const PageContext& source) const {
      return fn(event, dests, source.cover);
    }
  };
};

// Observes the type URL of a delivered event that has no projector fold.
using ProjectorUnknownFn = std::function<void(const std::string& type_url)>;

template <class TState>
class ProjectorDispatch {
 public:
  using EventFn = std::function<void(TState&, const google::protobuf::Any&)>;
  // A fold that also reads where the event sits (its book's cover and sequence).
  using EventWithContextFn =
      std::function<void(TState&, const google::protobuf::Any&, const PageContext&)>;
  using FinishFn =
      std::function<io::angzarr::v1::Projection(TState&, const io::angzarr::v1::EventBook&)>;
  using UnknownFn = ProjectorUnknownFn;

  explicit ProjectorDispatch(std::string name) : name(std::move(name)) {}

  ProjectorDispatch& ForDomains(std::vector<std::string> ds) {
    domains = std::move(ds);
    return *this;
  }
  ProjectorDispatch& OnEvent(std::string full_name, EventFn fn) {
    events.emplace_back(std::move(full_name),
                        [fn = std::move(fn)](TState& projection, const google::protobuf::Any& event,
                                             const PageContext&) { fn(projection, event); });
    return *this;
  }
  ProjectorDispatch& OnEventWithContext(std::string full_name, EventWithContextFn fn) {
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
  std::vector<std::pair<std::string, EventWithContextFn>> events;
  FinishFn finish;
  UnknownFn unknown;
};

template <class TState>
class ProcessManagerDispatch {
 public:
  using EventFn = std::function<io::angzarr::v1::ProcessManagerHandleResponse(
      const google::protobuf::Any&, TState&, const Destinations&)>;
  // An event handler that also reads the trigger book's cover.
  using EventWithCoverFn = std::function<io::angzarr::v1::ProcessManagerHandleResponse(
      const google::protobuf::Any&, TState&, const Destinations&, const io::angzarr::v1::Cover&)>;
  using RejectionFn =
      std::function<PmRejection(const io::angzarr::v1::Notification&,
                                const io::angzarr::v1::RejectionNotification&, TState&)>;
  // A compensator returning a full response: process events, commands
  // (deferred by the router), facts and escalation.
  using RejectionResponseFn = std::function<io::angzarr::v1::ProcessManagerHandleResponse(
      const io::angzarr::v1::Notification&, const io::angzarr::v1::RejectionNotification&,
      TState&)>;

  struct Handler {
    std::string source_domain;
    std::string full_name;
    EventWithCoverFn fn;
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
    return OnEventWithCover(
        std::move(source_domain), std::move(full_name),
        [fn = std::move(fn)](const google::protobuf::Any& event, TState& state,
                             const Destinations& dests,
                             const io::angzarr::v1::Cover&) { return fn(event, state, dests); });
  }
  ProcessManagerDispatch& OnEventWithCover(std::string source_domain, std::string full_name,
                                           EventWithCoverFn fn) {
    handlers.push_back({std::move(source_domain), std::move(full_name), std::move(fn)});
    return *this;
  }
  // compensates is "fq.Type" or "domain:fq.Type", as for aggregates.
  ProcessManagerDispatch& OnRejected(std::string compensates, RejectionFn fn) {
    return OnRejectedWithResponse(
        std::move(compensates),
        [fn = std::move(fn)](const io::angzarr::v1::Notification& n,
                             const io::angzarr::v1::RejectionNotification& rejection,
                             TState& state) {
          PmRejection r = fn(n, rejection, state);
          io::angzarr::v1::ProcessManagerHandleResponse resp;
          for (auto& e : r.process_events) *resp.add_process_events() = std::move(e);
          if (r.escalation) *resp.mutable_notification() = std::move(*r.escalation);
          return resp;
        });
  }
  ProcessManagerDispatch& OnRejectedWithResponse(std::string compensates, RejectionResponseFn fn) {
    rejections[compensates].push_back(std::move(fn));
    return *this;
  }

  std::string name;
  std::string pm_domain;
  std::vector<std::string> targets;
  Rebuilder<TState> rebuilder;
  std::vector<Handler> handlers;
  std::map<std::string, std::vector<RejectionResponseFn>> rejections;
};

}  // namespace angzarr::router
