//! Binds angzarr-project's handler-contract tier
//! (features/handler-contract/handling.feature, C-0533..C-0550, C-0565, C-0570, C-0571, C-0580, C-0583 and decision
//! 14.37's C-0667..C-0672; the spec is
//! the source, read from the submodule) to the Rust core: the order
//! aggregate and the Fulfillment process manager (a domain, decision 14.31)
//! built by hand over the orders.v1 fixture messages.

use angzarr_router::error::{CodedError, GrpcCode};
use angzarr_router::pb;
use angzarr_router_conformance as conf;
use cucumber::{given, then, when, World};
use prost::Message;

/// The business process's registered component.
struct Registered(angzarr_router::router::Router);

impl std::fmt::Debug for Registered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Registered(Router)")
    }
}

#[derive(Debug, Default, World)]
struct HandlingWorld {
    rules_version: String,
    pm_rules_version: String,
    settle: conf::Settle,
    seen: conf::OrderSink,
    snapshot: Option<pb::Snapshot>,
    result: Option<Result<pb::CommandHandlerResponse, CodedError>>,
    pm_result: Option<Result<pb::CommandHandlerResponse, CodedError>>,
    pm_placed: u32,
    policy: Option<(String, String, String)>,
    policy_strays: Vec<(String, String)>,
    policy_result: Option<Result<pb::PolicyResponse, CodedError>>,
    registered: Option<Registered>,
    second_refusal: Option<CodedError>,
    described: Vec<pb::ComponentDescriptor>,
    /// How AddItem fails, when it does (decision 14.37).
    fails: Option<conf::AddItemFails>,
    /// When the delivery says its input was accepted (C-0671).
    recorded_at: Option<prost_types::Timestamp>,
    /// A handler's panic that escaped the router, with its text.
    escaped: Option<String>,
    /// The answer to one of C-0670's conditions: `Ok` when it was not
    /// refused.
    outcome: Option<Result<(), CodedError>>,
}

impl HandlingWorld {
    fn dispatch(&mut self, mut req: pb::ContextualCommand) {
        if self.recorded_at.is_some() {
            req.recorded_at = self.recorded_at;
        }
        let order = conf::order_aggregate_with(
            &self.rules_version,
            self.settle.clone(),
            self.seen.clone(),
            self.fails.clone(),
        );
        // A handler's panic is the router's to answer (decision 14.37);
        // one that escapes is recorded, for the Then steps to fail on.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| order.dispatch(&req))) {
            Ok(answer) => self.result = Some(answer),
            Err(panic) => {
                self.escaped = Some(
                    panic
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| panic.downcast_ref::<&str>().map(|t| t.to_string()))
                        .unwrap_or_default(),
                )
            }
        }
    }

    /// The error the scenario's input was refused with: one of C-0670's
    /// conditions, or the handled command's.
    fn the_error(&self) -> &CodedError {
        match &self.outcome {
            Some(outcome) => outcome
                .as_ref()
                .expect_err("expected the condition refused, it was not"),
            None => self.refusal(),
        }
    }

    fn accepted(&self) -> &pb::CommandHandlerResponse {
        self.result
            .as_ref()
            .expect("a command was handled")
            .as_ref()
            .expect("expected the command accepted")
    }

    fn refusal(&self) -> &CodedError {
        if let Some(text) = &self.escaped {
            panic!(
                "the handler's panic escaped the router instead of being answered \
                 UNHANDLED_HANDLER_ERROR with cause HANDLER_PANICKED: {text:?}"
            );
        }
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

#[when("the process manager handles a StartFulfillment command")]
async fn pm_handles(w: &mut HandlingWorld) {
    let pm = conf::fulfillment_pm(&w.pm_rules_version);
    w.pm_result = Some(pm.dispatch(&conf::start_fulfillment()));
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

// Describing the process (component.proto, ComponentService.Describe): its
// one component's ComponentDescriptor, built by the router from the
// component's own table (owner ruling: one coordinator, one component, one
// business process).
#[when(r#"the coordinator asks the "order" business binary to describe its component"#)]
async fn describe(w: &mut HandlingWorld) {
    let order = conf::order_aggregate(&w.rules_version, w.settle.clone(), w.seen.clone());
    let router = angzarr_router::router::RouterBuilder::new()
        .aggregate(order)
        .build()
        .expect("one component");
    w.described = vec![router.component_descriptor().expect("its description")];
}

fn described<'a>(w: &'a HandlingWorld, name: &str) -> &'a pb::ComponentDescriptor {
    w.described
        .iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("no description of {name}"))
}

