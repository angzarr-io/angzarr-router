#pragma once

// The C++ binding's router: wraps the native router plus the host-side callback
// registry the core reaches through the single callback gateway. Register a
// component (assigning callback ids to its thunks and handing the core a
// serialized descriptor), then dispatch books/commands through it.
//
// The dispatch surfaces are generic in the component state message, so the
// generated wiring and handler thunks are statically typed. The one unavoidable
// erasing cast — the FFI registry is keyed by an opaque callback_id, not a type
// — lives in Session::EnsureState<TState>(key): each registration draws a fresh
// component key that all of that component's invokers capture, so a state entry
// is only ever created and reached by invokers typed on the same TState.
// host_ctx is a Session* directly.

#include <atomic>
#include <cstdint>
#include <cstring>
#include <map>
#include <mutex>
#include <stdexcept>
#include <string>

#include "angzarr/router/coded_error.h"
#include "angzarr/router/dispatch.h"
#include "angzarr/router/ffi.h"
#include "angzarr/router/statuses.h"
#include "angzarr/router/support.h"
#include "io/angzarr/router/ffi/v1/abi.pb.h"
#include "io/angzarr/v1/command_handler.pb.h"
#include "io/angzarr/v1/process_manager.pb.h"
#include "io/angzarr/v1/projector.pb.h"
#include "io/angzarr/v1/saga.pb.h"
#include "io/angzarr/v1/types.pb.h"

namespace angzarr::router {

namespace abi = io::angzarr::router::ffi::v1;
namespace pb = io::angzarr::v1;

extern "C" inline int32_t AngzarrTrampoline(void* host_ctx, uint64_t callback_id,
                                            const uint8_t* type_url, size_t type_url_len,
                                            const uint8_t* payload, size_t payload_len,
                                            const uint8_t* aux, size_t aux_len,
                                            ffi::AngzarrBuf* out);

// Refuses a router-ffi library whose ABI version differs from the one this
// binding is written against, naming both versions.
inline void CheckAbiVersion(uint32_t actual) {
  if (actual != ffi::kAbiVersion) {
    throw std::runtime_error("router-ffi ABI version mismatch: expected " +
                             std::to_string(ffi::kAbiVersion) + ", got " + std::to_string(actual));
  }
}

class Router {
 public:
  // Checks the linked router-ffi's ABI version before creating the native
  // router; a drifted library throws std::runtime_error.
  Router() : ptr_(NewNative()) {}
  ~Router() { ffi::angzarr_router_free(ptr_); }
  Router(const Router&) = delete;
  Router& operator=(const Router&) = delete;

  // The ABI version the linked router-ffi library reports.
  static uint32_t AbiVersion() { return ffi::angzarr_abi_version(); }

  Invoker* InvokerFor(uint64_t id) {
    std::lock_guard<std::mutex> lock(mu_);
    auto it = registry_.find(id);
    return it == registry_.end() ? nullptr : &it->second;
  }

  // --- registration --------------------------------------------------------

  template <class TState>
  void RegisterAggregate(AggregateDispatch<TState> d) {
    const ComponentKey key = NextComponentKey();
    abi::AggregateDescriptor desc;
    desc.set_name(d.name);
    desc.set_domain(d.domain);
    for (auto& [fq, fn] : d.rebuilder.appliers) {
      auto* e = desc.add_appliers();
      e->set_fq_type(fq);
      e->set_callback_id(Assign(ApplierInvoker<TState>(key, fn)));
    }
    if (d.rebuilder.snapshot) {
      desc.set_snapshot_callback_id(Assign(SnapshotInvoker<TState>(key, d.rebuilder.snapshot)));
    }
    for (auto& [fq, fn] : d.commands) {
      auto* e = desc.add_commands();
      e->set_fq_type(fq);
      e->set_callback_id(Assign(CommandInvoker<TState>(key, fn)));
    }
    for (auto& [fq, fns] : d.rejections) {
      auto* entry = desc.add_rejections();
      entry->set_compensates(fq);
      for (auto& fn : fns) entry->add_callback_ids(Assign(RejectionInvoker<TState>(key, fn)));
    }
    for (auto& [fq, fn] : d.undoes) {
      auto* e = desc.add_undoes();
      e->set_fq_type(fq);
      e->set_callback_id(Assign(UndoInvoker<TState>(key, fn)));
    }
    for (auto& [fq, fn] : d.facts) {
      auto* e = desc.add_facts();
      e->set_fq_type(fq);
      e->set_callback_id(Assign(FactInvoker<TState>(key, fn)));
    }
    desc.set_state_callback_id(Assign(StateInvoker<TState>(key)));
    Register(ffi::angzarr_router_register_aggregate, desc);
  }

