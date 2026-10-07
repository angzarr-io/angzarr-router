#pragma once

// Request builders shared by the binding unit tests.

#include <google/protobuf/any.pb.h>

#include <string>

#include "angzarr/router/support.h"
#include "io/angzarr/v1/command_handler.pb.h"
#include "io/angzarr/v1/types.pb.h"

namespace angzarr::router::testing {

namespace pb = io::angzarr::v1;

inline google::protobuf::Any EmptyAny(const std::string& fq) {
  google::protobuf::Any any;
  any.set_type_url("/" + fq);
  return any;
}

// A command for the "counter" domain whose single page is an empty fq message.
inline pb::ContextualCommand CommandOf(const std::string& fq) {
  pb::ContextualCommand cc;
  auto* book = cc.mutable_command();
  book->mutable_cover()->set_domain("counter");
  *book->add_pages()->mutable_command() = EmptyAny(fq);
  return cc;
}

// A "counter" command whose page is the Notification of a rejected fq_command.
inline pb::ContextualCommand RejectionOf(const std::string& fq_command) {
  pb::RejectionNotification rejection;
  auto* rc = rejection.mutable_rejected_command();
  rc->mutable_cover()->set_domain("counter");
  *rc->add_pages()->mutable_command() = EmptyAny(fq_command);
  pb::Notification n;
  *n.mutable_payload() = Pack::Wrap(rejection);
  pb::ContextualCommand cc;
  auto* book = cc.mutable_command();
  book->mutable_cover()->set_domain("counter");
  *book->add_pages()->mutable_command() = Pack::Wrap(n);
  return cc;
}

}  // namespace angzarr::router::testing
