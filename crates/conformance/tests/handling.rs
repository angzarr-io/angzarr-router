//! Binds angzarr-project's handler-contract tier
//! (features/handler-contract/handling.feature, C-0533..C-0550; the spec is
//! the source, read from the submodule) to the Rust core: the order
//! aggregate and the Fulfillment process manager built by hand over the
//! orders.v1 fixture messages.

use angzarr_router::error::{CodedError, GrpcCode};
use angzarr_router::pb;
use angzarr_router_conformance as conf;
use cucumber::{given, then, when, World};

#[derive(Debug, Default, World)]
struct HandlingWorld {
    rules_version: String,
    pm_rules_version: String,
    settle: conf::Settle,
    seen: conf::OrderSink,
    snapshot: Option<pb::Snapshot>,
    result: Option<Result<pb::CommandHandlerResponse, CodedError>>,
    pm_result: Option<Result<pb::ProcessManagerHandleResponse, CodedError>>,
    registration: Option<pb::ComponentDescriptor>,
}

impl HandlingWorld {
    fn dispatch(&mut self, req: pb::ContextualCommand) {
        let order =
            conf::order_aggregate(&self.rules_version, self.settle.clone(), self.seen.clone());
        self.result = Some(order.dispatch(&req));
    }

    fn accepted(&self) -> &pb::CommandHandlerResponse {
        self.result
            .as_ref()
            .expect("a command was handled")
            .as_ref()
            .expect("expected the command accepted")
    }

    fn refusal(&self) -> &CodedError {
        self.result
            .as_ref()
            .expect("a command was handled")
            .as_ref()
            .expect_err("expected a refusal")
    }

    fn observed(&self) -> conf::OrderObserved {
        self.seen
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("business code ran")
    }
}

/// The `Kind "value"` pairs of `ItemAdded "pear", ItemAdded "plum" and ...`.
fn history(list: &str) -> Vec<(String, String)> {
    let parts: Vec<&str> = list.split('"').collect();
    parts
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| {
            let kind = c[0]
                .trim()
                .trim_start_matches(',')
                .trim_start_matches("and")
                .trim();
            (kind.to_string(), c[1].to_string())
        })
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

// --- the version -------------------------------------------------------------

#[given(regex = r#"^an "order" business binary built as version "([^"]*)"$"#)]
async fn order_binary(w: &mut HandlingWorld, version: String) {
    w.rules_version = version;
}

#[given(regex = r#"^a "Fulfillment" process manager built as version "([^"]*)"$"#)]
async fn fulfillment(w: &mut HandlingWorld, version: String) {
    w.pm_rules_version = version;
}

#[when("the process manager handles an OrderPlaced event")]
async fn pm_handles(w: &mut HandlingWorld) {
    let pm = conf::fulfillment_pm(&w.pm_rules_version);
    w.pm_result = Some(pm.dispatch(&conf::order_placed_trigger()));
}

#[then(regex = r#"^the answer reports rules version "([^"]*)"$"#)]
async fn reports(w: &mut HandlingWorld, version: String) {
    let reported = match (&w.result, &w.pm_result) {
        (_, Some(pm)) => pm.as_ref().expect("the PM answered").rules_version.clone(),
        (Some(_), None) => w.accepted().rules_version.clone(),
        (None, None) => panic!("nothing was handled"),
    };
    assert_eq!(reported, version);
}

// The registration hello (types.proto ComponentDescriptor): the router builds
// it from the aggregate's own table.
#[when(r#"the "order" business binary registers with its coordinator"#)]
async fn registers(w: &mut HandlingWorld) {
    let order = conf::order_aggregate(&w.rules_version, w.settle.clone(), w.seen.clone());
    w.registration = Some(order.component_descriptor().expect("a registration hello"));
}

#[then(regex = r#"^the registration carries rules version "([^"]*)"$"#)]
async fn registration_version(w: &mut HandlingWorld, version: String) {
    let hello = w.registration.as_ref().expect("the binary registered");
    assert_eq!(hello.rules_version, version);
}

#[then("the registration carries the order state type")]
async fn registration_state(w: &mut HandlingWorld) {
    let hello = w.registration.as_ref().expect("the binary registered");
    assert_eq!(hello.kind(), pb::ComponentKind::Aggregate);
    assert_eq!(
        hello.state.as_ref().map(|s| s.type_url.as_str()),
        Some("/orders.v1.OrderState")
    );
}

#[given(
    regex = r#"^a delivered snapshot of the order state recorded under rules version "([^"]*)"$"#
)]
async fn delivered_snapshot(w: &mut HandlingWorld, version: String) {
    w.snapshot = Some(conf::order_snapshot(&["fig"], &version));
}

#[when(regex = r#"^AddItem "([^"]*)" is handled over that snapshot$"#)]
async fn over_snapshot(w: &mut HandlingWorld, item: String) {
    let snapshot = w.snapshot.clone();
    w.dispatch(conf::order_command("AddItem", &item, &[], None, snapshot));
}

#[then("business code decides on the snapshot's state")]
async fn decided_on_snapshot(w: &mut HandlingWorld) {
    assert_eq!(w.observed().items, vec!["fig".to_string()]);
}

// --- commands, facts and upcasting --------------------------------------------