  template <class TState>
  void RegisterProjector(ProjectorDispatch<TState> d) {
    const ComponentKey key = NextComponentKey();
    abi::ProjectorDescriptor desc;
    desc.set_name(d.name);
    for (auto& dom : d.domains) desc.add_domains(dom);
    for (auto& [fq, fn] : d.events) {
      auto* e = desc.add_events();
      e->set_fq_type(fq);
      e->set_callback_id(Assign(ProjectorEventInvoker<TState>(key, fn)));
    }
    if (d.unknown) {
      desc.set_unknown_callback_id(Assign(ProjectorUnknownInvoker(d.unknown)));
    }
    if (d.finish) {
      desc.set_finish_callback_id(Assign(ProjectorFinishInvoker<TState>(key, d.finish)));
    }
    Register(ffi::angzarr_router_register_projector, desc);
  }

  void RegisterSaga(SagaDispatch d) {
    abi::SagaDescriptor desc;
    desc.set_name(d.name);
    desc.set_input_domain(d.input_domain);
    for (auto& t : d.targets) desc.add_target_domains(t);
    for (auto& [fq, fn] : d.events) {
      auto* e = desc.add_events();
      e->set_fq_type(fq);
      e->set_callback_id(Assign(SagaEventInvoker(d.targets, fn)));
    }
    Register(ffi::angzarr_router_register_saga, desc);
  }

  template <class TState>
  void RegisterProcessManager(ProcessManagerDispatch<TState> d) {
    const ComponentKey key = NextComponentKey();
    abi::ProcessManagerDescriptor desc;
    desc.set_name(d.name);
    desc.set_pm_domain(d.pm_domain);
    for (auto& t : d.targets) desc.add_target_domains(t);
    for (auto& [fq, fn] : d.rebuilder.appliers) {
      auto* e = desc.add_appliers();
      e->set_fq_type(fq);
      e->set_callback_id(Assign(ApplierInvoker<TState>(key, fn)));
    }
    if (d.rebuilder.snapshot) {
      desc.set_snapshot_callback_id(Assign(SnapshotInvoker<TState>(key, d.rebuilder.snapshot)));
    }
    for (auto& h : d.handlers) {
      auto* e = desc.add_events();
      e->set_input_domain(h.source_domain);
      e->set_fq_type(h.full_name);
      e->set_callback_id(Assign(PmEventInvoker<TState>(key, d.targets, h.fn)));
    }
    for (auto& [fq, fns] : d.rejections) {
      auto* entry = desc.add_rejections();
      entry->set_compensates(fq);
      for (auto& fn : fns) entry->add_callback_ids(Assign(PmRejectionInvoker<TState>(key, fn)));
    }
    desc.set_state_callback_id(Assign(StateInvoker<TState>(key)));
    Register(ffi::angzarr_router_register_process_manager, desc);
  }

  // --- dispatch ------------------------------------------------------------

  pb::BusinessResponse Dispatch(const pb::ContextualCommand& command) {
    return ParseResponse<pb::BusinessResponse>(DispatchVia(command, ffi::angzarr_router_dispatch),
                                               "BusinessResponse");
  }
  pb::SagaResponse DispatchSaga(const pb::SagaHandleRequest& request) {
    return ParseResponse<pb::SagaResponse>(DispatchVia(request, ffi::angzarr_router_dispatch_saga),
                                           "SagaResponse");
  }
  pb::Projection DispatchProjector(const pb::EventBook& book) {
    return ParseResponse<pb::Projection>(DispatchVia(book, ffi::angzarr_router_dispatch_projector),
                                         "Projection");
  }
  pb::ProcessManagerHandleResponse DispatchProcessManager(
      const pb::ProcessManagerHandleRequest& request) {
    return ParseResponse<pb::ProcessManagerHandleResponse>(
        DispatchVia(request, ffi::angzarr_router_dispatch_process_manager),
        "ProcessManagerHandleResponse");
  }
  // Handles facts through the aggregate claiming the facts' cover domain;
  // returns the facts to record.
  pb::EventBook DispatchFact(const pb::FactRequest& request) {
    return ParseResponse<pb::EventBook>(DispatchVia(request, ffi::angzarr_router_dispatch_fact),
                                        "EventBook");
  }
  // Replays history into the state of the aggregate claiming domain, else the
  // process manager whose own domain it is (empty selects a sole registered
  // aggregate); returns its packed state.
  pb::ReplayResponse DispatchReplay(const std::string& domain, const pb::ReplayRequest& request) {
    abi::ReplayCall call;
    call.set_domain(domain);
    *call.mutable_request() = request;
    return ParseResponse<pb::ReplayResponse>(DispatchVia(call, ffi::angzarr_router_dispatch_replay),
                                             "ReplayResponse");
  }

