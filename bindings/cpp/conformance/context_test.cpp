#include <catch2/catch.hpp>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

#include "angzarr/router/router.h"
#include "builders.h"
#include "gherkin.h"
#include "uuid5.h"

namespace {
using namespace angzarr::conformance;
using angzarr::router::AggregateDispatch;
using angzarr::router::CodedError;
using angzarr::router::CommandContext;
using angzarr::router::Destinations;
using angzarr::router::FactRecord;
using angzarr::router::GrpcCode;
using angzarr::router::PageContext;
using angzarr::router::ProcessManagerDispatch;
using angzarr::router::ProjectorDispatch;
using angzarr::router::Rebuilder;

pb::EventPage IncreasedAt(uint32_t seq) {
  pb::EventPage page;
  page.mutable_header()->set_sequence(seq);
  SetAnyEmpty(page.mutable_event(), "test.counter.Increased");
  return page;
}

struct ContextWorld {
  angzarr::router::Router router;
  std::vector<pb::Cover> covers;
  std::vector<std::pair<std::string, uint32_t>> pages;
  std::vector<uint32_t> applied;
  std::optional<pb::EventBook> facts;
  std::optional<CodedError> fact_err;
  std::optional<pb::ReplayResponse> replayed;
  std::optional<pb::ProcessManagerHandleResponse> pm;
  std::optional<pb::BusinessResponse> command;

  // The ledger aggregate (domain "ledger") over CounterState: Increased folds
  // count += 1 and records the page sequence it applied; a snapshot loads
  // CounterState; IncreaseBy records the handled cover and emits one Increased
  // whose cover carries the ledger's own linkage; the only declared fact,
  // Increased, is recorded as received and flagged by a CounterState carrying
  // the count it brings the ledger to.
  void RegisterLedger() {
    Rebuilder<tc::CounterState> rebuilder;
    rebuilder
        .ApplyWithContext(
            "test.counter.Increased",
            [this](tc::CounterState& state, const google::protobuf::Any&, const PageContext& ctx) {
              state.set_count(state.count() + 1);
              applied.push_back(ctx.sequence);
            })
        .WithSnapshot([](tc::CounterState& state, const google::protobuf::Any& any) {
          if (!state.ParseFromString(any.value())) throw std::runtime_error("decode snapshot");
        });
    AggregateDispatch<tc::CounterState> agg("Ledger", "ledger", std::move(rebuilder));
    agg.OnCommand("test.counter.IncreaseBy",
                  [this](const google::protobuf::Any&, tc::CounterState&,
                         const CommandContext& cctx) -> std::optional<pb::EventBook> {
                    covers.push_back(cctx.cover);
                    pb::EventBook book;
                    *book.mutable_cover()->mutable_ext() = LedgerLinkage();
                    SetAnyEmpty(book.add_pages()->mutable_event(), "test.counter.Increased");
                    return book;
                  })
        .OnFact("test.counter.Increased",
                [](const google::protobuf::Any& fact, const tc::CounterState& state) {
                  tc::CounterState flag;
                  flag.set_count(state.count() + 1);
                  return FactRecord(fact, {angzarr::router::Pack::Wrap(flag)});
                });
    router.RegisterAggregate(std::move(agg));
  }

  // The reserving process-manager (domain "reserving-pm", target "inventory")
  // over CounterState (Increased folds count += 1): an Increased trigger from
  // "counter" records the trigger cover and emits nothing; a rejected Reserve
  // is compensated with a Release command to "inventory".
  void RegisterReservingPm() {
    Rebuilder<tc::CounterState> rebuilder;
    rebuilder.Apply("test.counter.Increased",
                    [](tc::CounterState& state, const google::protobuf::Any&) {
                      state.set_count(state.count() + 1);
                    });
    ProcessManagerDispatch<tc::CounterState> pmd("Reserving", "reserving-pm", {"inventory"},
                                                 std::move(rebuilder));
    pmd.OnEventWithCover("counter", "test.counter.Increased",
                         [this](const google::protobuf::Any&, tc::CounterState&,
                                const Destinations&, const pb::Cover& trigger_cover) {
                           covers.push_back(trigger_cover);
                           return pb::ProcessManagerHandleResponse{};
                         })
        .OnRejectedWithResponse(
            "test.counter.Reserve",
            [](const pb::Notification&, const pb::RejectionNotification&, tc::CounterState&) {
              auto release = ReserveCommand();
              SetAnyEmpty(release.mutable_pages(0)->mutable_command(), "test.counter.Release");
              pb::ProcessManagerHandleResponse resp;
              *resp.add_commands() = release;
              return resp;
            });
    router.RegisterProcessManager(std::move(pmd));
  }

  // The tracking projector: every Increased fold records its book's root and
  // the page's sequence.
  void RegisterTracker() {
    ProjectorDispatch<tc::CounterProjectorState> proj("Tracker");
    proj.OnEventWithContext(
        "test.counter.Increased",
        [this](tc::CounterProjectorState&, const google::protobuf::Any&, const PageContext& ctx) {
          pages.emplace_back(ctx.cover.root().value(), ctx.sequence);
        });
    router.RegisterProjector(std::move(proj));
  }