#[then(regex = r#"^the "order" aggregate's description carries rules version "([^"]*)"$"#)]
async fn described_version(w: &mut HandlingWorld, version: String) {
    let order = described(w, "order");
    assert_eq!(order.kind(), pb::ComponentKind::Aggregate);
    assert_eq!(order.rules_version, version);
}

#[then(r#"the "order" aggregate's description carries the order state type"#)]
async fn described_state(w: &mut HandlingWorld) {
    assert_eq!(
        described(w, "order")
            .state
            .as_ref()
            .map(|s| s.type_url.as_str()),
        Some("/orders.v1.OrderState")
    );
}

#[then("the answer describes one component:")]
async fn describes_exactly(w: &mut HandlingWorld, step: &cucumber::gherkin::Step) {
    let table = step.table.as_ref().expect("a table");
    let expected: Vec<Vec<String>> = table.rows.iter().skip(1).cloned().collect();
    let actual: Vec<Vec<String>> = w
        .described
        .iter()
        .map(|d| {
            vec![
                d.name.clone(),
                d.kind()
                    .as_str_name()
                    .trim_start_matches("COMPONENT_KIND_")
                    .to_string(),
                d.rules_version.clone(),
                d.state
                    .as_ref()
                    .map(|s| s.type_url.clone())
                    .unwrap_or_default(),
            ]
        })
        .collect();
    assert_eq!(actual, expected);
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

// --- the state on every answer (C-0570, C-0571; decisions 14.12, 14.14) -------

#[given("business code that never mentions snapshots")]
async fn never_mentions_snapshots(_w: &mut HandlingWorld) {}

#[when(regex = r#"^(.+) is handled over ItemAdded "([^"]*)"$"#)]
async fn delivery_over(w: &mut HandlingWorld, delivery: String, prior: String) {
    let history = [("ItemAdded".to_string(), prior)];
    let quoted = |text: &str| text.split('"').nth(1).unwrap_or_default().to_string();
    let req = if delivery == "a command that business code answers with no events" {
        conf::order_command("Acknowledge", "", &history, None, None)
    } else if let Some(fact) = delivery.strip_prefix("a fact ") {
        let mut req = conf::order_fact(fact.split(' ').next().unwrap(), &quoted(fact));
        req.events = conf::order_command("AddItem", "", &history, None, None).events;
        req
    } else {
        let command = delivery.split(' ').next().unwrap().to_string();
        conf::order_command(&command, &quoted(&delivery), &history, None, None)
    };
    w.dispatch(req);
}

#[then(regex = r#"^the answer carries the order state with (.+) as its snapshot state$"#)]
async fn answer_carries_state(w: &mut HandlingWorld, described: String) {
    // `items "pear" and "apple"`, `item "pear"`, or `item "pear" and "apple"
    // delivered` (the last quoted item delivered).
    let quoted: Vec<&str> = described.split('"').skip(1).step_by(2).collect();
    let (items, delivered) = if described.ends_with("delivered") {
        quoted.split_at(quoted.len() - 1)
    } else {
        (quoted.as_slice(), &[][..])
    };
    let snapshot = w
        .accepted()
        .events
        .as_ref()
        .and_then(|b| b.snapshot.clone())
        .expect("the answer's snapshot");
    let mut expected: Vec<(String, u32)> = items.iter().map(|i| (i.to_string(), 1)).collect();
    expected.sort();
    assert_eq!(
        conf::read_order_state(&snapshot.state.expect("a state")),
        (
            "/orders.v1.OrderState".to_string(),
            expected,
            delivered.iter().map(|d| d.to_string()).collect()
        )
    );
}

#[when(
    regex = r"^the process manager handles a StartFulfillment command (and writes a FulfillmentStarted event|without writing a process event)$"
)]
async fn pm_handles_writing(w: &mut HandlingWorld, writing: String) {
    let writes = writing.starts_with("and writes");
    w.pm_placed = u32::from(writes);
    let pm = conf::fulfillment_pm_writing(&w.pm_rules_version, writes);
    w.pm_result = Some(pm.dispatch(&conf::start_fulfillment()));
}

