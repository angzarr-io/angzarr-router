//! On wasm32 the kernel-facing dispatch traits ask no `Send` (owner rulings
//! 2026-10-27 and 2026-10-30): a browser coordinator's components, whose
//! handles are not `Send`, implement the same one trait set, and the
//! router hosts them. `RcDomain`, `RcPolicy` and `RcProjector` stand for
//! such components: each holds an `Rc`, so neither it nor its dispatch
//! future (which holds the `Rc` across an await) is `Send`.
//!
//! This file is compiled only for wasm32, by `just wasm`; compiling is the
//! test (the native build of the same traits asks `Send`, and these impls
//! would not compile there).

#![cfg(target_arch = "wasm32")]

use std::future::Future;
use std::pin::pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use angzarr_router::context::CommandContext;
use angzarr_router::error::CodedError;
use angzarr_router::pb;
use angzarr_router::router::{
    CommandHandler, DynCommandHandler, DynPolicyComponent, DynProjectorHandler, PolicyComponent,
    ProjectorHandler, RouterBuilder,
};

/// An await point holding `held` across it: makes the future not `Send`.
async fn across_an_await(held: Rc<str>) -> String {
    std::future::ready(()).await;
    held.to_string()
}

struct RcDomain(Rc<str>);

impl CommandHandler for RcDomain {
    fn name(&self) -> &str {
        &self.0
    }
    fn command_types(&self) -> Vec<String> {
        Vec::new()
    }
    async fn dispatch(
        &self,
        _req: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError> {
        Ok(pb::CommandHandlerResponse {
            rules_version: across_an_await(Rc::clone(&self.0)).await,
            ..Default::default()
        })
    }
}

struct RcPolicy(Rc<str>);

impl PolicyComponent for RcPolicy {
    fn name(&self) -> &str {
        &self.0
    }
    fn input_domain(&self) -> &str {
        &self.0
    }
    async fn dispatch(&self, _source: &pb::EventBook) -> Result<pb::PolicyResponse, CodedError> {
        across_an_await(Rc::clone(&self.0)).await;
        Ok(pb::PolicyResponse::default())
    }
    async fn dispatch_refusal(
        &self,
        _refusal: &pb::Refusal,
        _ctx: &CommandContext,
    ) -> Result<Option<prost_types::Any>, CodedError> {
        across_an_await(Rc::clone(&self.0)).await;
        Ok(None)
    }
}

struct RcProjector(Rc<str>);

impl ProjectorHandler for RcProjector {
    fn name(&self) -> &str {
        &self.0
    }
    async fn dispatch(&self, _events: &pb::EventBook) -> Result<pb::Projection, CodedError> {
        Ok(pb::Projection {
            projector: across_an_await(Rc::clone(&self.0)).await,
            ..Default::default()
        })
    }
}

fn now<F: Future>(future: F) -> F::Output {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(answer) => answer,
        Poll::Pending => panic!("nothing here waits"),
    }
}

#[test]
fn a_router_hosts_components_that_are_not_send() {
    let router = RouterBuilder::new()
        .aggregate(RcDomain(Rc::from("rc")))
        .build()
        .expect("one domain");
    let answer = now(router.dispatch_command(&pb::ContextualCommand::default()));
    assert_eq!(answer.expect("answered").rules_version, "rc");

    let router = RouterBuilder::new()
        .projector(RcProjector(Rc::from("rc")))
        .build()
        .expect("one projector");
    let answer = now(router.dispatch_projector(&pb::EventBook::default()));
    assert_eq!(answer.expect("answered").projector, "rc");
}

#[test]
fn components_that_are_not_send_box_through_their_twins() {
    let domain: Box<dyn DynCommandHandler> = Box::new(RcDomain(Rc::from("rc")));
    let policy: Box<dyn DynPolicyComponent> = Box::new(RcPolicy(Rc::from("rc")));
    let projector: Box<dyn DynProjectorHandler> = Box::new(RcProjector(Rc::from("rc")));
    assert!(now(CommandHandler::dispatch(&domain, &Default::default())).is_ok());
    assert!(now(PolicyComponent::dispatch(&policy, &Default::default())).is_ok());
    assert!(now(PolicyComponent::dispatch_refusal(
        &policy,
        &Default::default(),
        &CommandContext::default()
    ))
    .is_ok());
    assert!(now(ProjectorHandler::dispatch(&projector, &Default::default())).is_ok());
}
