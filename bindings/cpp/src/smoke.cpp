// Smoke check: register the CounterAggregate seam and round-trip one IncreaseBy
// through the staticlib. Proves the binding + generated wiring compile,
// instantiate, link, and dispatch before the full conformance suite.

#include <cstdint>
#include <iostream>
#include <vector>

#include "test/counter/counter_aggregate_angzarr.h"

namespace {

class Counter : public test::counter::CounterAggregateHandler {
 public:
  std::vector<test::counter::Increased> IncreaseBy(
      const test::counter::IncreaseBy& cmd, test::counter::CounterState& state,
      const angzarr::router::CommandContext& cctx) override {
    (void)state;
    (void)cctx;
    std::vector<test::counter::Increased> out;
    for (uint32_t i = 0; i < cmd.n(); ++i) out.emplace_back();
    return out;
  }
  io::angzarr::v1::EventBook FailHard(const test::counter::FailHard&, test::counter::CounterState&,
                                      const angzarr::router::CommandContext&) override {
    throw std::runtime_error("hard failure");
  }
  void ApplyIncreased(test::counter::CounterState& state,
                      const test::counter::Increased&) override {
    state.set_count(state.count() + 1);
  }
  io::angzarr::v1::BusinessResponse OnReserveRejected(
      const io::angzarr::v1::Notification&, const io::angzarr::v1::RejectionNotification&,
      test::counter::CounterState&, const angzarr::router::CommandContext&) override {
    return {};
  }
};

}  // namespace

int main() {
  angzarr::router::Router router;
  Counter fixture;
  test::counter::RegisterCounterAggregate(router, fixture);

  io::angzarr::v1::ContextualCommand cc;
  cc.mutable_command()->mutable_cover()->set_domain("counter");
  test::counter::IncreaseBy ib;
  ib.set_n(3);
  auto* page = cc.mutable_command()->add_pages();
  page->mutable_command()->set_type_url("/test.counter.IncreaseBy");
  page->mutable_command()->set_value(ib.SerializeAsString());

  auto resp = router.Dispatch(cc);
  std::cout << "events=" << resp.events().pages_size() << "\n";
  return resp.events().pages_size() == 3 ? 0 : 1;
}
