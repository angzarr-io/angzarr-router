#include <google/protobuf/io/coded_stream.h>
#include <google/protobuf/io/zero_copy_stream_impl_lite.h>

#include <catch2/catch.hpp>
#include <optional>
#include <string>
#include <vector>

#include "angzarr/router/router.h"
#include "builders.h"
#include "gherkin.h"
#include "sha256.h"
#include "test/counter/order_saga_angzarr.h"

namespace {
using namespace angzarr::conformance;
using angzarr::router::CodedError;
using angzarr::router::Destinations;
using angzarr::router::GrpcCode;
using angzarr::router::PageContext;
using angzarr::router::SagaDispatch;
using angzarr::router::SagaEmission;

// The conformance OrderSaga fixture: a declared source event emits one Reserve
// command for "inventory" (deferred: the router stamps its provenance).
class SagaFixture : public tc::OrderSagaHandler {
 public:
  SagaEmission Increased(const tc::Increased&, const Destinations&, const PageContext&) override {
    return {{ReserveCommand()}, {}};
  }
};

// The OrderSaga Increased handler through the context-taking registration: it
// records each triggering event's sequence in seen, then runs fixture.
SagaDispatch::EventWithContextFn RecordingIncreased(SagaFixture& fixture,
                                                    std::vector<uint32_t>& seen) {
  return [&fixture, &seen](const google::protobuf::Any& event_any, const Destinations& dests,
                           const PageContext& source) {
    seen.push_back(source.sequence);
    return fixture.Increased(CodedError::Parse<tc::Increased>(event_any), dests, source);
  };
}

// The OrderSaga dispatch delivering to target, recording into seen.
SagaDispatch RecordingSaga(SagaFixture& fixture, const std::string& target,
                           std::vector<uint32_t>& seen) {
  SagaDispatch saga("OrderSaga", "order", {target});
  saga.OnEventWithContext("test.counter.Increased", RecordingIncreased(fixture, seen));
  return saga;
}

// The parity saga ("order" -> "inventory"): its Increased handler emits the
// parity command twice.
SagaDispatch ParitySaga() {
  SagaDispatch saga("parity-saga", "order", {"inventory"});
  saga.OnEvent("test.counter.Increased",
               [](const google::protobuf::Any&, const Destinations&, const pb::Cover&) {
                 return SagaEmission{{ParityCommand(), ParityCommand()}, {}};
               });
  return saga;
}

// The lowercase hex SHA-256 of cmd's deterministic encoding, unknown fields
// discarded.
std::string CommandHash(const pb::CommandBook& emitted) {
  pb::CommandBook cmd = emitted;
  cmd.DiscardUnknownFields();
  std::string bytes;
  {
    google::protobuf::io::StringOutputStream sink(&bytes);
    google::protobuf::io::CodedOutputStream out(&sink);
    out.SetSerializationDeterministic(true);
    REQUIRE(cmd.SerializeToCodedStream(&out));
  }
  return Sha256Hex(bytes);
}

// A raw saga registration's return code and the coded error it surfaces.
struct Registration {
  int32_t ret;
  std::optional<CodedError> err;
};

// Registers a hand-built SagaDescriptor declaring a Reserve rejection handler
// through the binding's raw FFI registration (SagaDispatch cannot declare one)
// on a fresh native router.
Registration RegisterCompensatingSaga() {
  angzarr::router::abi::SagaDescriptor desc;
  desc.set_name("order-saga");
  desc.set_input_domain("order");
  desc.add_target_domains("inventory");
  auto* rejection = desc.add_rejections();
  rejection->set_compensates("test.counter.Reserve");
  rejection->add_callback_ids(1);
  const std::string bytes = desc.SerializeAsString();
  void* router = angzarr::router::ffi::angzarr_router_new();
  const int32_t ret = angzarr::router::ffi::angzarr_router_register_saga(
      router, reinterpret_cast<const uint8_t*>(bytes.data()), bytes.size(),
      &angzarr::router::AngzarrTrampoline);
  angzarr::router::ffi::angzarr_router_free(router);
  Registration out{ret, std::nullopt};
  if (ret != 0) out.err = angzarr::router::FromStatusBytes("", ret);
  return out;
}

struct SagaWorld {
  angzarr::router::Router router;
  SagaFixture fixture;
  std::vector<uint32_t> seen;
  std::optional<pb::SagaResponse> resp;
  std::optional<CodedError> err;
  std::optional<Registration> registration;