#[when(regex = r#"^(\w+) "([^"]*)" is handled over no prior events$"#)]
async fn handled(w: &mut HandlingWorld, command: String, item: String) {
    w.dispatch(conf::order_command(&command, &item, &[], None, None));
}

#[when(regex = r#"^a fact (\w+) "([^"]*)" is handled over no prior events$"#)]
async fn fact_handled(w: &mut HandlingWorld, fact: String, item: String) {
    w.dispatch(conf::order_fact(&fact, &item));
}

#[then("the command is accepted")]
async fn command_accepted(w: &mut HandlingWorld) {
    w.accepted();
}

#[then("the fact is accepted")]
async fn fact_accepted(w: &mut HandlingWorld) {
    w.accepted();
}

#[then(regex = r#"^business code decides an (\w+) for "([^"]*)"$"#)]
async fn decides(w: &mut HandlingWorld, kind: String, item: String) {
    let seen = w.observed();
    assert_eq!(seen.handled, format!("orders.v1.{kind}"));
    assert_eq!(seen.item, item);
}

#[then(regex = r#"^business code ingests an (\w+) for "([^"]*)"$"#)]
async fn ingests(w: &mut HandlingWorld, kind: String, item: String) {
    let seen = w.observed();
    assert_eq!(seen.handled, format!("orders.v1.{kind}"));
    assert_eq!(seen.item, item);
}

#[then("the command is refused INVALID_ARGUMENT")]
async fn refused_invalid(w: &mut HandlingWorld) {
    assert_eq!(w.refusal().grpc, GrpcCode::InvalidArgument);
}

#[then(regex = r#"^the refusal message names the type URL "([^"]*)"$"#)]
async fn message_names(w: &mut HandlingWorld, url: String) {
    assert_eq!(
        w.refusal().message,
        format!("no handler for type URL {url}")
    );
}

// The extras render as the status's google.rpc.ErrorInfo metadata (see
// crates/router-ffi/src/abi.rs, coded_to_status_bytes).
#[then(regex = r#"^the refusal's error info carries upcast_type_url "([^"]*)"$"#)]
async fn carries_upcast(w: &mut HandlingWorld, url: String) {
    assert_eq!(
        w.refusal()
            .extras
            .get("upcast_type_url")
            .map(String::as_str),
        Some(url.as_str())
    );
}

#[then("business code is not invoked")]
async fn not_invoked(w: &mut HandlingWorld) {
    assert!(w.seen.lock().unwrap().is_empty());
}

// --- conflicts -----------------------------------------------------------------

#[given("business code settles conflicts with the nonce it was given")]
async fn settles(w: &mut HandlingWorld) {
    w.settle = conf::Settle::Delivered;
}

#[given("business code does not settle conflicts")]
async fn does_not_settle(w: &mut HandlingWorld) {
    w.settle = conf::Settle::Never;
}

#[given(regex = r#"^business code settles conflicts with nonce "([^"]*)"$"#)]
async fn settles_with(w: &mut HandlingWorld, nonce: String) {
    w.settle = conf::Settle::With(nonce.into_bytes());
}

#[when(
    regex = r#"^AddItem "([^"]*)" is handled (STRICT|AGGREGATE_HANDLES|COMMUTATIVE) at expected sequence (\d+) over (.+?)(?: with nonce "([^"]*)")?$"#
)]
async fn handled_resolving(
    w: &mut HandlingWorld,
    item: String,
    name: String,
    expected: u64,
    events: String,
    nonce: String,
) {
    w.dispatch(conf::order_command(
        "AddItem",
        &item,
        &history(&events),
        Some((expected, resolution(&name), nonce.into_bytes())),
        None,
    ));
}

#[then(regex = r"^business code saw expected sequence (\d+) under (\w+)$")]
async fn saw_handling(w: &mut HandlingWorld, expected: u64, name: String) {
    let seen = w.observed();
    assert_eq!(seen.expected_sequence, expected);
    assert_eq!(seen.conflict_resolution, resolution(&name));
}

#[then("business code saw no conflict window and no nonce")]
async fn saw_no_conflict(w: &mut HandlingWorld) {
    let seen = w.observed();
    assert_eq!(seen.window, None);
    assert_eq!(seen.nonce, None);
}

#[then(regex = r"^business code saw the window at sequences (\d+) and (\d+)$")]
async fn saw_window(w: &mut HandlingWorld, first: u64, second: u64) {
    assert_eq!(w.observed().window, Some(vec![first, second]));
}

#[then("the command fails as ABORTED")]
async fn aborted(w: &mut HandlingWorld) {
    assert_eq!(w.refusal().grpc, GrpcCode::Aborted);
}

#[then("business code's events are discarded")]
async fn discarded(w: &mut HandlingWorld) {
    // A refusal carries no events: none reach the coordinator.
    w.refusal();
}

#[then(regex = r#"^business code saw items "([^"]*)" and "([^"]*)" in its state$"#)]
async fn saw_items(w: &mut HandlingWorld, first: String, second: String) {
    assert_eq!(w.observed().items, vec![first, second]);
}

#[tokio::main]
async fn main() {
    let feature = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../angzarr-project/features/handler-contract/handling.feature"
    );
    HandlingWorld::cucumber()
        .fail_on_skipped()
        .run_and_exit(feature)
        .await;
}
