#pragma once

// The shared conformance envelopes, built BY FIELD (each an orthogonal envelope
// wrapping an empty inner message; the salient field is set from the scenario) —
// byte-equivalent to conformance/fixtures/*.txtpb, mirroring how the Go/C#
// bindings construct their envelopes. The behaviour asserted is the same
// cross-language contract.

#include <google/protobuf/any.pb.h>

#include <cstdint>
#include <map>
#include <optional>
#include <string>
#include <vector>

#include "angzarr/router/support.h"
#include "io/angzarr/v1/command_handler.pb.h"
#include "io/angzarr/v1/process_manager.pb.h"
#include "io/angzarr/v1/saga.pb.h"
#include "io/angzarr/v1/types.pb.h"
#include "test/counter/counter.pb.h"

namespace angzarr::conformance {

namespace pb = io::angzarr::v1;
namespace tc = test::counter;

inline std::string TypeUrl(const std::string& fq) { return "/" + fq; }

inline void SetAny(google::protobuf::Any* any, const std::string& fq, const std::string& value) {
  any->set_type_url(TypeUrl(fq));
  any->set_value(value);
}
inline void SetAnyEmpty(google::protobuf::Any* any, const std::string& fq) {
  any->set_type_url(TypeUrl(fq));
}

inline google::protobuf::Any ParentLinkage() {
  google::protobuf::Any any;
  SetAny(&any, "test.counter.Parent", std::string({1, 2, 3}));
  return any;
}

// --- commands -------------------------------------------------------------

inline pb::ContextualCommand IncreaseCommand(int n) {
  pb::ContextualCommand cc;
  auto* book = cc.mutable_command();
  book->mutable_cover()->set_domain("counter");
  tc::IncreaseBy ib;
  ib.set_n(static_cast<uint32_t>(n));
  SetAny(book->add_pages()->mutable_command(), "test.counter.IncreaseBy", ib.SerializeAsString());
  return cc;
}

inline pb::ContextualCommand IncreaseCommandWithLinkage(int n) {
  auto cc = IncreaseCommand(n);
  *cc.mutable_command()->mutable_cover()->mutable_ext() = ParentLinkage();
  return cc;
}

inline pb::ContextualCommand EmptyCommand(const std::string& fq) {
  pb::ContextualCommand cc;
  auto* book = cc.mutable_command();
  book->mutable_cover()->set_domain("counter");
  SetAnyEmpty(book->add_pages()->mutable_command(), fq);
  return cc;
}
inline pb::ContextualCommand FailHardCommand() { return EmptyCommand("test.counter.FailHard"); }
inline pb::ContextualCommand UnhandledCommand() { return EmptyCommand("test.counter.Reserve"); }

inline pb::Notification RejectionNotificationFor(const std::string& fq_command,
                                                 const std::string& domain) {
  pb::RejectionNotification rejection;
  auto* rc = rejection.mutable_rejected_command();
  rc->mutable_cover()->set_domain(domain);
  SetAnyEmpty(rc->add_pages()->mutable_command(), fq_command);
  pb::Notification n;
  *n.mutable_payload() = angzarr::router::Pack::Wrap(rejection);
  return n;
}

inline pb::ContextualCommand RejectionCommand(const std::string& fq_command) {
  pb::ContextualCommand cc;
  auto* book = cc.mutable_command();
  book->mutable_cover()->set_domain("counter");
  *book->add_pages()->mutable_command() =
      angzarr::router::Pack::Wrap(RejectionNotificationFor(fq_command, "counter"));
  return cc;
}

inline pb::ContextualCommand CommandMissingBook() {
  auto cc = IncreaseCommand(1);
  cc.clear_command();
  return cc;
}
inline pb::ContextualCommand CommandMissingPage() {
  auto cc = IncreaseCommand(1);
  cc.mutable_command()->clear_pages();
  return cc;
}
inline pb::ContextualCommand CommandMissingPayload() {
  auto cc = IncreaseCommand(1);
  cc.mutable_command()->mutable_pages(0)->clear_command();
  return cc;
}

// --- prior history --------------------------------------------------------

inline pb::EventPage IncreasedPageAt(int seq) {
  pb::EventPage page;
  SetAnyEmpty(page.mutable_event(), "test.counter.Increased");
  page.mutable_header()->set_sequence(static_cast<uint32_t>(seq));
  return page;
}

inline std::optional<pb::EventBook> PriorIncreases(int n) {
  if (n == 0) return std::nullopt;
  pb::EventBook book;
  book.set_next_sequence(static_cast<uint32_t>(n));
  for (int i = 0; i < n; ++i) *book.add_pages() = IncreasedPageAt(i);
  return book;
}

inline pb::EventBook CorruptHistory() {
  pb::EventBook book;
  auto* page = book.add_pages();
  *page = IncreasedPageAt(0);
  page->mutable_event()->set_value(std::string({(char)0xff, (char)0xff, (char)0xff}));
  book.set_next_sequence(1);
  return book;
}

inline pb::EventBook SnapshotHistory() {
  pb::EventBook book;
  tc::CounterState state;
  state.set_count(10);
  auto* snap = book.mutable_snapshot();
  snap->set_sequence(10);
  *snap->mutable_state() = angzarr::router::Pack::Wrap(state);
  *book.add_pages() = IncreasedPageAt(10);
  *book.add_pages() = IncreasedPageAt(11);
  book.set_next_sequence(12);
  return book;
}

// --- saga / process-manager shared ----------------------------------------

inline pb::CommandBook ReserveCommand() {
  pb::CommandBook book;
  book.mutable_cover()->set_domain("inventory");
  SetAnyEmpty(book.add_pages()->mutable_command(), "test.counter.Reserve");
  return book;
}

inline pb::EventBook OneFact() {
  pb::EventBook book;
  book.add_pages();
  return book;
}

inline pb::EventPage IncreasedEventPage() {
  pb::EventPage page;
  SetAnyEmpty(page.mutable_event(), "test.counter.Increased");
  return page;
}

// --- saga requests --------------------------------------------------------

inline pb::SagaHandleRequest SagaEventSource(const std::string& fq,
                                             const std::map<std::string, uint32_t>& dest) {
  pb::SagaHandleRequest req;
  auto* source = req.mutable_source();
  source->mutable_cover()->set_domain("order");
  SetAnyEmpty(source->add_pages()->mutable_event(), fq);
  for (const auto& [k, v] : dest) (*req.mutable_destination_sequences())[k] = v;
  return req;
}

inline pb::SagaHandleRequest SagaRejectionSource(const std::string& fq_command) {
  pb::SagaHandleRequest req;
  auto* source = req.mutable_source();
  source->mutable_cover()->set_domain("order");
  *source->add_pages()->mutable_event() =
      angzarr::router::Pack::Wrap(RejectionNotificationFor(fq_command, "inventory"));
  return req;
}

inline pb::SagaHandleRequest SagaSourceNoPages() {
  pb::SagaHandleRequest req;
  req.mutable_source();
  return req;
}
inline pb::SagaHandleRequest SagaRequestNoSource() { return {}; }

// --- projector deliveries -------------------------------------------------

inline pb::EventBook DeliveryBook(const std::string& domain, int n) {
  pb::EventBook book;
  book.mutable_cover()->set_domain(domain);
  for (int i = 0; i < n; ++i) *book.add_pages() = IncreasedEventPage();
  return book;
}
inline pb::EventBook DeliveryNoCover() {
  auto book = DeliveryBook("counter", 1);
  book.clear_cover();
  return book;
}

// --- process-manager triggers ---------------------------------------------

inline pb::ProcessManagerHandleRequest PmTrigger(const std::string& domain,
                                                 const std::vector<std::string>& fqs,
                                                 const std::optional<pb::EventBook>& state,
                                                 const std::map<std::string, uint32_t>& dest) {
  pb::ProcessManagerHandleRequest req;
  auto* trigger = req.mutable_trigger();
  trigger->mutable_cover()->set_domain(domain);
  for (const auto& fq : fqs) SetAnyEmpty(trigger->add_pages()->mutable_event(), fq);
  if (state) *req.mutable_process_state() = *state;
  for (const auto& [k, v] : dest) (*req.mutable_destination_sequences())[k] = v;
  return req;
}

inline pb::EventBook PmStateOf(int n) {
  pb::EventBook book;
  for (int i = 0; i < n; ++i) *book.add_pages() = IncreasedEventPage();
  return book;
}

inline pb::ProcessManagerHandleRequest PmRejection(const std::string& fq_command) {
  pb::ProcessManagerHandleRequest req;
  auto* trigger = req.mutable_trigger();
  trigger->mutable_cover()->set_domain("counter");
  *trigger->add_pages()->mutable_event() =
      angzarr::router::Pack::Wrap(RejectionNotificationFor(fq_command, "inventory"));
  return req;
}

inline pb::ProcessManagerHandleRequest PmNoTrigger() { return {}; }
inline pb::ProcessManagerHandleRequest PmEmptyTrigger() {
  pb::ProcessManagerHandleRequest req;
  req.mutable_trigger();
  return req;
}

}  // namespace angzarr::conformance
