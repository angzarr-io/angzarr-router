//! Cucumber harness for the handler-contract tier's
//! `policy_dispatch.feature`: drives it against the Rust router's
//! PolicyDispatch. Every router binds the same feature; only this
//! step-definition layer differs.

mod common;

use angzarr_router::error::CodedError;
use angzarr_router::pb;
use angzarr_router::router::PolicyComponent;
use angzarr_router_conformance as conf;
use cucumber::writer::Stats as _;
use cucumber::{given, then, when, World};

#[derive(Debug, Default, World)]
struct PolicyWorld {
    /// Outcome of the dispatched source.
    result: Option<Result<pb::PolicyResponse, CodedError>>,
    /// The source sequences the policy handler saw.
    seen: conf::SequenceSink,
}

impl PolicyWorld {
    /// Dispatches through the kernel-facing [`PolicyComponent`], awaiting
    /// its answer.
    async fn dispatch(&mut self, req: pb::EventBook) {
        let policy = conf::order_policy(self.seen.clone());
        self.result = Some(PolicyComponent::dispatch(&policy, &req).await);
    }

    fn response(&self) -> &pb::PolicyResponse {
        self.result
            .as_ref()
            .expect("a source was dispatched")
            .as_ref()
            .expect("expected a successful policy response")
    }

    /// Every command answered, in page order.
    fn commands(&self) -> Vec<pb::CommandBook> {
        self.response()
            .reactions
            .iter()
            .flat_map(|r| r.commands.iter().cloned())
            .collect()
    }
}

#[given(regex = r#"^an order policy delivering to "([^"]*)"$"#)]
async fn an_order_policy(_w: &mut PolicyWorld, _target: String) {
    // Each dispatch builds a fresh policy; nothing to seed.
}

#[when(regex = r"^an Increased event at sequence (\d+) is dispatched$")]
async fn increased_at(w: &mut PolicyWorld, seq: u64) {
    w.dispatch(conf::policy_event_source("test.counter.Increased", seq))
        .await;
}

#[when("a Reserve event is dispatched")]
async fn reserve_event(w: &mut PolicyWorld) {
    w.dispatch(conf::policy_event_source("test.counter.Reserve", 0))
        .await;
}

#[then(regex = r#"^the policy emits one command to "([^"]*)", in the reaction to sequence (\d+)$"#)]
async fn emits_one_command_in_reaction(w: &mut PolicyWorld, target: String, sequence: u64) {
    let reactions = &w.response().reactions;
    assert_eq!(reactions.len(), 1, "one reaction, to the one handled page");
    assert_eq!(reactions[0].trigger_sequence, sequence, "its trigger");
    assert_eq!(
        reactions[0].commands.len(),
        1,
        "exactly one command emitted"
    );
    assert_eq!(
        reactions[0].commands[0]
            .cover
            .as_ref()
            .expect("cover")
            .domain,
        target
    );
}

#[then(regex = r"^the policy handler saw source sequence (\d+)$")]
async fn handler_saw_sequence(w: &mut PolicyWorld, seq: u64) {
    assert_eq!(*w.seen.lock().unwrap(), vec![seq]);
}

#[then("the command carries no router-stamped provenance")]
async fn no_stamped_provenance(w: &mut PolicyWorld) {
    let commands = w.commands();
    let page = commands[0].page.as_ref().expect("a command page");
    assert_eq!(
        page.command_id, None,
        "the coordinator stamps the command id"
    );
    assert_eq!(
        page.correlation_id, None,
        "the coordinator stamps the correlation"
    );
    assert_eq!(
        page.causation_id, None,
        "the coordinator stamps the causation"
    );
}

#[then("the policy emits no commands")]
async fn emits_no_commands(w: &mut PolicyWorld) {
    assert!(w.commands().is_empty(), "no commands emitted");
}

#[tokio::main]
async fn main() {
    let tier = PolicyWorld::cucumber()
        .fail_on_skipped()
        .run(common::tier("policy_dispatch.feature"))
        .await;
    common::exit(tier.execution_has_failed());
}
