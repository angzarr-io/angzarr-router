#include <catch2/catch.hpp>
#include <string>
#include <vector>

#include "angzarr/router/router.h"
#include "helpers.h"
#include "test/counter/counter.pb.h"

using namespace angzarr::router;
using namespace angzarr::router::testing;
namespace tc = test::counter;

TEST_CASE("a projector observes events with no fold through OnUnknown", "[projector]") {
  Router router;
  std::vector<std::string> unknown;
  int folded = 0;
  ProjectorDispatch<tc::CounterProjectorState> d("counter-projector");
  d.ForDomains({"counter"})
      .OnEvent("test.counter.Increased",
               [&folded](tc::CounterProjectorState&, const google::protobuf::Any&) { ++folded; })
      .OnUnknown([&unknown](const std::string& type_url) { unknown.push_back(type_url); });
  router.RegisterProjector(std::move(d));

  pb::EventBook book;
  book.mutable_cover()->set_domain("counter");
  *book.add_pages()->mutable_event() = EmptyAny("test.counter.Increased");
  *book.add_pages()->mutable_event() = EmptyAny("test.counter.Unwatched");
  router.DispatchProjector(book);

  REQUIRE(folded == 1);
  REQUIRE(unknown == std::vector<std::string>{"/test.counter.Unwatched"});
}
