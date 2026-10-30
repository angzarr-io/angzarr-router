//! Binds angzarr-project's handler-contract tier
//! (features/handler-contract/handling.feature; the spec is the source, read
//! from the submodule) to the Rust core: the order aggregate, the
//! Fulfillment process manager (a domain, decision 14.31) and the
//! OrderFulfillment policy, built by hand over the tier's orders.v1 fixture
//! messages.

mod common;

use angzarr_router::error::{CodedError, GrpcCode, RetryableStatus};
use angzarr_router::pb;
use angzarr_router_conformance as conf;
use cucumber::writer::Stats as _;
use cucumber::{given, then, when, World};
use prost::Message;
use std::sync::{Arc, Mutex};

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
    /// What the policy's business code answers each OrderPlaced with.
    policy_commands: Vec<pb::CommandBook>,
    /// The policy's ReserveStock refusal callback returns this fact, when
    /// it has one (C-0691).
    translates_to: Option<prost_types::Any>,
    /// The refusals the policy's callback was called with.
    translated: Arc<Mutex<Vec<pb::Refusal>>>,
    /// The refusal handed to the policy, and its answer (C-0691, C-0692).
    refusal: Option<pb::Refusal>,
    refusal_answer: Option<Result<pb::OnRefusedResponse, CodedError>>,
    /// The pages of a delivery after its snapshot (C-0688).
    delivery_pages: Vec<pb::EventPage>,
    policy_result: Option<Result<pb::PolicyResponse, CodedError>>,
    registered: Option<Registered>,
    second_refusal: Option<CodedError>,
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
                 ROUTER_REASON_UNHANDLED_HANDLER_ERROR with cause ROUTER_REASON_HANDLER_PANICKED: {text:?}"
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

// --- policies: the router translates, the coordinator checks (14.42) ---------

#[given(regex = r#"^an? "([^"]*)" policy translating "([^"]*)" events into "([^"]*)" commands$"#)]
async fn policy_translating(w: &mut HandlingWorld, name: String, input: String, output: String) {
    w.policy = Some((name, input, output));
}

/// A ReserveStock command for "inventory", as the policy's business code
/// answers an OrderPlaced with.
fn reserve_stock() -> pb::CommandBook {
    conf::order_command_for("inventory", "ReserveStock")
}

#[given("its business code answers each OrderPlaced event with one ReserveStock command")]
async fn answers_reserve_stock(w: &mut HandlingWorld) {
    w.policy_commands = vec![reserve_stock()];
}

#[given(
    regex = r#"^its business code answers an OrderPlaced event with a ShipOrder command for "([^"]*)" that names no root, no edition and another correlation$"#
)]
async fn answers_unchecked(w: &mut HandlingWorld, domain: String) {
    let mut command = conf::order_command_for(&domain, "ShipOrder");
    let cover = command.cover.as_mut().expect("a cover");
    cover.root = None;
    cover.edition = None;
    command.page.as_mut().expect("a page").correlation_id = Some(pb::Uuid {
        value: conf::root_of("another-correlation").into(),
    });
    w.policy_commands = vec![command];
}

/// The policy from `policy_translating`, answering each OrderPlaced with
/// `policy_commands`, handling `source`.
fn handle_policy(w: &mut HandlingWorld, source: pb::EventBook) {
    let (name, input, output) = w.policy.clone().expect("a policy");
    let commands = w.policy_commands.clone();
    let policy = angzarr_router::policy::PolicyDispatch::new(name, input, output)
        .on_event("orders.v1.OrderPlaced", move |_e, _c| Ok(commands.clone()));
    let router = angzarr_router::router::RouterBuilder::new()
        .policy(policy)
        .build()
        .expect("one policy");
    w.policy_result = Some(router.dispatch_policy(&source));
}

