#pragma once

// The C++ binding's router: wraps the native router plus the host-side callback
// registry the core reaches through the single callback gateway. Register a
// component (assigning callback ids to its thunks and handing the core a
// serialized descriptor), then dispatch books/commands through it.
//
// The dispatch surfaces are generic in the component state message, so the
// generated wiring and handler thunks are statically typed. The one unavoidable
// erasing cast — the FFI registry is keyed by an opaque callback_id, not a type
// — lives in Session::EnsureState<TState>(), guaranteed correct because the same
// TState's invokers created the state. host_ctx is a Session* directly.

#include <atomic>
#include <cstdint>
#include <cstring>
#include <map>
#include <mutex>
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

class Router {
 public:
  Router() : ptr_(ffi::angzarr_router_new()) {}
  ~Router() { ffi::angzarr_router_free(ptr_); }
  Router(const Router&) = delete;
  Router& operator=(const Router&) = delete;

  Invoker* InvokerFor(uint64_t id) {
    std::lock_guard<std::mutex> lock(mu_);
    auto it = registry_.find(id);
    return it == registry_.end() ? nullptr : &it->second;
  }

  // --- registration --------------------------------------------------------

  template <class TState>
  void RegisterAggregate(AggregateDispatch<TState> d) {
    abi::AggregateDescriptor desc;
    desc.set_name(d.name);
    desc.set_domain(d.domain);
    for (auto& [fq, fn] : d.rebuilder.appliers) {
      auto* e = desc.add_appliers();
      e->set_fq_type(fq);
      e->set_callback_id(Assign(ApplierInvoker<TState>(fn)));
    }
    if (d.rebuilder.snapshot) {
      desc.set_snapshot_callback_id(Assign(ApplierInvoker<TState>(d.rebuilder.snapshot)));
    }
    for (auto& [fq, fn] : d.commands) {
      auto* e = desc.add_commands();
      e->set_fq_type(fq);
      e->set_callback_id(Assign(CommandInvoker<TState>(fn)));
    }
    for (auto& [fq, fns] : d.rejections) {
      auto* entry = desc.add_rejections();
      entry->set_fq_command_type(fq);
      for (auto& fn : fns) entry->add_callback_ids(Assign(RejectionInvoker<TState>(fn)));
    }
    Register(ffi::angzarr_router_register_aggregate, desc);
  }

  template <class TState>
  void RegisterProjector(ProjectorDispatch<TState> d) {
    abi::ProjectorDescriptor desc;
    desc.set_name(d.name);
    for (auto& dom : d.domains) desc.add_domains(dom);
    for (auto& [fq, fn] : d.events) {
      auto* e = desc.add_events();
      e->set_fq_type(fq);
      e->set_callback_id(Assign(ApplierInvoker<TState>(fn)));
    }
    if (d.finish) {
      desc.set_finish_callback_id(Assign(ProjectorFinishInvoker<TState>(d.finish)));
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
      e->set_callback_id(Assign(SagaEventInvoker(fn)));
    }
    for (auto& [fq, fns] : d.rejections) {
      auto* entry = desc.add_rejections();
      entry->set_fq_command_type(fq);
      for (auto& fn : fns) entry->add_callback_ids(Assign(SagaRejectionInvoker(fn)));
    }
    Register(ffi::angzarr_router_register_saga, desc);
  }

