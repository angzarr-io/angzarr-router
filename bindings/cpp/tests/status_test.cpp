#include <catch2/catch.hpp>

#include "angzarr/router/router.h"
#include "helpers.h"
#include "test/counter/counter.pb.h"

namespace {

using namespace angzarr::router;
using namespace angzarr::router::testing;
namespace tc = test::counter;

}  // namespace

TEST_CASE("a coded error carrying gRPC OK fails as INVALID_ARGUMENT", "[status]") {
  Router router;
  AggregateDispatch<tc::CounterState> d("Counter", "counter", Rebuilder<tc::CounterState>{});
  d.OnCommand(
      "test.counter.FailHard",
      [](const google::protobuf::Any&, tc::CounterState&, const CommandContext&) -> pb::EventBook {
        throw CodedError("ZERO_CODED", "rejected with grpc 0", static_cast<GrpcCode>(0));
      });
  router.RegisterAggregate(std::move(d));
  try {
    router.Dispatch(CommandOf("test.counter.FailHard"));
    FAIL("a CodedError with gRPC 0 dispatched as success");
  } catch (const CodedError& e) {
    REQUIRE(e.code == "ZERO_CODED");
    REQUIRE(e.grpc == GrpcCode::kInvalidArgument);
  }
}

TEST_CASE("a coded error carrying gRPC OK serializes as INVALID_ARGUMENT", "[status]") {
  google::rpc::Status status;
  REQUIRE(status.ParseFromString(
      ToStatusBytes(CodedError("ZERO_CODED", "m", static_cast<GrpcCode>(0)))));
  REQUIRE(status.code() == static_cast<int32_t>(GrpcCode::kInvalidArgument));
}