 private:
  struct Dispatched {
    std::string response;
    int32_t status;
  };

  static void* NewNative() {
    CheckAbiVersion(AbiVersion());
    return ffi::angzarr_router_new();
  }

  ComponentKey NextComponentKey() { return ++next_component_; }

  uint64_t Assign(Invoker invoker) {
    std::lock_guard<std::mutex> lock(mu_);
    uint64_t id = ++next_id_;
    registry_[id] = std::move(invoker);
    return id;
  }

  template <class Desc, class Fn>
  void Register(Fn ffi_fn, const Desc& desc) {
    std::string bytes = desc.SerializeAsString();
    int32_t ret = ffi_fn(ptr_, reinterpret_cast<const uint8_t*>(bytes.data()), bytes.size(),
                         &AngzarrTrampoline);
    if (ret != 0) throw FromStatusBytes("", ret);
  }

  template <class Fn>
  Dispatched DispatchVia(const google::protobuf::Message& request, Fn ffi_fn) {
    Session session(*this);
    std::string req = request.SerializeAsString();
    ffi::AngzarrBuf out{nullptr, 0};
    int32_t ret =
        ffi_fn(ptr_, &session, reinterpret_cast<const uint8_t*>(req.data()), req.size(), &out);
    return {ConsumeOut(&out), ret};
  }

  template <class T>
  T ParseResponse(const Dispatched& d, const char* what) {
    if (d.status != 0) throw FromStatusBytes(d.response, d.status);
    T msg;
    if (!msg.ParseFromString(d.response)) {
      throw CodedError::Unhandled(std::string("unmarshal ") + what);
    }
    return msg;
  }

  static std::string ConsumeOut(ffi::AngzarrBuf* out) {
    if (!out->data || out->len == 0) return {};
    std::string s(reinterpret_cast<const char*>(out->data), out->len);
    ffi::angzarr_buf_release(out->data, out->len);
    return s;
  }

  static CommandContext ContextOf(const abi::CommandContextAux& cax) {
    return CommandContext{cax.next_sequence(), cax.had_prior_events(), cax.cover()};
  }

  static google::protobuf::Any AnyOf(const std::string& type_url, const std::string& payload) {
    google::protobuf::Any any;
    any.set_type_url(type_url);
    any.set_value(payload);
    return any;
  }

  // --- invoker adapters (the lone TState cast lives in EnsureState) --------
  // Each stateful adapter captures its component's key and reaches only that
  // component's state in the session.

  static PageContext PageOf(const std::string& aux) {
    abi::ProjectorEventAux pax;
    pax.ParseFromString(aux);
    return PageContext{pax.cover(), pax.sequence()};
  }

  template <class TState>
  static Invoker ApplierInvoker(ComponentKey key,
                                typename Rebuilder<TState>::ApplierWithContextFn fn) {
    return [key, fn](Session& s, const std::string& tu, const std::string& payload,
                     const std::string& aux) {
      fn(s.EnsureState<TState>(key), AnyOf(tu, payload), PageOf(aux));
      return InvokerResult{"", ffi::kStatusOk, false};
    };
  }

  template <class TState>
  static Invoker SnapshotInvoker(ComponentKey key, typename Rebuilder<TState>::ApplierFn fn) {
    return [key, fn](Session& s, const std::string& tu, const std::string& payload,
                     const std::string&) {
      fn(s.EnsureState<TState>(key), AnyOf(tu, payload));
      return InvokerResult{"", ffi::kStatusOk, false};
    };
  }