fn pm_answer(w: &HandlingWorld) -> &pb::CommandHandlerResponse {
    w.pm_result
        .as_ref()
        .expect("the PM handled")
        .as_ref()
        .expect("the PM answered")
}

#[then("the answer's snapshot state is the process manager's state after the command")]
async fn pm_snapshot_state(w: &mut HandlingWorld) {
    assert_eq!(
        pm_answer(w)
            .events
            .as_ref()
            .and_then(|b| b.snapshot.as_ref())
            .and_then(|s| s.state.clone()),
        Some(conf::fulfillment_state(w.pm_placed))
    );
}

#[then("none of the answer's event pages carry a state")]
async fn pm_pages_carry_no_state(w: &mut HandlingWorld) {
    let events = pm_answer(w).events.as_ref().expect("its events");
    // The state rides the snapshot only: every page is the PM's event.
    assert_eq!(events.pages.len() as u32, w.pm_placed);
    assert!(events.pages.iter().all(|p| p
        .payload
        .as_ref()
        .is_some_and(|a| a.type_url.ends_with("orders.v1.FulfillmentStarted"))));
}

// --- one destination per policy (C-0583; decision 14.24) ----------------------

#[given(regex = r#"^an? "([^"]*)" policy translating "([^"]*)" events into "([^"]*)" commands$"#)]
async fn policy_translating(w: &mut HandlingWorld, name: String, input: String, output: String) {
    w.policy = Some((name, input, output));
}

#[given(
    regex = r#"^its business code answers an OrderPlaced event with a (\w+) command for "([^"]*)" and a (\w+) command for "([^"]*)"$"#
)]
async fn policy_answers(
    w: &mut HandlingWorld,
    first: String,
    first_domain: String,
    second: String,
    second_domain: String,
) {
    w.policy_strays = vec![(first, first_domain), (second, second_domain)];
}

#[when("the OrderPlaced event is handled")]
async fn policy_handles(w: &mut HandlingWorld) {
    let (name, input, output) = w.policy.clone().expect("a policy");
    let commands: Vec<pb::CommandBook> = w
        .policy_strays
        .iter()
        .map(|(command, domain)| conf::order_command_for(domain, command))
        .collect();
    let policy = angzarr_router::policy::PolicyDispatch::new(name, input.clone(), output)
        .on_event("orders.v1.OrderPlaced", move |_e, _c| Ok(commands.clone()));
    let router = angzarr_router::router::RouterBuilder::new()
        .policy(policy)
        .build()
        .expect("one policy");
    let mut source = conf::order_placed_source();
    source.cover.as_mut().expect("a cover").domain = input;
    w.policy_result = Some(router.dispatch_policy(&source));
}

#[then("the policy's answer fails with INTERNAL status")]
async fn policy_fails_internal(w: &mut HandlingWorld) {
    let err = w
        .policy_result
        .as_ref()
        .expect("the policy handled")
        .as_ref()
        .expect_err("refused");
    assert_eq!(err.grpc, GrpcCode::Internal);
}

#[then("the answer carries no commands")]
async fn policy_no_commands(w: &mut HandlingWorld) {
    // A refused answer is a status with no PolicyResponse: it carries none.
    assert!(w.policy_result.as_ref().expect("handled").is_err());
}

// --- one component per business process (C-0580; decision 14.19) ------------