/// An "order" book of `events` ((type, sequence)), from the order stream
/// "order-1".
fn order_book(events: &[(&str, u64)]) -> pb::EventBook {
    pb::EventBook {
        cover: Some(conf::cover_of("order", "order-1")),
        pages: events
            .iter()
            .map(|(kind, sequence)| pb::EventPage {
                sequence: *sequence,
                payload: Some(prost_types::Any {
                    type_url: angzarr_router::type_url(&format!("orders.v1.{kind}")),
                    value: Vec::new(),
                }),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

#[when("the OrderPlaced event is handled")]
async fn policy_handles(w: &mut HandlingWorld) {
    handle_policy(w, order_book(&[("OrderPlaced", 1)]));
}

#[when(
    regex = r"^a book holding OrderPlaced at sequence (\d+), an OrderNoteAdded at sequence (\d+) and OrderPlaced at sequence (\d+) is handled$"
)]
async fn policy_handles_book(w: &mut HandlingWorld, first: u64, note: u64, second: u64) {
    handle_policy(
        w,
        order_book(&[
            ("OrderPlaced", first),
            ("OrderNoteAdded", note),
            ("OrderPlaced", second),
        ]),
    );
}

fn policy_answer(w: &HandlingWorld) -> &pb::PolicyResponse {
    w.policy_result
        .as_ref()
        .expect("the policy handled")
        .as_ref()
        .expect("the policy answered")
}

#[then("the answer is not refused")]
async fn policy_not_refused(w: &mut HandlingWorld) {
    policy_answer(w);
}

#[then("the answer's one reaction carries that ShipOrder command unchanged")]
async fn one_reaction_unchanged(w: &mut HandlingWorld) {
    let reactions = &policy_answer(w).reactions;
    assert_eq!(reactions.len(), 1, "one reaction");
    assert_eq!(reactions[0].commands, w.policy_commands, "unchanged");
}

#[then(regex = r"^the answer holds (\w+) reactions, in page order:$")]
async fn holds_reactions(w: &mut HandlingWorld, _count: String, step: &cucumber::gherkin::Step) {
    let table = step.table.as_ref().expect("a table");
    let expected: Vec<Vec<String>> = table.rows.iter().skip(1).cloned().collect();
    let actual: Vec<Vec<String>> = policy_answer(w)
        .reactions
        .iter()
        .map(|r| {
            let types: Vec<&str> = r
                .commands
                .iter()
                .map(|c| {
                    angzarr_router::command_payload(c.page.as_ref().expect("a page"))
                        .map(|p| angzarr_router::type_name_from_url(&p.type_url))
                        .expect("a command")
                })
                .collect();
            let commands = match types.as_slice() {
                [one] => format!("one {}", one.trim_start_matches("orders.v1.")),
                other => panic!("expected one command per reaction, got {other:?}"),
            };
            vec![r.trigger_sequence.to_string(), commands]
        })
        .collect();
    assert_eq!(actual, expected);
}

#[then(regex = r"^no reaction names sequence (\d+)$")]
async fn no_reaction_for(w: &mut HandlingWorld, sequence: u64) {
    assert!(policy_answer(w)
        .reactions
        .iter()
        .all(|r| r.trigger_sequence != sequence));
}

// --- OnRefused: a translation or none (C-0691, C-0692; decision 14.42) ---------

#[given(
    regex = r#"^an "([^"]*)" policy whose (\w+) refusals are translated by a callback returning the "([^"]*)" fact (\w+)$"#
)]
async fn policy_translating_refusals(
    w: &mut HandlingWorld,
    name: String,
    _command: String,
    input: String,
    fact: String,
) {
    w.policy = Some((name, input, "inventory".to_string()));
    w.translates_to = Some(prost_types::Any {
        type_url: angzarr_router::type_url(&format!("orders.v1.{fact}")),
        value: conf::orders::v1::StockUnavailable {
            item: "apple".to_string(),
        }
        .encode_to_vec(),
    });
}