  template <class TState>
  void RegisterProcessManager(ProcessManagerDispatch<TState> d) {
    abi::ProcessManagerDescriptor desc;
    desc.set_name(d.name);
    desc.set_pm_domain(d.pm_domain);
    for (auto& [fq, fn] : d.rebuilder.appliers) {
      auto* e = desc.add_appliers();
      e->set_fq_type(fq);
      e->set_callback_id(Assign(ApplierInvoker<TState>(fn)));
    }
    if (d.rebuilder.snapshot) {
      desc.set_snapshot_callback_id(Assign(ApplierInvoker<TState>(d.rebuilder.snapshot)));
    }
    for (auto& h : d.handlers) {
      auto* e = desc.add_events();
      e->set_input_domain(h.source_domain);
      e->set_fq_type(h.full_name);
      e->set_callback_id(Assign(PmEventInvoker<TState>(h.fn)));
    }
    for (auto& [fq, fns] : d.rejections) {
      auto* entry = desc.add_rejections();
      entry->set_fq_command_type(fq);
      for (auto& fn : fns) entry->add_callback_ids(Assign(PmRejectionInvoker<TState>(fn)));
    }
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

 private:
  struct Dispatched {
    std::string response;
    int32_t status;
  };

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

  static google::protobuf::Any AnyOf(const std::string& type_url, const std::string& payload) {
    google::protobuf::Any any;
    any.set_type_url(type_url);
    any.set_value(payload);
    return any;
  }

  // --- invoker adapters (the lone TState cast lives in EnsureState) --------

  template <class TState>
  static Invoker ApplierInvoker(std::function<void(TState&, const google::protobuf::Any&)> fn) {
    return [fn](Session& s, const std::string& tu, const std::string& payload, const std::string&) {
      fn(s.EnsureState<TState>(), AnyOf(tu, payload));
      return InvokerResult{"", ffi::kStatusOk, false};
    };
  }

  template <class TState>
  static Invoker CommandInvoker(typename AggregateDispatch<TState>::CommandFn fn) {
    return [fn](Session& s, const std::string& tu, const std::string& payload,
                const std::string& aux) {
      abi::CommandContextAux cax;
      cax.ParseFromString(aux);
      CommandContext cctx{cax.next_sequence(), cax.had_prior_events()};
      auto book = fn(AnyOf(tu, payload), s.EnsureState<TState>(), cctx);
      return InvokerResult{book.SerializeAsString(), ffi::kStatusOk, true};
    };
  }

  template <class TState>
  static Invoker RejectionInvoker(typename AggregateDispatch<TState>::RejectionFn fn) {
    return [fn](Session& s, const std::string&, const std::string&, const std::string& aux) {
      abi::RejectionAux rax;
      rax.ParseFromString(aux);
      pb::Notification n;
      n.ParseFromString(rax.notification());
      pb::RejectionNotification rej;
      rej.ParseFromString(rax.rejection());
      CommandContext cctx{};
      if (rax.has_cctx()) {
        cctx.next_sequence = rax.cctx().next_sequence();
        cctx.had_prior_events = rax.cctx().had_prior_events();
      }
      auto resp = fn(n, rej, s.EnsureState<TState>(), cctx);
      return InvokerResult{resp.SerializeAsString(), ffi::kStatusOk, true};
    };
  }

  template <class TState>
  static Invoker ProjectorFinishInvoker(typename ProjectorDispatch<TState>::FinishFn fn) {
    return [fn](Session& s, const std::string&, const std::string& payload, const std::string&) {
      pb::EventBook book;
      book.ParseFromString(payload);
      auto proj = fn(s.EnsureState<TState>(), book);
      return InvokerResult{proj.SerializeAsString(), ffi::kStatusOk, true};
    };
  }

  static Invoker SagaEventInvoker(SagaDispatch::EventFn fn) {
    return
        [fn](Session&, const std::string& tu, const std::string& payload, const std::string& aux) {
          abi::SagaEventAux sax;
          sax.ParseFromString(aux);
          std::map<std::string, uint32_t> seqs;
          for (const auto& kv : sax.destination_sequences()) seqs[kv.first] = kv.second;
          Destinations dests(std::move(seqs));
          auto emission = fn(AnyOf(tu, payload), dests);
          pb::SagaResponse resp;
          for (auto& c : emission.commands) *resp.add_commands() = c;
          for (auto& e : emission.events) *resp.add_events() = e;
          return InvokerResult{resp.SerializeAsString(), ffi::kStatusOk, true};
        };
  }

  static Invoker SagaRejectionInvoker(SagaDispatch::RejectionFn fn) {
    return [fn](Session&, const std::string&, const std::string&, const std::string& aux) {
      abi::RejectionAux rax;
      rax.ParseFromString(aux);
      pb::Notification n;
      n.ParseFromString(rax.notification());
      pb::RejectionNotification rej;
      rej.ParseFromString(rax.rejection());
      pb::SagaResponse resp;
      for (auto& e : fn(n, rej)) *resp.add_events() = e;
      return InvokerResult{resp.SerializeAsString(), ffi::kStatusOk, true};
    };
  }

  template <class TState>
  static Invoker PmEventInvoker(typename ProcessManagerDispatch<TState>::EventFn fn) {
    return [fn](Session& s, const std::string& tu, const std::string& payload,
                const std::string& aux) {
      abi::PmEventAux pax;
      pax.ParseFromString(aux);
      std::map<std::string, uint32_t> seqs;
      for (const auto& kv : pax.destination_sequences()) seqs[kv.first] = kv.second;
      Destinations dests(std::move(seqs));
      auto resp = fn(AnyOf(tu, payload), s.EnsureState<TState>(), dests);
      return InvokerResult{resp.SerializeAsString(), ffi::kStatusOk, true};
    };
  }

  template <class TState>
  static Invoker PmRejectionInvoker(typename ProcessManagerDispatch<TState>::RejectionFn fn) {
    return [fn](Session& s, const std::string&, const std::string&, const std::string& aux) {
      abi::RejectionAux rax;
      rax.ParseFromString(aux);
      pb::Notification n;
      n.ParseFromString(rax.notification());
      pb::RejectionNotification rej;
      rej.ParseFromString(rax.rejection());
      auto r = fn(n, rej, s.EnsureState<TState>());
      pb::ProcessManagerHandleResponse resp;
      for (auto& e : r.process_events) *resp.add_process_events() = e;
      if (r.escalation) *resp.mutable_notification() = *r.escalation;
      return InvokerResult{resp.SerializeAsString(), ffi::kStatusOk, true};
    };
  }

  void* ptr_;
  std::map<uint64_t, Invoker> registry_;
  std::atomic<uint64_t> next_id_{0};
  std::mutex mu_;
};

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
    InvokerResult r{ToStatusBytes(e), -static_cast<int32_t>(e.grpc), true};
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
      result = {ToStatusBytes(e), -static_cast<int32_t>(e.grpc), true};
    } catch (const std::exception& e) {
      CodedError ce = CodedError::Unhandled(e.what());
      result = {ToStatusBytes(ce), -static_cast<int32_t>(ce.grpc), true};
    }
    write_out(out, result);
    return result.status;
  } catch (...) {
    return fail(CodedError::Unhandled("cpp callback gateway failed"));
  }
}

}  // namespace angzarr::router