#[given(r#"the "order" business binary has its "order" aggregate registered"#)]
async fn order_registered(w: &mut HandlingWorld) {
    let order = conf::order_aggregate(&w.rules_version, w.settle.clone(), w.seen.clone());
    w.registered = Some(Registered(
        angzarr_router::router::RouterBuilder::new()
            .aggregate(order)
            .build()
            .expect("the order aggregate"),
    ));
}

// The native builder takes a process's component at once: registering a
// second beside the order aggregate refuses the whole configuration, and the
// process keeps the one it has.
#[when(
    regex = r#"^an? (aggregate|policy|process manager|projector) component "([^"]*)" is registered in the same business process$"#
)]
async fn second_registered(w: &mut HandlingWorld, kind: String, name: String) {
    let order = conf::order_aggregate(&w.rules_version, w.settle.clone(), w.seen.clone());
    let builder = angzarr_router::router::RouterBuilder::new().aggregate(order);
    let builder = match kind.as_str() {
        "aggregate" => builder.aggregate(angzarr_router::domain::DomainDispatch::new(
            name.clone(),
            name,
            angzarr_router::rebuild::Rebuilder::new(|| ()),
        )),
        "policy" => builder.policy(angzarr_router::policy::PolicyDispatch::new(
            name,
            "order",
            "inventory",
        )),
        "process manager" => builder.process_manager(conf::fulfillment_pm(&w.rules_version)),
        _ => builder.projector(angzarr_router::projector::ProjectorDispatch::new(
            name,
            || (),
        )),
    };
    w.second_refusal = builder.build().err();
}

#[then(
    regex = r#"^the registration is refused with reason (\w+), naming the "order" aggregate already registered$"#
)]
async fn refused_naming_order(w: &mut HandlingWorld, reason: String) {
    let err = w.second_refusal.as_ref().expect("refused");
    assert_eq!(err.code, reason);
    assert_eq!(
        err.extras.get("component").map(String::as_str),
        Some("order")
    );
    assert_eq!(
        err.extras.get("component_kind").map(String::as_str),
        Some("AGGREGATE")
    );
}

#[then(r#"the business process describes only the "order" aggregate"#)]
async fn describes_only_order(w: &mut HandlingWorld) {
    let hello = w
        .registered
        .as_ref()
        .expect("registered")
        .0
        .component_descriptor()
        .expect("its description");
    assert_eq!(hello.name, "order");
    assert_eq!(hello.kind(), pb::ComponentKind::Aggregate);
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

// --- errors are a key and an argument map (decision 14.37) --------------------
//
// The extras render as the status's google.rpc.ErrorInfo metadata (see
// crates/router-ffi/src/abi.rs, coded_to_status_bytes).

fn grpc_name(code: GrpcCode) -> &'static str {
    match code {
        GrpcCode::Unknown => "UNKNOWN",
        GrpcCode::InvalidArgument => "INVALID_ARGUMENT",
        GrpcCode::NotFound => "NOT_FOUND",
        GrpcCode::AlreadyExists => "ALREADY_EXISTS",
        GrpcCode::PermissionDenied => "PERMISSION_DENIED",
        GrpcCode::OutOfRange => "OUT_OF_RANGE",
        GrpcCode::Unavailable => "UNAVAILABLE",
        GrpcCode::FailedPrecondition => "FAILED_PRECONDITION",
        GrpcCode::Aborted => "ABORTED",
        GrpcCode::Unimplemented => "UNIMPLEMENTED",
        GrpcCode::Internal => "INTERNAL",
        GrpcCode::DataLoss => "DATA_LOSS",
    }
}

/// `name "value"` and bare `name` (present, any value) pairs of
/// `sequence "3" and state_type_url`.
fn expected_metadata(text: &str) -> Vec<(String, Option<String>)> {
    text.split(" and ")
        .map(|part| {
            let part = part.trim();
            match part.split_once(' ') {
                Some((name, value)) => (
                    name.to_string(),
                    Some(value.trim().trim_matches('"').to_string()),
                ),
                None => (part.to_string(), None),
            }
        })
        .collect()
}

fn add_item_fails(w: &mut HandlingWorld, fails: conf::AddItemFails) {
    w.fails = Some(fails);
}

