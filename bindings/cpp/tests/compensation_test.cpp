#include <catch2/catch.hpp>
#include <string>
#include <vector>

#include "angzarr/router/router.h"
#include "helpers.h"
#include "test/counter/counter.pb.h"

namespace {

using namespace angzarr::router;
using namespace angzarr::router::testing;
namespace tc = test::counter;

pb::BusinessResponse OnePage(const std::string& fq) {
  pb::BusinessResponse resp;
  *resp.mutable_events()->add_pages()->mutable_event() = EmptyAny(fq);
  return resp;
}

}  // namespace

TEST_CASE("two compensators for one rejected command fan out in registration order",
          "[compensation]") {
  Router router;
  std::vector<std::string> ran;
  AggregateDispatch<tc::CounterState> d("Counter", "counter", Rebuilder<tc::CounterState>{});
  d.OnRejected("test.counter.Reserve",
               [&ran](const pb::Notification&, const pb::RejectionNotification&, tc::CounterState&,
                      const CommandContext&) {
                 ran.push_back("first");
                 return OnePage("test.counter.CompensatedFirst");
               });
  d.OnRejected("test.counter.Reserve",
               [&ran](const pb::Notification&, const pb::RejectionNotification&, tc::CounterState&,
                      const CommandContext&) {
                 ran.push_back("second");
                 return OnePage("test.counter.CompensatedSecond");
               });
  router.RegisterAggregate(std::move(d));

  auto resp = router.Dispatch(RejectionOf("test.counter.Reserve"));
  REQUIRE(ran == std::vector<std::string>{"first", "second"});
  REQUIRE(resp.events().pages_size() == 2);
  REQUIRE(resp.events().pages(0).event().type_url() == "/test.counter.CompensatedFirst");
  REQUIRE(resp.events().pages(1).event().type_url() == "/test.counter.CompensatedSecond");
}