  void Dispatch(pb::SagaHandleRequest req) {
    try {
      resp = router.DispatchSaga(req);
      err.reset();
    } catch (const CodedError& e) {
      err = e;
      resp.reset();
    }
  }
};

void Register(StepRegistry& r, SagaWorld& w) {
  r.On("an order saga delivering to {string}",
       [&w](const StepArgs& a) { w.router.RegisterSaga(RecordingSaga(w.fixture, a[0], w.seen)); });
  r.On("an Increased event at sequence {int} is dispatched", [&w](const StepArgs& a) {
    w.Dispatch(SagaEventSource("test.counter.Increased", static_cast<uint32_t>(std::stoi(a[0]))));
  });
  r.On("a Reserve event is dispatched",
       [&w](const StepArgs&) { w.Dispatch(SagaEventSource("test.counter.Reserve")); });
  r.On("a source with no pages is dispatched",
       [&w](const StepArgs&) { w.Dispatch(SagaSourceNoPages()); });
  r.On("a request with no source is dispatched",
       [&w](const StepArgs&) { w.Dispatch(SagaRequestNoSource()); });
  r.On("a rejection of Reserve is dispatched",
       [&w](const StepArgs&) { w.Dispatch(SagaRejectionSource("test.counter.Reserve")); });

  r.On("the saga emits one command to {string}", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->commands_size() == 1);
    REQUIRE(w.resp->commands(0).cover().domain() == a[0]);
  });
  r.On("the command is deferred from source sequence {int} at command index {int}",
       [&w](const StepArgs& a) { RequireDeferred(w.resp->commands(0), "order", a); });
  r.On("a parity saga emitting the parity command twice",
       [&w](const StepArgs&) { w.router.RegisterSaga(ParitySaga()); });
  r.On("an Increased event of order root {string} at sequence {int} is dispatched",
       [&w](const StepArgs& a) {
         w.Dispatch(SagaRootedSource(a[0], static_cast<uint32_t>(std::stoi(a[1]))));
       });
  r.On("the parity source event at sequence {int} is dispatched", [&w](const StepArgs& a) {
    w.Dispatch(ParitySource(static_cast<uint32_t>(std::stoi(a[0]))));
  });
  r.On("a saga declaring a compensation for Reserve is registered",
       [&w](const StepArgs&) { w.registration = RegisterCompensatingSaga(); });
  r.On("the registration is refused as INVALID_ARGUMENT", [&w](const StepArgs&) {
    REQUIRE(w.registration.has_value());
    REQUIRE(w.registration->ret == -3);
    REQUIRE(w.registration->err.has_value());
    REQUIRE(w.registration->err->grpc == GrpcCode::kInvalidArgument);
  });
  r.On("the command leaves its source component to the coordinator", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    RequireNoSourceComponent(w.resp->commands(0));
  });
  r.On("the command is deferred from order root {string}", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->commands_size() == 1);
    const std::string want = CoverOf("order", a[0]).SerializeAsString();
    for (const auto& page : w.resp->commands(0).pages()) {
      REQUIRE(page.header().sequence_type_case() == pb::PageHeader::kAngzarrDeferred);
      REQUIRE(page.header().angzarr_deferred().source().SerializeAsString() == want);
    }
  });
  r.On("the command at index {int} hashes to SHA-256 {string}", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(CommandHash(w.resp->commands(std::stoi(a[0]))) == a[1]);
  });
  r.On("the saga handler saw source sequence {int}", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.seen == std::vector<uint32_t>{static_cast<uint32_t>(std::stoi(a[0]))});
  });
  r.On("the saga emits no commands", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->commands_size() == 0);
  });
  r.On("the dispatch fails with {word}", [&w](const StepArgs& a) {
    REQUIRE(w.err.has_value());
    REQUIRE(w.err->code == a[0]);
  });
  r.On("the saga injects no events", [&w](const StepArgs&) {
    REQUIRE_FALSE(w.err.has_value());
    REQUIRE(w.resp->events_size() == 0);
  });
}

}  // namespace

TEST_CASE("order saga dispatch", "[saga]") {
  for (auto& sc : ParseFeature(FeaturePath("saga.feature"))) {
    DYNAMIC_SECTION(sc.name) {
      SagaWorld world;
      StepRegistry reg;
      Register(reg, world);
      for (const auto& step : sc.steps) reg.Run(step.text);
    }
  }
}