#[given(
    regex = r#"^business code rejects AddItem with code (\w+) and arg (\w+) "([^"]*)", and logs "([^"]*)" for itself$"#
)]
async fn rejects_with_code(
    w: &mut HandlingWorld,
    code: String,
    name: String,
    value: String,
    logs: String,
) {
    add_item_fails(
        w,
        conf::AddItemFails::Rejects {
            status: angzarr_router::error::BusinessStatus::FailedPrecondition,
            code,
            args: vec![(name, value)],
            logs,
        },
    );
}

#[given(
    regex = r#"^business code refuses AddItem with the refusal object ItemOutOfStock with item "([^"]*)" and on_hand (\d+)$"#
)]
async fn refuses_typed(w: &mut HandlingWorld, item: String, on_hand: u32) {
    add_item_fails(w, conf::AddItemFails::Refuses { item, on_hand });
}

#[given(
    regex = r#"^business code for AddItem returns an unclassified error whose text is "([^"]*)"$"#
)]
async fn errs_with_text(w: &mut HandlingWorld, text: String) {
    add_item_fails(w, conf::AddItemFails::Errs { text });
}

#[given(regex = r#"^business code for AddItem panics with "([^"]*)"$"#)]
async fn panics_with(w: &mut HandlingWorld, text: String) {
    add_item_fails(w, conf::AddItemFails::Panics { text });
}

#[given(
    regex = r#"^business code for AddItem fails with code (\w+) and arg (\w+) "([^"]*)", and logs "([^"]*)" for itself$"#
)]
async fn fails_with_bug(
    w: &mut HandlingWorld,
    code: String,
    name: String,
    value: String,
    logs: String,
) {
    add_item_fails(
        w,
        conf::AddItemFails::Bug {
            code,
            args: vec![(name, value)],
            logs,
        },
    );
}

#[then(
    regex = r"^(?:the command|it) is refused (\w+) with reason (\w+), which is also its status message$"
)]
async fn refused_with_reason(w: &mut HandlingWorld, status: String, reason: String) {
    let err = w.the_error();
    assert_eq!(err.code, reason, "the reason key ({err:?})");
    assert_eq!(grpc_name(err.grpc), status, "the gRPC status");
    assert_eq!(
        err.status_message(),
        reason,
        "the status message IS the reason key (decision 14.37)"
    );
}

#[then(regex = r"^the refusal's error info carries (.+)$")]
async fn error_info_carries(w: &mut HandlingWorld, metadata: String) {
    let err = w.the_error();
    match metadata.as_str() {
        "no other metadata" => assert!(
            err.extras.is_empty(),
            "expected no metadata, got {:?}",
            err.extras
        ),
        "no cause" => assert!(
            !err.extras
                .keys()
                .any(|k| k == "cause" || k.starts_with("cause.")),
            "expected no cause, got {:?}",
            err.extras
        ),
        _ => {
            for (name, value) in expected_metadata(&metadata) {
                let carried = err.extras.get(&name);
                match value {
                    Some(value) => assert_eq!(
                        carried,
                        Some(&value),
                        "error info {name} (all: {:?})",
                        err.extras
                    ),
                    None => assert!(
                        carried.is_some_and(|v| !v.is_empty()),
                        "error info carries {name} (all: {:?})",
                        err.extras
                    ),
                }
            }
        }
    }
}

#[then(
    regex = r#"^the refusal's status details carry the ItemOutOfStock with item "([^"]*)" and on_hand (\d+)$"#
)]
async fn details_carry_out_of_stock(w: &mut HandlingWorld, item: String, on_hand: u32) {
    let refusal = w
        .the_error()
        .refusal
        .as_deref()
        .expect("the typed refusal in the status details");
    assert_eq!(refusal.type_url, "/orders.v1.ItemOutOfStock");
    assert_eq!(
        conf::orders::v1::ItemOutOfStock::decode(refusal.value.as_slice()).expect("ItemOutOfStock"),
        conf::orders::v1::ItemOutOfStock { item, on_hand }
    );
}