#[given(regex = r#"^an "([^"]*)" policy with no callback for (\w+) refusals$"#)]
async fn policy_without_callback(w: &mut HandlingWorld, name: String, _command: String) {
    w.policy = Some((name, "order".to_string(), "inventory".to_string()));
}

#[when(regex = r"^the policy is given the refusal of a (\w+) it sent$")]
async fn policy_given_refusal(w: &mut HandlingWorld, command: String) {
    let (name, input, output) = w.policy.clone().expect("a policy");
    let mut sent = conf::order_command_for(&output, &command);
    sent.page.as_mut().expect("a page").command_id = Some(pb::Uuid {
        value: conf::root_of("refused-1").into(),
    });
    let refusal = pb::Refusal {
        event: Some(order_book(&[("OrderPlaced", 1)])),
        command: Some(sent),
        response: None,
    };
    let mut policy = angzarr_router::policy::PolicyDispatch::new(name, input, output);
    if let Some(fact) = w.translates_to.clone() {
        let calls = w.translated.clone();
        policy = policy.on_refused(&format!("orders.v1.{command}"), move |refusal, _ctx| {
            calls.lock().unwrap().push(refusal.clone());
            Ok(fact.clone())
        });
    }
    w.refusal_answer = Some(policy.dispatch_refusal(
        &refusal,
        &angzarr_router::context::CommandContext::default(),
    ));
    w.refusal = Some(refusal);
}

fn refusal_answer(w: &HandlingWorld) -> &pb::OnRefusedResponse {
    w.refusal_answer
        .as_ref()
        .expect("the policy was given a refusal")
        .as_ref()
        .expect("the policy answered")
}

#[then("the callback is called with that refusal")]
async fn callback_called(w: &mut HandlingWorld) {
    assert_eq!(
        *w.translated.lock().unwrap(),
        vec![w.refusal.clone().expect("a refusal")]
    );
}

#[then(regex = r"^the answer's fact page carries (\w+) as its only content$")]
async fn fact_page_only(w: &mut HandlingWorld, fact: String) {
    let page = refusal_answer(w)
        .fact
        .as_ref()
        .and_then(|f| f.page.as_ref())
        .expect("a fact page");
    let expected = pb::CommandPage {
        body: Some(pb::command_page::Body::Fact(pb::Fact {
            payload: w.translates_to.clone(),
        })),
        ..Default::default()
    };
    assert!(w
        .translates_to
        .as_ref()
        .is_some_and(|a| a.type_url.ends_with(&fact)));
    assert_eq!(*page, expected, "the payload alone");
}

#[then("the answer's fact sets no cover, no handling and no identity")]
async fn fact_unaddressed(w: &mut HandlingWorld) {
    let fact = refusal_answer(w).fact.as_ref().expect("a fact");
    assert_eq!(fact.cover, None);
    assert_eq!(fact.handling, None);
    let page = fact.page.as_ref().expect("a page");
    assert_eq!(page.command_id, None);
    assert_eq!(page.correlation_id, None);
    assert_eq!(page.causation_id, None);
    assert_eq!(page.fact_origin, None);
}

#[then("the answer carries no fact")]
async fn no_fact(w: &mut HandlingWorld) {
    assert_eq!(refusal_answer(w).fact, None, "no translation");
}

// --- one component per business process (C-0580; decision 14.19) ------------