  // A FactRequest of facts pages of test.counter.<fact> in "ledger" over prior
  // Increased events.
  void HandleFacts(const std::string& fact, int count, int prior) {
    pb::FactRequest req;
    auto* book = req.mutable_facts();
    book->mutable_cover()->set_domain("ledger");
    for (int i = 0; i < count; ++i) {
      SetAnyEmpty(book->add_pages()->mutable_event(), "test.counter." + fact);
    }
    auto* history = req.mutable_prior_events();
    for (int i = 0; i < prior; ++i) *history->add_pages() = IncreasedAt(static_cast<uint32_t>(i));
    history->set_next_sequence(static_cast<uint32_t>(prior));
    try {
      facts = router.DispatchFact(req);
      fact_err.reset();
    } catch (const CodedError& e) {
      fact_err = e;
      facts.reset();
    }
  }
};

void Register(StepRegistry& r, ContextWorld& w) {
  r.On("a ledger aggregate", [&w](const StepArgs&) { w.RegisterLedger(); });
  r.On("a reserving process-manager", [&w](const StepArgs&) { w.RegisterReservingPm(); });
  r.On("a tracking projector", [&w](const StepArgs&) { w.RegisterTracker(); });

  r.On("{int} Increased facts are handled over {int} prior Increased events",
       [&w](const StepArgs& a) { w.HandleFacts("Increased", std::stoi(a[0]), std::stoi(a[1])); });
  r.On("a Reserve fact is handled over no prior events",
       [&w](const StepArgs&) { w.HandleFacts("Reserve", 1, 0); });
  r.On("the ledger replays a snapshot of {int} then {int} Increased events",
       [&w](const StepArgs& a) {
         pb::ReplayRequest req;
         auto* snap = req.mutable_base_snapshot();
         snap->set_sequence(1);
         tc::CounterState state;
         state.set_count(static_cast<uint32_t>(std::stoi(a[0])));
         *snap->mutable_state() = angzarr::router::Pack::Wrap(state);
         for (int i = 0; i < std::stoi(a[1]); ++i) {
           *req.add_events() = IncreasedAt(static_cast<uint32_t>(2 + i));
         }
         w.replayed = w.router.DispatchReplay("ledger", req);
       });
  r.On("the reserving process-manager replays {int} Increased events", [&w](const StepArgs& a) {
    pb::ReplayRequest req;
    for (int i = 0; i < std::stoi(a[0]); ++i) {
      *req.add_events() = IncreasedAt(static_cast<uint32_t>(i));
    }
    w.replayed = w.router.DispatchReplay("reserving-pm", req);
  });
  auto ledger_command = [](const std::string& label) {
    pb::ContextualCommand cc;
    auto* book = cc.mutable_command();
    *book->mutable_cover() = CoverOf("ledger", label);
    tc::IncreaseBy ib;
    ib.set_n(1);
    SetAny(book->add_pages()->mutable_command(), "test.counter.IncreaseBy", ib.SerializeAsString());
    return cc;
  };
  r.On("an IncreaseBy command for ledger root {string} is dispatched",
       [&w, ledger_command](const StepArgs& a) {
         w.command = w.router.Dispatch(ledger_command(a[0]));
       });
  r.On("an IncreaseBy command for ledger root {string} on behalf of a parent is dispatched",
       [&w, ledger_command](const StepArgs& a) {
         auto cc = ledger_command(a[0]);
         *cc.mutable_command()->mutable_cover()->mutable_ext() = ParentLinkage();
         w.command = w.router.Dispatch(cc);
       });
  r.On(
      "an Increased trigger of counter root {string} at sequence {int} is dispatched to the "
      "reserving process-manager",
      [&w](const StepArgs& a) {
        pb::ProcessManagerHandleRequest req;
        auto* trigger = req.mutable_trigger();
        *trigger->mutable_cover() = CoverOf("counter", a[0]);
        *trigger->add_pages() = IncreasedAt(static_cast<uint32_t>(std::stoi(a[1])));
        w.pm = w.router.DispatchProcessManager(req);
      });
  r.On(
      "a rejection of Reserve sent to {string} at sequence {int} is dispatched to the reserving "
      "process-manager",
      [&w](const StepArgs& a) {
        auto rejected = ReserveCommand();
        rejected.mutable_cover()->set_domain(a[0]);
        pb::RejectionNotification rejection;
        *rejection.mutable_rejected_command() = rejected;
        pb::Notification n;
        *n.mutable_payload() = angzarr::router::Pack::Wrap(rejection);
        pb::ProcessManagerHandleRequest req;
        auto* trigger = req.mutable_trigger();
        trigger->mutable_cover()->set_domain("reserving-pm");
        auto* page = trigger->add_pages();
        page->mutable_header()->set_sequence(static_cast<uint32_t>(std::stoi(a[1])));
        *page->mutable_event() = angzarr::router::Pack::Wrap(n);
        w.pm = w.router.DispatchProcessManager(req);
      });
  r.On("Increased events of counter root {string} at sequences {int} and {int} are projected",
       [&w](const StepArgs& a) {
         pb::EventBook book;
         *book.mutable_cover() = CoverOf("counter", a[0]);
         *book.add_pages() = IncreasedAt(static_cast<uint32_t>(std::stoi(a[1])));
         *book.add_pages() = IncreasedAt(static_cast<uint32_t>(std::stoi(a[2])));
         w.router.DispatchProjector(book);
       });

  r.On("each Increased fact is recorded, flagged by the counts {int} and {int}",
       [&w](const StepArgs& a) {
         REQUIRE_FALSE(w.fact_err.has_value());
         REQUIRE(w.facts.has_value());
         std::vector<std::pair<std::string, int>> recorded;
         for (const auto& page : w.facts->pages()) {
           const std::string type = FqFromUrl(page.event().type_url());
           int count = -1;
           if (type == "test.counter.CounterState") {
             tc::CounterState state;
             REQUIRE(state.ParseFromString(page.event().value()));
             count = static_cast<int>(state.count());
           }
           recorded.emplace_back(type, count);
         }
         const std::vector<std::pair<std::string, int>> expected{
             {"test.counter.Increased", -1},
             {"test.counter.CounterState", std::stoi(a[0])},
             {"test.counter.Increased", -1},
             {"test.counter.CounterState", std::stoi(a[1])},
         };
         REQUIRE(recorded == expected);
       });
  r.On("the facts are refused with {word} as INVALID_ARGUMENT", [&w](const StepArgs& a) {
    REQUIRE_FALSE(w.facts.has_value());
    REQUIRE(w.fact_err.has_value());
    REQUIRE(w.fact_err->code == a[0]);
    REQUIRE(w.fact_err->grpc == GrpcCode::kInvalidArgument);
  });
  r.On("the replayed state has a count of {int}", [&w](const StepArgs& a) {
    REQUIRE(w.replayed.has_value());
    REQUIRE(FqFromUrl(w.replayed->state().type_url()) == "test.counter.CounterState");
    tc::CounterState state;
    REQUIRE(state.ParseFromString(w.replayed->state().value()));
    REQUIRE(static_cast<int>(state.count()) == std::stoi(a[0]));
  });
  r.On("the ledger applied Increased events at sequences {int} and {int}", [&w](const StepArgs& a) {
    const std::vector<uint32_t> want = {static_cast<uint32_t>(std::stoi(a[0])),
                                        static_cast<uint32_t>(std::stoi(a[1]))};
    REQUIRE(w.applied == want);
  });
  r.On("the recorded event carries the ledger's own linkage", [&w](const StepArgs&) {
    REQUIRE(w.command.has_value());
    REQUIRE(w.command->has_events());
    REQUIRE(w.command->events().pages_size() == 1);
    REQUIRE(w.command->events().cover().ext().SerializeAsString() ==
            LedgerLinkage().SerializeAsString());
  });
  auto saw_root = [&w](const std::string& label) {
    REQUIRE(w.covers.size() == 1);
    REQUIRE(w.covers[0].root().value() == RootOf(label));
  };
  r.On("the ledger handler saw root {string}", [saw_root](const StepArgs& a) { saw_root(a[0]); });
  r.On("the reserving process-manager saw trigger root {string}",
       [saw_root](const StepArgs& a) { saw_root(a[0]); });
  r.On(
      "the reserving process-manager emits one Release command deferred from source sequence "
      "{int}",
      [&w](const StepArgs& a) {
        REQUIRE(w.pm.has_value());
        REQUIRE(w.pm->commands_size() == 1);
        const auto& page = w.pm->commands(0).pages(0);
        REQUIRE(FqFromUrl(page.command().type_url()) == "test.counter.Release");
        REQUIRE(page.header().sequence_type_case() == pb::PageHeader::kAngzarrDeferred);
        REQUIRE(static_cast<int>(page.header().angzarr_deferred().source_seq()) == std::stoi(a[0]));
      });
  r.On("the projector saw root {string} at sequences {int} and {int}", [&w](const StepArgs& a) {
    const std::string root = RootOf(a[0]);
    const std::vector<std::pair<std::string, uint32_t>> want = {
        {root, static_cast<uint32_t>(std::stoi(a[1]))},
        {root, static_cast<uint32_t>(std::stoi(a[2]))}};
    REQUIRE(w.pages == want);
  });
}

}  // namespace

TEST_CASE("UUID v5 roots match the cross-language reference", "[context]") {
  const std::string root = RootOf("ledger-1");
  std::string hex;
  for (unsigned char c : root) {
    static const char* digits = "0123456789abcdef";
    hex += digits[c >> 4];
    hex += digits[c & 0xf];
  }
  REQUIRE(hex == "280aa180e8cc5bc197586aeeda5ec0b0");
}

TEST_CASE("facts, replay and handler context", "[context]") {
  for (auto& sc : ParseFeature(FeaturePath("context.feature"))) {
    DYNAMIC_SECTION(sc.name) {
      ContextWorld world;
      StepRegistry reg;
      Register(reg, world);
      for (const auto& step : sc.steps) reg.Run(step.text);
    }
  }
}