#[then(regex = r#"^no part of the answer carries the text "([^"]*)"$"#)]
async fn no_text(w: &mut HandlingWorld, text: String) {
    let err = w.the_error();
    for (part, shown) in [
        ("status message", err.status_message().to_string()),
        ("display", err.to_string()),
        ("error", format!("{err:?}")),
    ] {
        assert!(
            !shown.contains(&text),
            "the {part} carries {text:?}: {shown}"
        );
    }
}

// C-0670's conditions, each driven through the router's public API.

fn with_payload_type(mut req: pb::ContextualCommand, type_url: &str) -> pb::ContextualCommand {
    let page = req
        .command
        .as_mut()
        .and_then(|b| b.page.as_mut())
        .expect("a page");
    let payload = match page.body.as_mut() {
        Some(pb::command_page::Body::Command(c)) => c.payload.as_mut(),
        Some(pb::command_page::Body::Fact(f)) => f.payload.as_mut(),
        None => None,
    };
    payload.expect("a payload").type_url = type_url.to_string();
    req
}

fn condition(w: &mut HandlingWorld, outcome: Result<(), CodedError>) {
    w.outcome = Some(outcome);
}

fn items(n: u64) -> Vec<(String, String)> {
    (0..n)
        .map(|i| ("ItemAdded".to_string(), format!("item-{i}")))
        .collect()
}

#[when(regex = r"^the state fold fails on a persisted event at sequence (\d+)$")]
async fn fold_fails_persisted(w: &mut HandlingWorld, sequence: u64) {
    let mut history = items(sequence);
    history.push(("ItemAdded".to_string(), conf::ROTTEN_ITEM.to_string()));
    w.dispatch(conf::order_command(
        "AddItem", "apple", &history, None, None,
    ));
    let outcome = w.result.take().expect("handled").map(|_| ());
    condition(w, outcome);
}

#[when(regex = r"^the state fold fails on an event the command emitted at sequence (\d+)$")]
async fn fold_fails_emitted(w: &mut HandlingWorld, sequence: u64) {
    let history = items(sequence);
    w.dispatch(conf::order_command(
        "AddItem",
        conf::ROTTEN_ITEM,
        &history,
        None,
        None,
    ));
    let outcome = w.result.take().expect("handled").map(|_| ());
    condition(w, outcome);
}

#[when(regex = r#"^a command of type "([^"]*)" arrives$"#)]
async fn command_of_type(w: &mut HandlingWorld, type_url: String) {
    w.dispatch(with_payload_type(
        conf::order_command("AddItem", "apple", &[], None, None),
        &type_url,
    ));
    let outcome = w.result.take().expect("handled").map(|_| ());
    condition(w, outcome);
}

#[when(regex = r#"^a fact of type "([^"]*)" arrives$"#)]
async fn fact_of_type(w: &mut HandlingWorld, type_url: String) {
    w.dispatch(with_payload_type(
        conf::order_fact("ItemDelivered", "apple"),
        &type_url,
    ));
    let outcome = w.result.take().expect("handled").map(|_| ());
    condition(w, outcome);
}

#[when(regex = r"^a CommandRefused for (\w+) arrives with no on_refused_(\w+)$")]
async fn command_refused_unhandled(w: &mut HandlingWorld, command: String, _none: String) {
    let refused = pb::CommandRefused {
        command: Some(conf::order_command_for("order", &command)),
        ..Default::default()
    };
    let mut req = conf::order_fact("ItemDelivered", "apple");
    let req_page = req
        .command
        .as_mut()
        .and_then(|b| b.page.as_mut())
        .expect("a page");
    req_page.body = Some(pb::command_page::Body::Fact(pb::Fact {
        payload: Some(prost_types::Any {
            type_url: angzarr_router::type_url("io.angzarr.v1.CommandRefused"),
            value: refused.encode_to_vec(),
        }),
    }));
    w.dispatch(req);
    let outcome = w.result.take().expect("handled").map(|_| ());
    condition(w, outcome);
}

