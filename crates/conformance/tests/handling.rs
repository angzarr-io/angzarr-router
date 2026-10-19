//! Cucumber harness for `handling.feature`: the rules version, conflict
//! resolution and command upcasting, driven natively against the list
//! aggregate on the Rust core.

use angzarr_router::error::{CodedError, GrpcCode};
use angzarr_router::pb;
use angzarr_router_conformance as conf;
use cucumber::{given, then, when, World};

#[derive(Debug, Default, World)]
struct HandlingWorld {
    rules_version: String,
    ack: conf::Ack,
    seen: conf::ListSink,
    result: Option<Result<pb::CommandHandlerResponse, CodedError>>,
}

impl HandlingWorld {
    fn dispatch(&mut self, req: pb::ContextualCommand) {
        let list = conf::list_aggregate(&self.rules_version, self.ack.clone(), self.seen.clone());
        self.result = Some(list.dispatch(&req));
    }

    fn response(&self) -> &pb::CommandHandlerResponse {
        self.result
            .as_ref()
            .expect("a command was dispatched")
            .as_ref()
            .expect("expected an accepted command")
    }

    fn refusal(&self) -> &CodedError {
        self.result
            .as_ref()
            .expect("a command was dispatched")
            .as_ref()
            .expect_err("expected a refusal")
    }

    fn observed(&self) -> conf::ListObserved {
        self.seen
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("the handler ran")
    }
}

/// The quoted keys of `"x", "y" and "z"`.
fn keys(list: &str) -> Vec<String> {
    list.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

fn resolution(name: &str) -> pb::ConflictResolution {
    match name {
        "STRICT" => pb::ConflictResolution::Strict,
        "AGGREGATE_HANDLES" => pb::ConflictResolution::AggregateHandles,
        "COMMUTATIVE" => pb::ConflictResolution::Commutative,
        other => panic!("unknown resolution {other}"),
    }
}

#[given(regex = r#"^a list aggregate at rules version "([^"]*)"$"#)]
async fn list_aggregate(w: &mut HandlingWorld, rules_version: String) {
    w.rules_version = rules_version;
}

#[given("the handler resolves conflicts with the delivered nonce")]
async fn resolves(w: &mut HandlingWorld) {
    w.ack = conf::Ack::Delivered;
}

#[given("the handler does not resolve conflicts")]
async fn ignores(w: &mut HandlingWorld) {
    w.ack = conf::Ack::Never;
}

#[given(regex = r#"^the handler resolves conflicts with nonce "([^"]*)"$"#)]
async fn made_up(w: &mut HandlingWorld, nonce: String) {
    w.ack = conf::Ack::With(nonce.into_bytes());
}

#[when(regex = r#"^(\w+) "([^"]*)" is handled over no prior events$"#)]
async fn handled(w: &mut HandlingWorld, command: String, key: String) {
    w.dispatch(conf::list_command(&command, &key, &[], None));
}

#[when(
    regex = r#"^Bump "([^"]*)" is handled (STRICT|AGGREGATE_HANDLES|COMMUTATIVE) at expected sequence (\d+) over bumps of (.+?)(?: with nonce "([^"]*)")?$"#
)]
async fn handled_resolving(
    w: &mut HandlingWorld,
    key: String,
    name: String,
    expected: u64,
    history: String,
    nonce: String,
) {
    w.dispatch(conf::list_command(
        "Bump",
        &key,
        &keys(&history),
        Some((expected, resolution(&name), nonce.into_bytes())),
    ));
}

#[then(regex = r#"^the response reports rules version "([^"]*)"$"#)]
async fn reports(w: &mut HandlingWorld, rules_version: String) {
    assert_eq!(w.response().rules_version, rules_version);
}

#[then(regex = r"^the handler saw expected sequence (\d+) and resolution (\w+)$")]
async fn saw_handling(w: &mut HandlingWorld, expected: u64, name: String) {
    let seen = w.observed();
    assert_eq!(seen.expected_sequence, expected);
    assert_eq!(seen.conflict_resolution, resolution(&name));
}

#[then("the handler saw no conflict window")]
async fn saw_no_window(w: &mut HandlingWorld) {
    assert_eq!(w.observed().window, None);
}

#[then(regex = r"^the handler saw the window at sequences (\d+) and (\d+)$")]
async fn saw_window(w: &mut HandlingWorld, first: u64, second: u64) {
    assert_eq!(w.observed().window, Some(vec![first, second]));
}

#[then("the command is accepted")]
async fn accepted(w: &mut HandlingWorld) {
    w.response();
}

#[then(regex = r"^the command fails with ([A-Z_]+) as ABORTED$")]
async fn aborted(w: &mut HandlingWorld, code: String) {
    let err = w.refusal();
    assert_eq!(err.code, code);
    assert_eq!(err.grpc, GrpcCode::Aborted);
}

#[then("no events are recorded")]
async fn no_events(w: &mut HandlingWorld) {
    // A refusal carries no events at all.
    w.refusal();
}

#[then(regex = r#"^the handler saw items "([^"]*)" and "([^"]*)" in its state$"#)]
async fn saw_items(w: &mut HandlingWorld, first: String, second: String) {
    assert_eq!(w.observed().items, vec![first, second]);
}

#[then("the handler saw a Bump")]
async fn saw_bump(w: &mut HandlingWorld) {
    assert_eq!(w.observed().command, "test.counter.Bump");
}

#[then(regex = r#"^the command is refused INVALID_ARGUMENT naming the type URL "([^"]*)"$"#)]
async fn refused_naming(w: &mut HandlingWorld, url: String) {
    let err = w.refusal();
    assert_eq!(err.grpc, GrpcCode::InvalidArgument);
    assert_eq!(err.message, format!("no handler for type URL {url}"));
}

#[then(regex = r#"^the refusal carries the upcast type URL "([^"]*)"$"#)]
async fn carries_upcast(w: &mut HandlingWorld, url: String) {
    assert_eq!(
        w.refusal()
            .extras
            .get("upcast_type_url")
            .map(String::as_str),
        Some(url.as_str())
    );
}

#[tokio::main]
async fn main() {
    let feature = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/features/handling.feature"
    );
    HandlingWorld::cucumber().run_and_exit(feature).await;
}
