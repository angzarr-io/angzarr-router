#include <catch2/catch.hpp>
#include <optional>

#include "angzarr/router/router.h"
#include "helpers.h"
#include "test/counter/counter.pb.h"

using namespace angzarr::router;
using namespace angzarr::router::testing;
namespace tc = test::counter;

TEST_CASE("a command handler returning no result yields an empty, coverless book", "[status]") {
  Router router;
  AggregateDispatch<tc::CounterState> d("Counter", "counter", Rebuilder<tc::CounterState>{});
  d.OnCommand("test.counter.IncreaseBy",
              [](const google::protobuf::Any&, tc::CounterState&,
                 const CommandContext&) -> std::optional<pb::EventBook> { return std::nullopt; });
  router.RegisterAggregate(std::move(d));

  auto cc = CommandOf("test.counter.IncreaseBy");
  *cc.mutable_command()->mutable_cover()->mutable_ext() = EmptyAny("test.counter.Parent");
  auto resp = router.Dispatch(cc);
  REQUIRE(resp.has_events());
  REQUIRE(resp.events().pages_size() == 0);
  REQUIRE_FALSE(resp.events().has_cover());
}

TEST_CASE("a compensator returning no result contributes nothing to the fan-out", "[status]") {
  Router router;
  AggregateDispatch<tc::CounterState> d("Counter", "counter", Rebuilder<tc::CounterState>{});
  d.OnRejected(
      "test.counter.Reserve",
      [](const pb::Notification&, const pb::RejectionNotification&, tc::CounterState&,
         const CommandContext&) -> std::optional<pb::BusinessResponse> { return std::nullopt; });
  router.RegisterAggregate(std::move(d));

  auto resp = router.Dispatch(RejectionOf("test.counter.Reserve"));
  REQUIRE(resp.result_case() == pb::BusinessResponse::RESULT_NOT_SET);
}
