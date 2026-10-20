//! Binds angzarr-project's handler-contract tier
//! (features/handler-contract/handling.feature, C-0533..C-0550, C-0565, C-0570, C-0571, C-0580 and C-0583; the spec is
//! the source, read from the submodule) to the Rust core: the order
//! aggregate and the Fulfillment process manager built by hand over the
//! orders.v1 fixture messages.

use angzarr_router::error::{CodedError, GrpcCode};
use angzarr_router::pb;
use angzarr_router_conformance as conf;
use cucumber::{given, then, when, World};

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
    pm_result: Option<Result<pb::ProcessManagerHandleResponse, CodedError>>,
    pm_placed: u32,
    saga: Option<(String, String, String)>,
    saga_strays: Vec<(String, String)>,
    saga_result: Option<Result<pb::SagaResponse, CodedError>>,
    registered: Option<Registered>,
    second_refusal: Option<CodedError>,
    described: Vec<pb::ComponentDescriptor>,
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
    regex = r"^the process manager handles an OrderPlaced event (and writes a FulfillmentStarted event|without writing a process event)$"
)]
async fn pm_handles_writing(w: &mut HandlingWorld, writing: String) {
    let writes = writing.starts_with("and writes");
    w.pm_placed = u32::from(writes);
    let pm = conf::fulfillment_pm_writing(&w.pm_rules_version, writes);
    w.pm_result = Some(pm.dispatch(&conf::order_placed_trigger()));
}

fn pm_answer(w: &HandlingWorld) -> &pb::ProcessManagerHandleResponse {
    w.pm_result
        .as_ref()
        .expect("the PM handled")
        .as_ref()
        .expect("the PM answered")
}

#[then("the answer's process state is the process manager's state after the reaction")]
async fn pm_process_state(w: &mut HandlingWorld) {
    assert_eq!(
        pm_answer(w)
            .process_state
            .as_ref()
            .and_then(|s| s.state.clone()),
        Some(conf::fulfillment_state(w.pm_placed))
    );
}

#[then("none of the answer's process events carry a state")]
async fn pm_events_carry_no_state(w: &mut HandlingWorld) {
    assert!(pm_answer(w)
        .process_events
        .iter()
        .all(|b| b.snapshot.is_none()));
}

// --- one destination per saga (C-0583; decision 14.24) ----------------------

#[given(regex = r#"^an? "([^"]*)" saga translating "([^"]*)" events into "([^"]*)" commands$"#)]
async fn saga_translating(w: &mut HandlingWorld, name: String, input: String, output: String) {
    w.saga = Some((name, input, output));
}

#[given(
    regex = r#"^its business code answers an OrderPlaced event with a (\w+) command for "([^"]*)" and a (\w+) command for "([^"]*)"$"#
)]
async fn saga_answers(
    w: &mut HandlingWorld,
    first: String,
    first_domain: String,
    second: String,
    second_domain: String,
) {
    w.saga_strays = vec![(first, first_domain), (second, second_domain)];
}

#[when("the OrderPlaced event is handled")]
async fn saga_handles(w: &mut HandlingWorld) {
    let (name, input, output) = w.saga.clone().expect("a saga");
    let commands: Vec<pb::CommandBook> = w
        .saga_strays
        .iter()
        .map(|(command, domain)| conf::order_command_for(domain, command))
        .collect();
    let saga = angzarr_router::saga::SagaDispatch::new(name, input.clone(), output)
        .on_event("orders.v1.OrderPlaced", move |_e, _c| Ok(commands.clone()));
    let router = angzarr_router::router::RouterBuilder::new()
        .saga(saga)
        .build()
        .expect("one saga");
    let mut source = conf::order_placed_trigger().trigger.expect("a trigger");
    source.cover.as_mut().expect("a cover").domain = input;
    w.saga_result = Some(router.dispatch_saga(&source));
}

#[then("the saga's answer fails with INTERNAL status")]
async fn saga_fails_internal(w: &mut HandlingWorld) {
    let err = w
        .saga_result
        .as_ref()
        .expect("the saga handled")
        .as_ref()
        .expect_err("refused");
    assert_eq!(err.grpc, GrpcCode::Internal);
}

#[then("the answer carries no commands")]
async fn saga_no_commands(w: &mut HandlingWorld) {
    // A refused answer is a status with no SagaResponse: it carries none.
    assert!(w.saga_result.as_ref().expect("handled").is_err());
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
    regex = r#"^an? (aggregate|saga|process manager|projector) component "([^"]*)" is registered in the same business process$"#
)]
async fn second_registered(w: &mut HandlingWorld, kind: String, name: String) {
    let order = conf::order_aggregate(&w.rules_version, w.settle.clone(), w.seen.clone());
    let builder = angzarr_router::router::RouterBuilder::new().aggregate(order);
    let builder = match kind.as_str() {
        "aggregate" => builder.aggregate(angzarr_router::aggregate::AggregateDispatch::new(
            name.clone(),
            name,
            angzarr_router::rebuild::Rebuilder::new(|| ()),
        )),
        "saga" => builder.saga(angzarr_router::saga::SagaDispatch::new(
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

#[then(r#"the registration is refused, naming the "order" aggregate already registered"#)]
async fn refused_naming_order(w: &mut HandlingWorld) {
    let err = w.second_refusal.as_ref().expect("refused");
    assert_eq!(
        err.code,
        angzarr_router::error::codes::DUPLICATE_REGISTRATION
    );
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