  template <class TState>
  static Invoker CommandInvoker(ComponentKey key,
                                typename AggregateDispatch<TState>::CommandFn fn) {
    return [key, fn](Session& s, const std::string& tu, const std::string& payload,
                     const std::string& aux) {
      abi::CommandContextAux cax;
      cax.ParseFromString(aux);
      auto book = fn(AnyOf(tu, payload), s.EnsureState<TState>(key), ContextOf(cax));
      if (!book) return InvokerResult{"", ffi::kStatusOkEmpty, false};
      return InvokerResult{book->SerializeAsString(), ffi::kStatusOk, true};
    };
  }

  template <class TState>
  static Invoker RejectionInvoker(ComponentKey key,
                                  typename AggregateDispatch<TState>::RejectionFn fn) {
    return [key, fn](Session& s, const std::string&, const std::string&, const std::string& aux) {
      abi::RejectionAux rax;
      rax.ParseFromString(aux);
      pb::Notification n;
      n.ParseFromString(rax.notification());
      pb::RejectionNotification rej;
      rej.ParseFromString(rax.rejection());
      auto resp = fn(n, rej, s.EnsureState<TState>(key), ContextOf(rax.cctx()));
      if (!resp) return InvokerResult{"", ffi::kStatusOkEmpty, false};
      return InvokerResult{resp->SerializeAsString(), ffi::kStatusOk, true};
    };
  }

  template <class TState>
  static Invoker UndoInvoker(ComponentKey key, typename AggregateDispatch<TState>::UndoFn fn) {
    return [key, fn](Session& s, const std::string&, const std::string&, const std::string& aux) {
      abi::UndoAux uax;
      uax.ParseFromString(aux);
      pb::Notification n;
      n.ParseFromString(uax.notification());
      pb::Compensate compensate;
      compensate.ParseFromString(uax.compensate());
      auto resp = fn(n, compensate, s.EnsureState<TState>(key), ContextOf(uax.cctx()));
      if (!resp) return InvokerResult{"", ffi::kStatusOkEmpty, false};
      return InvokerResult{resp->SerializeAsString(), ffi::kStatusOk, true};
    };
  }

  // A fact handler: the fact to record as a serialized Any, or STATUS_OK_EMPTY
  // to record the delivered fact unchanged.
  template <class TState>
  static Invoker FactInvoker(ComponentKey key, typename AggregateDispatch<TState>::FactFn fn) {
    return [key, fn](Session& s, const std::string& tu, const std::string& payload,
                     const std::string&) {
      auto fact = fn(AnyOf(tu, payload), s.EnsureState<TState>(key));
      if (!fact) return InvokerResult{"", ffi::kStatusOkEmpty, false};
      return InvokerResult{fact->SerializeAsString(), ffi::kStatusOk, true};
    };
  }

  // Packs the component's current session state as a serialized Any (Replay).
  template <class TState>
  static Invoker StateInvoker(ComponentKey key) {
    return [key](Session& s, const std::string&, const std::string&, const std::string&) {
      return InvokerResult{Pack::Wrap(s.EnsureState<TState>(key)).SerializeAsString(),
                           ffi::kStatusOk, true};
    };
  }

  template <class TState>
  static Invoker ProjectorEventInvoker(ComponentKey key,
                                       typename ProjectorDispatch<TState>::EventWithContextFn fn) {
    return [key, fn](Session& s, const std::string& tu, const std::string& payload,
                     const std::string& aux) {
      fn(s.EnsureState<TState>(key), AnyOf(tu, payload), PageOf(aux));
      return InvokerResult{"", ffi::kStatusOk, false};
    };
  }

  template <class TState>
  static Invoker ProjectorFinishInvoker(ComponentKey key,
                                        typename ProjectorDispatch<TState>::FinishFn fn) {
    return
        [key, fn](Session& s, const std::string&, const std::string& payload, const std::string&) {
          pb::EventBook book;
          book.ParseFromString(payload);
          auto proj = fn(s.EnsureState<TState>(key), book);
          return InvokerResult{proj.SerializeAsString(), ffi::kStatusOk, true};
        };
  }

  static Invoker ProjectorUnknownInvoker(ProjectorUnknownFn fn) {
    return [fn](Session&, const std::string& tu, const std::string&, const std::string&) {
      fn(tu);
      return InvokerResult{"", ffi::kStatusOk, false};
    };
  }