#[given(r#"the "order" business binary has its "order" aggregate registered"#)]
async fn order_registered(w: &mut HandlingWorld) {
    let order = conf::order_aggregate(&w.rules_version, w.settle.clone(), w.seen.clone());
    // The descriptor angzarr-cli generates for it (decision 14.42).
    let descriptor = pb::ComponentDescriptor {
        name: "order".to_string(),
        kind: pb::ComponentKind::Aggregate as i32,
        rules_version: w.rules_version.clone(),
        ..Default::default()
    };
    w.registered = Some(Registered(
        angzarr_router::router::RouterBuilder::new()
            .aggregate(order)
            .descriptor(descriptor)
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

#[then(regex = r#"^business code saw items (.+) in its state$"#)]
async fn saw_items(w: &mut HandlingWorld, items: String) {
    // `"pear" and "plum"`, or `"pear", "plum" and "fig"`; the fixture
    // records the state's items sorted.
    let mut expected: Vec<String> = items
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();
    expected.sort();
    assert_eq!(w.observed().items, expected);
}

// --- folding what it is given (C-0688; decision 14.40) -------------------------

#[given(
    regex = r#"^a delivered snapshot at sequence (\d+) of the order state with item "([^"]*)"$"#
)]
async fn snapshot_at(w: &mut HandlingWorld, sequence: u64, item: String) {
    w.snapshot = Some(pb::Snapshot {
        sequence,
        ..conf::order_snapshot(&[&item], "")
    });
}

#[given(regex = r"^the delivery's pages are (.+)$")]
async fn delivery_pages(w: &mut HandlingWorld, pages: String) {
    // `ItemAdded "plum" at sequence 5 and ItemAdded "fig" at sequence 6`.
    w.delivery_pages = pages
        .split(" and ")
        .map(|page| {
            let mut parts = page.split('"');
            let kind = parts.next().expect("a kind").trim().to_string();
            let item = parts.next().expect("an item").to_string();
            let sequence = parts
                .next()
                .and_then(|rest| rest.trim().strip_prefix("at sequence "))
                .and_then(|n| n.parse().ok())
                .expect("at sequence N");
            let mut req = conf::order_command("AddItem", "", &[(kind, item)], None, None);
            let mut page = req.events.take().expect("a book").pages.remove(0);
            page.sequence = sequence;
            page
        })
        .collect();
}

#[when(regex = r#"^AddItem "([^"]*)" is handled over that delivery$"#)]
async fn over_delivery(w: &mut HandlingWorld, item: String) {
    let mut req = conf::order_command("AddItem", &item, &[], None, w.snapshot.clone());
    let book = req.events.as_mut().expect("a book");
    book.next_sequence = w.delivery_pages.last().map_or(0, |p| p.sequence + 1);
    book.pages = w.delivery_pages.clone();
    w.dispatch(req);
}

// --- the conflict report (C-0542 to C-0550; decision 14.42) -------------------

fn reported(w: &HandlingWorld) -> pb::ConflictReport {
    w.accepted()
        .conflict
        .clone()
        .expect("the answer reports the conflict")
}

#[then(regex = r#"^the answer reports resolved nonce "([^"]*)"$"#)]
async fn reports_nonce(w: &mut HandlingWorld, nonce: String) {
    assert_eq!(reported(w).resolved_nonce, nonce.into_bytes());
}

#[then("the answer reports no resolved nonce")]
async fn reports_no_nonce(w: &mut HandlingWorld) {
    assert!(reported(w).resolved_nonce.is_empty(), "an empty nonce");
}

/// `"items[pear]"`, or several quoted units joined by commas and "and".
fn quoted_units(text: &str) -> Vec<String> {
    text.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

#[then(regex = r#"^the answer reports window units (".*") and command units (".*")$"#)]
async fn reports_units(w: &mut HandlingWorld, window: String, command: String) {
    let report = reported(w);
    assert_eq!(report.window_units, quoted_units(&window), "window units");
    assert_eq!(
        report.command_units,
        quoted_units(&command),
        "command units"
    );
}

#[then("the answer carries business code's events")]
async fn carries_events(w: &mut HandlingWorld) {
    let item = w.observed().item;
    let events = w.accepted().events.as_ref().expect("its events");
    let added: Vec<String> = events
        .pages
        .iter()
        .map(|p| {
            let payload = p.payload.as_ref().expect("a payload");
            assert!(payload.type_url.ends_with("orders.v1.ItemAdded"));
            conf::orders::v1::ItemAdded::decode(payload.value.as_slice())
                .expect("ItemAdded")
                .item
        })
        .collect();
    assert_eq!(added, vec![item], "business code's one ItemAdded");
}

// --- errors are a key and an argument map (decision 14.37) --------------------
//
// The extras render as the status's google.rpc.ErrorInfo metadata.

fn grpc_name(code: GrpcCode) -> &'static str {
    match code {
        GrpcCode::Unknown => "UNKNOWN",
        GrpcCode::InvalidArgument => "INVALID_ARGUMENT",
        GrpcCode::DeadlineExceeded => "DEADLINE_EXCEEDED",
        GrpcCode::ResourceExhausted => "RESOURCE_EXHAUSTED",
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
    regex = r#"^business code for AddItem (?:returns|fails with) an unclassified error whose text is "([^"]*)"$"#
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

// C-0678 (decision 14.37, latest): a bug the handler marks retryable,
// under the status it picks.
#[given(
    regex = r#"^business code for AddItem fails with a retryable bug under (\w+), code (\w+) and arg (\w+) "([^"]*)", and logs "([^"]*)" for itself$"#
)]
async fn fails_with_retryable_bug(
    w: &mut HandlingWorld,
    status: String,
    code: String,
    name: String,
    value: String,
    logs: String,
) {
    add_item_fails(
        w,
        conf::AddItemFails::BugRetryable {
            status: match status.as_str() {
                "UNAVAILABLE" => RetryableStatus::Unavailable,
                "DEADLINE_EXCEEDED" => RetryableStatus::DeadlineExceeded,
                "RESOURCE_EXHAUSTED" => RetryableStatus::ResourceExhausted,
                other => panic!("{other} is no retryable status a handler picks"),
            },
            code,
            args: vec![(name, value)],
            logs,
        },
    );
}

// C-0679 (decision 14.37, latest): a handler's immediate-return business
// error, under its own code.
#[given(
    regex = r#"^business code rejects AddItem for immediate return with code (\w+) and arg (\w+) "([^"]*)", and logs "([^"]*)" for itself$"#
)]
async fn rejects_for_immediate_return(
    w: &mut HandlingWorld,
    code: String,
    name: String,
    value: String,
    logs: String,
) {
    add_item_fails(
        w,
        conf::AddItemFails::Immediate {
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
        _ if metadata.starts_with("no ") => {
            let name = metadata.trim_start_matches("no ");
            assert!(
                !err.extras.contains_key(name),
                "expected no {name}, got {:?}",
                err.extras
            );
        }
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

#[when("a CommandRefused fact arrives whose payload does not decode")]
async fn command_refused_undecodable(w: &mut HandlingWorld) {
    let mut req = conf::order_fact("ItemDelivered", "apple");
    let page = req
        .command
        .as_mut()
        .and_then(|b| b.page.as_mut())
        .expect("a page");
    page.body = Some(pb::command_page::Body::Fact(pb::Fact {
        payload: Some(prost_types::Any {
            type_url: angzarr_router::type_url("io.angzarr.v1.CommandRefused"),
            value: vec![0xff, 0xff, 0xff],
        }),
    }));
    w.dispatch(req);
    let outcome = w.result.take().expect("handled").map(|_| ());
    condition(w, outcome);
}

#[when("a COMMUTATIVE command reaches a state with no resolver")]
async fn commutative_without_resolver(w: &mut HandlingWorld) {
    let order = angzarr_router::domain::DomainDispatch::new(
        "order",
        "order",
        angzarr_router::rebuild::Rebuilder::new(|| ()),
    )
    .on_command("orders.v1.AddItem", |_cmd, _state, _cctx| Ok(None));
    let req = conf::order_command(
        "AddItem",
        "apple",
        &[],
        Some((0, pb::ConflictResolution::Commutative, Vec::new())),
        None,
    );
    condition(w, order.dispatch(&req).map(|_| ()));
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
    let tier = HandlingWorld::cucumber()
        .fail_on_skipped()
        .run(common::tier("handling.feature"))
        .await;
    common::exit(tier.execution_has_failed());
}