#[when(r#"a policy is dispatched to the business process hosting the "order" aggregate"#)]
async fn policy_to_aggregate(w: &mut HandlingWorld) {
    let order = conf::order_aggregate(&w.rules_version, w.settle.clone(), w.seen.clone());
    let router = angzarr_router::router::RouterBuilder::new()
        .aggregate(order)
        .build()
        .expect("the order aggregate");
    condition(
        w,
        router
            .dispatch_policy(&conf::order_placed_source())
            .map(|_| ()),
    );
}

fn fulfillment_policy(input: &str) -> angzarr_router::router::Router {
    angzarr_router::router::RouterBuilder::new()
        .policy(angzarr_router::policy::PolicyDispatch::new(
            "OrderFulfillment",
            input,
            "fulfillment",
        ))
        .build()
        .expect("one policy")
}

#[when(regex = r#"^a policy consuming "([^"]*)" is given a book of domain "([^"]*)"$"#)]
async fn policy_given_other_domain(w: &mut HandlingWorld, input: String, domain: String) {
    let mut source = conf::order_placed_source();
    source.cover.as_mut().expect("a cover").domain = domain;
    condition(
        w,
        fulfillment_policy(&input)
            .dispatch_policy(&source)
            .map(|_| ()),
    );
}

#[when("the router is built with two components")]
async fn built_with_two_components(w: &mut HandlingWorld) {
    let order = conf::order_aggregate(&w.rules_version, w.settle.clone(), w.seen.clone());
    let built = angzarr_router::router::RouterBuilder::new()
        .aggregate(order)
        .aggregate(angzarr_router::domain::DomainDispatch::new(
            "payment",
            "payment",
            angzarr_router::rebuild::Rebuilder::new(|| ()),
        ))
        .build();
    condition(w, built.map(|_| ()));
}

#[when("the router is built with two upcaster chains")]
async fn built_with_two_chains(w: &mut HandlingWorld) {
    let order = conf::order_aggregate(&w.rules_version, w.settle.clone(), w.seen.clone());
    let built = angzarr_router::router::RouterBuilder::new()
        .aggregate(order)
        .upcaster(angzarr_router::upcaster::UpcasterDispatch::new(
            "one", "order",
        ))
        .upcaster(angzarr_router::upcaster::UpcasterDispatch::new(
            "two", "order",
        ))
        .build();
    condition(w, built.map(|_| ()));
}

/// A refusal of a RemoveEverything the order aggregate's policy sent, whole:
/// its triggering event's stream, the refused command with its command_id.
fn whole_refusal() -> pb::Refusal {
    let mut command = conf::order_command_for("inventory", "RemoveEverything");
    command.page.as_mut().expect("a page").command_id = Some(pb::Uuid {
        value: conf::root_of("refused-1").into(),
    });
    pb::Refusal {
        event: Some(pb::EventBook {
            cover: Some(conf::cover_of("order", "order-1")),
            ..conf::order_placed_source()
        }),
        command: Some(command),
        response: None,
    }
}

/// A bare refusal fact, as a policy's OnRefused answers it: a Fact page,
/// no cover, no identity, no Handling.
fn bare_fact() -> pb::OnRefusedResponse {
    pb::OnRefusedResponse {
        fact: Some(pb::CommandBook {
            page: Some(pb::CommandPage {
                body: Some(pb::command_page::Body::Fact(pb::Fact {
                    payload: Some(prost_types::Any {
                        type_url: angzarr_router::type_url("orders.v1.ItemDelivered"),
                        value: Vec::new(),
                    }),
                })),
                ..Default::default()
            }),
            ..Default::default()
        }),
    }
}

fn address(w: &mut HandlingWorld, refusal: pb::Refusal, answer: pb::OnRefusedResponse) {
    condition(
        w,
        angzarr_router::policy::address_refusal_fact(&refusal, &answer).map(|_| ()),
    );
}

#[when("a policy's OnRefused answer holds no fact")]
async fn answer_holds_no_fact(w: &mut HandlingWorld) {
    address(w, whole_refusal(), pb::OnRefusedResponse { fact: None });
}