  static Invoker SagaEventInvoker(std::vector<std::string> targets, SagaDispatch::EventFn fn) {
    return
        [dests = Destinations(std::move(targets)), fn](
            Session&, const std::string& tu, const std::string& payload, const std::string& aux) {
          abi::SagaEventAux sax;
          sax.ParseFromString(aux);
          auto emission = fn(AnyOf(tu, payload), dests, sax.source_cover());
          pb::SagaResponse resp;
          for (auto& c : emission.commands) *resp.add_commands() = c;
          for (auto& e : emission.events) *resp.add_events() = e;
          return InvokerResult{resp.SerializeAsString(), ffi::kStatusOk, true};
        };
  }

  template <class TState>
  static Invoker PmEventInvoker(ComponentKey key, std::vector<std::string> targets,
                                typename ProcessManagerDispatch<TState>::EventWithCoverFn fn) {
    return [key, dests = Destinations(std::move(targets)), fn](Session& s, const std::string& tu,
                                                               const std::string& payload,
                                                               const std::string& aux) {
      abi::PmEventAux pax;
      pax.ParseFromString(aux);
      auto resp = fn(AnyOf(tu, payload), s.EnsureState<TState>(key), dests, pax.trigger_cover());
      return InvokerResult{resp.SerializeAsString(), ffi::kStatusOk, true};
    };
  }

  template <class TState>
  static Invoker PmRejectionInvoker(
      ComponentKey key, typename ProcessManagerDispatch<TState>::RejectionResponseFn fn) {
    return [key, fn](Session& s, const std::string&, const std::string&, const std::string& aux) {
      abi::RejectionAux rax;
      rax.ParseFromString(aux);
      pb::Notification n;
      n.ParseFromString(rax.notification());
      pb::RejectionNotification rej;
      rej.ParseFromString(rax.rejection());
      auto resp = fn(n, rej, s.EnsureState<TState>(key));
      return InvokerResult{resp.SerializeAsString(), ffi::kStatusOk, true};
    };
  }

  void* ptr_;
  std::map<uint64_t, Invoker> registry_;
  std::atomic<uint64_t> next_id_{0};
  std::atomic<ComponentKey> next_component_{0};
  std::mutex mu_;
};

// A coded failure as a callback outcome: the google.rpc.Status bytes and the
// negated wire gRPC code (never 0, which the core would read as success).
inline InvokerResult ErrorResult(const CodedError& e) {
  return InvokerResult{ToStatusBytes(e), -static_cast<int32_t>(WireGrpc(e.grpc)), true};
}

// The single host-callback gateway. One inline definition; passed by address to
// every registration. Catches every exception and codes it — never unwinds
// across the boundary.
extern "C" inline int32_t AngzarrTrampoline(void* host_ctx, uint64_t callback_id,
                                            const uint8_t* type_url, size_t type_url_len,
                                            const uint8_t* payload, size_t payload_len,
                                            const uint8_t* aux, size_t aux_len,
                                            ffi::AngzarrBuf* out) {
  auto read = [](const uint8_t* p, size_t n) {
    return (p && n) ? std::string(reinterpret_cast<const char*>(p), n) : std::string();
  };
  auto write_out = [](ffi::AngzarrBuf* o, const InvokerResult& r) {
    if (!o) return;
    if (!r.has_response || r.response.empty()) {
      o->data = nullptr;
      o->len = 0;
      return;
    }
    uint8_t* buf = ffi::angzarr_buf_alloc(r.response.size());
    std::memcpy(buf, r.response.data(), r.response.size());
    o->data = buf;
    o->len = r.response.size();
  };
  auto fail = [&](const CodedError& e) {
    InvokerResult r = ErrorResult(e);
    write_out(out, r);
    return r.status;
  };
  try {
    auto* session = static_cast<Session*>(host_ctx);
    Invoker* invoker = session ? session->router().InvokerFor(callback_id) : nullptr;
    if (invoker == nullptr) {
      return fail(CodedError::Unhandled("no host callback registered for id " +
                                        std::to_string(callback_id)));
    }
    InvokerResult result;
    try {
      result = (*invoker)(*session, read(type_url, type_url_len), read(payload, payload_len),
                          read(aux, aux_len));
    } catch (const CodedError& e) {
      result = ErrorResult(e);
    } catch (const std::exception& e) {
      result = ErrorResult(CodedError::Unhandled(e.what()));
    }
    write_out(out, result);
    return result.status;
  } catch (...) {
    return fail(CodedError::Unhandled("cpp callback gateway failed"));
  }
}

}  // namespace angzarr::router
