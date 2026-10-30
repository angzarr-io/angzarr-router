//! Cucumber harness for the handler-contract tier's
//! `counter_dispatch.feature`: step defs parse the tier's orthogonal
//! `.txtpb` skeletons, set the scenario's data by field, dispatch to the
//! Rust router, and assert outcomes. Every router binds the same feature;
//! only the step-definition layer differs.

mod common;

use angzarr_router::error::CodedError;
use angzarr_router::pb;
use angzarr_router_conformance as conf;
use cucumber::writer::Stats as _;
use cucumber::{given, then, when, World};

#[derive(Debug, Default, World)]
struct CounterWorld {
    /// Prior-events book supplied to the next dispatch.
    prior: Option<pb::EventBook>,
    /// Outcome of the dispatched command.
    result: Option<Result<pb::CommandHandlerResponse, CodedError>>,
    /// Historical-state evidence the handler recorded during dispatch.
    observed: conf::ObservedSink,
}

impl CounterWorld {
    fn dispatch(&mut self, mut cc: pb::ContextualCommand) {
        cc.events = self.prior.clone();
        self.result = Some(conf::counter_aggregate(self.observed.clone()).dispatch(&cc));
    }

    fn last_observed(&self) -> conf::Observed {
        *self
            .observed
            .lock()
            .unwrap()
            .last()
            .expect("the handler recorded an observation")
    }
}

#[given("a new counter")]
async fn new_counter(w: &mut CounterWorld) {
    w.prior = None;
}

#[given(regex = r"^a counter that has already recorded (\d+) increases?$")]
async fn prior_increases(w: &mut CounterWorld, n: u32) {
    w.prior = conf::prior_history(n);
}

#[given(
    regex = r#"^a counter that has already recorded (\d+) increases under the "([^"]*)" type-URL prefix$"#
)]
async fn prior_increases_prefixed(w: &mut CounterWorld, n: u32, prefix: String) {
    let carrier = pb::ContextualCommand {
        events: conf::prior_history(n),
        ..Default::default()
    };
    w.prior = conf::with_type_url_prefix(carrier, &prefix).events;
}

#[given("a counter whose history holds a corrupt event")]
async fn corrupt_history(w: &mut CounterWorld) {
    w.prior = conf::corrupt_prior_history();
}

#[given(
    regex = r"^a counter delivered a starting state of (\d+) at sequence (\d+) and one Increased event at sequence (\d+)$"
)]
async fn starting_state(w: &mut CounterWorld, count: u32, sequence: u64, event_at: u64) {
    w.prior = conf::starting_state_history(count, sequence, event_at);
}

#[when(regex = r"^the operator increases the counter by (\d+)$")]
async fn increase_by(w: &mut CounterWorld, n: u32) {
    w.dispatch(conf::increase_command(n));
}

#[when(
    regex = r#"^the operator increases the counter by (\d+) under the "([^"]*)" type-URL prefix$"#
)]
async fn increase_by_prefixed(w: &mut CounterWorld, n: u32, prefix: String) {
    w.dispatch(conf::with_type_url_prefix(
        conf::increase_command(n),
        &prefix,
    ));
}

#[when("the operator triggers a hard failure")]
async fn hard_failure(w: &mut CounterWorld) {
    w.dispatch(conf::failhard_command());
}

#[when("an unhandled command is dispatched")]
async fn unhandled_command(w: &mut CounterWorld) {
    w.dispatch(conf::unhandled_command());
}

#[then(regex = r"^(\d+) increases are recorded$")]
async fn recorded(w: &mut CounterWorld, count: u32) {
    assert_eq!(
        recorded_book(w).pages.len() as u32,
        count,
        "recorded event count"
    );
}

#[then("the recorded events carry no router-stamped sequence")]
async fn no_stamped_sequence(w: &mut CounterWorld) {
    for (i, page) in recorded_book(w).pages.iter().enumerate() {
        assert_eq!(page.sequence, 0, "event {i} passes through unsequenced");
        assert_eq!(page.meta, None, "event {i} passes through with no meta");
    }
}

#[then(regex = r"^the command is rejected as ([A-Z_]+)$")]
async fn rejected_as(w: &mut CounterWorld, code: String) {
    assert_failed_with(w, &code);
    let err = w.result.as_ref().unwrap().as_ref().unwrap_err();
    assert_eq!(
        err.grpc,
        angzarr_router::error::GrpcCode::FailedPrecondition,
        "a business rejection is FAILED_PRECONDITION"
    );
}

#[then(regex = r"^the command fails with ([A-Z_]+)$")]
async fn fails_with(w: &mut CounterWorld, code: String) {
    assert_failed_with(w, &code);
}

#[then(regex = r"^the handler saw (no )?prior history, at next sequence (\d+)$")]
async fn observed_history(w: &mut CounterWorld, no: String, next_sequence: u64) {
    let obs = w.last_observed();
    assert_eq!(
        obs.had_prior_events,
        no.is_empty(),
        "had_prior_events evidence"
    );
    assert_eq!(obs.next_sequence, next_sequence, "next_sequence evidence");
}

#[then(regex = r"^the handler saw a counter of (\d+), at next sequence (\d+)$")]
async fn observed_count(w: &mut CounterWorld, count: u32, next_sequence: u64) {
    let obs = w.last_observed();
    assert_eq!(
        obs.count, count,
        "rebuilt count (the starting state + every page given)"
    );
    assert_eq!(obs.next_sequence, next_sequence, "next_sequence evidence");
    assert!(obs.had_prior_events, "a snapshot implies had_prior_events");
}

#[then("no events are recorded")]
async fn no_events(w: &mut CounterWorld) {
    // A rejection/failure is an Err and carries no events; if some impl
    // returned Ok, it must carry an empty book.
    if let Some(Ok(resp)) = &w.result {
        if let Some(book) = &resp.events {
            assert!(
                book.pages.is_empty(),
                "expected no events recorded, got {}",
                book.pages.len()
            );
        }
    }
}

fn recorded_book(w: &CounterWorld) -> &pb::EventBook {
    let resp = w
        .result
        .as_ref()
        .expect("a command was dispatched")
        .as_ref()
        .expect("expected a successful response");
    resp.events.as_ref().expect("an events response")
}

#[then(regex = r#"^the refusal is INTERNAL naming the type URL "([^"]*)"$"#)]
async fn refused_naming(w: &mut CounterWorld, url: String) {
    let err = w
        .result
        .as_ref()
        .expect("a command was dispatched")
        .as_ref()
        .expect_err("expected a refusal");
    // The router's own malformed-input error is a bug (14.37 ruling).
    assert_eq!(err.grpc, angzarr_router::error::GrpcCode::Internal);
    // Decision 14.37: the type URL is an arg; the status message is the key.
    assert_eq!(err.extras.get("type_url"), Some(&url), "the type_url arg");
    assert_eq!(
        err.status_message(),
        err.code,
        "the status message is the key"
    );
}

fn assert_failed_with(w: &CounterWorld, code: &str) {
    let err = w
        .result
        .as_ref()
        .expect("a command was dispatched")
        .as_ref()
        .err()
        .unwrap_or_else(|| panic!("expected failure {code}, got a success"));
    assert_eq!(err.code, code, "coded-error reason");
}

#[tokio::main]
async fn main() {
    let tier = CounterWorld::cucumber()
        .fail_on_skipped()
        .run(common::tier("counter_dispatch.feature"))
        .await;
    common::exit(tier.execution_has_failed());
}