#[when("a policy's refusal fact sets its own cover or handling")]
async fn fact_sets_cover(w: &mut HandlingWorld) {
    let mut answer = bare_fact();
    answer.fact.as_mut().expect("a fact").cover = Some(conf::cover_of("order", "order-1"));
    address(w, whole_refusal(), answer);
}

#[when("a policy's refusal fact has no page")]
async fn fact_has_no_page(w: &mut HandlingWorld) {
    let mut answer = bare_fact();
    answer.fact.as_mut().expect("a fact").page = None;
    address(w, whole_refusal(), answer);
}

#[when("a policy's refusal fact's page is a Command")]
async fn fact_page_is_command(w: &mut HandlingWorld) {
    let mut answer = bare_fact();
    answer.fact = Some(conf::order_command_for("order", "AddItem"));
    answer.fact.as_mut().expect("a fact").cover = None;
    address(w, whole_refusal(), answer);
}

#[when("a policy's refusal fact sets its own command_id")]
async fn fact_sets_command_id(w: &mut HandlingWorld) {
    let mut answer = bare_fact();
    answer
        .fact
        .as_mut()
        .and_then(|f| f.page.as_mut())
        .expect("a page")
        .command_id = Some(pb::Uuid {
        value: conf::root_of("made-up").into(),
    });
    address(w, whole_refusal(), answer);
}

#[when("a refusal to address names no refused command")]
async fn no_refused_command(w: &mut HandlingWorld) {
    let refusal = pb::Refusal {
        command: None,
        ..whole_refusal()
    };
    address(w, refusal, bare_fact());
}

#[when("a refusal to address names a refused command with no command_id")]
async fn refused_command_without_id(w: &mut HandlingWorld) {
    let mut refusal = whole_refusal();
    refusal
        .command
        .as_mut()
        .and_then(|c| c.page.as_mut())
        .expect("a page")
        .command_id = None;
    address(w, refusal, bare_fact());
}

#[when("a refusal to address names no triggering stream")]
async fn no_trigger_stream(w: &mut HandlingWorld) {
    let refusal = pb::Refusal {
        event: None,
        ..whole_refusal()
    };
    address(w, refusal, bare_fact());
}

#[when(regex = r"^a policy is given a Refusal with no (command|event)$")]
async fn policy_given_partial_refusal(w: &mut HandlingWorld, missing: String) {
    let mut refusal = whole_refusal();
    match missing.as_str() {
        "command" => refusal.command = None,
        _ => refusal.event = None,
    }
    condition(
        w,
        fulfillment_policy("order")
            .dispatch_refusal(
                &refusal,
                &angzarr_router::context::CommandContext::default(),
            )
            .map(|_| ()),
    );
}

#[when("the host gives the router bytes that are no ContextualCommand")]
async fn undecodable_bytes(w: &mut HandlingWorld) {
    condition(
        w,
        angzarr_router::decode_message::<pb::ContextualCommand>(&[0xff, 0xff, 0xff]).map(|_| ()),
    );
}

// --- when the input was accepted (C-0671; decision 14.37) ---------------------

#[given(regex = r#"^the delivery says its input was accepted at "([^"]*)"$"#)]
async fn accepted_at(w: &mut HandlingWorld, at: String) {
    w.recorded_at = Some(at.parse().expect("an RFC 3339 timestamp"));
}

#[then(regex = r#"^business code read "([^"]*)" as when its input was accepted$"#)]
async fn read_accepted_at(w: &mut HandlingWorld, at: String) {
    let expected: prost_types::Timestamp = at.parse().expect("an RFC 3339 timestamp");
    assert_eq!(
        w.observed().recorded_at,
        Some(expected),
        "CommandContext.recorded_at is the delivery's ContextualCommand.recorded_at"
    );
}

// The order fixtures read no clock: what business code read is the
// delivery's acceptance time, exactly.
#[then("business code read no clock of its own for it")]
async fn read_no_clock(w: &mut HandlingWorld) {
    assert_eq!(
        w.observed().recorded_at,
        w.recorded_at,
        "business code read the delivered acceptance time, not a clock"
    );
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
