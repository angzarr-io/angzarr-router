//! The handler-facing handling surface: conflict delivery, the COMMUTATIVE
//! resolver, the overlap rule and command upcasting (docs/design-handling.md).
//! Also the rules version every response reports.
//!
//! Spec governing each group (angzarr-project spec/20261019-draft):
//! - rules version: types.proto EventMeta.rules_version,
//!   command_handler.proto CommandHandlerResponse.rules_version,
//!   process_manager.proto ProcessManagerHandleResponse.rules_version;
//!   decision 12.6; backlog RV-2.
//! - conflict delivery: types.proto Handling, ConflictResolution,
//!   ContextualCommand.conflict_nonce; decision 12.7; backlog HD-5, NC-1.
//! - COMMUTATIVE resolver and overlap: options.proto conflict_units region
//!   ("Overlap", "The resolver"); backlog CR-1.
//! - command upcasting: upcaster.proto, command_handler.proto Handle step 2;
//!   decision 12.11; backlog VC-1.
//!
//! The behaviour-level scenarios are angzarr-project's handler-contract
//! tier (features/handler-contract/handling.feature, C-0533..C-0550), bound
//! by crates/conformance/tests/handling.rs. Snapshot selection is the
//! coordinator's (decision 14.1): the router folds a delivered snapshot
//! unchecked.

use std::sync::{Arc, Mutex};

use prost_types::Any;

use crate::aggregate::{AggregateDispatch, CommandContext};
use crate::conflict::overlap;
use crate::error::{codes, GrpcCode};
use crate::pb;
use crate::process_manager::ProcessManagerDispatch;
use crate::rebuild::Rebuilder;
use crate::test_support::*;
use crate::type_url;
use crate::upcaster::UpcasterDispatch;

/// A COMMUTATIVE-capable state: one counter per item key. `changed` below is
/// what the generator (backlog CR-1) would emit for
/// `map<string, uint32> items = 1 [(conflict) = {}]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ListState {
    pub(crate) items: std::collections::BTreeMap<String, u32>,
}

pub(crate) fn changed(before: &ListState, after: &ListState) -> Vec<String> {
    let keys: std::collections::BTreeSet<&String> =
        before.items.keys().chain(after.items.keys()).collect();
    keys.into_iter()
        .filter(|k| before.items.get(*k) != after.items.get(*k))
        .map(|k| format!("items[{k}]"))
        .collect()
}

/// An event bumping item `key` (a Cover whose domain is the key).
pub(crate) fn bump(key: &str) -> Any {
    cover_any(key)
}

pub(crate) fn list_rebuilder() -> Rebuilder<ListState> {
    Rebuilder::new(ListState::default)
        .apply(&cover_full_name(), |s: &mut ListState, any| {
            let key = <pb::Cover as prost::Message>::decode(any.value.as_slice())?.domain;
            *s.items.entry(key).or_default() += 1;
            Ok(())
        })
        .with_snapshot(|s: &mut ListState, _any| {
            s.items.insert("snap".to_string(), 1);
            Ok(())
        })
}

/// A list aggregate whose `test.Bump` command bumps the item named in the
/// command's Cover payload, recording each CommandContext it saw.
pub(crate) fn list_aggregate(
    seen: Arc<Mutex<Vec<CommandContext>>>,
) -> AggregateDispatch<ListState> {
    AggregateDispatch::new("list", "list", list_rebuilder()).on_command(
        "test.Bump",
        move |cmd, _state, cctx| {
            let key = <pb::Cover as prost::Message>::decode(cmd.value.as_slice())
                .unwrap()
                .domain;
            seen.lock().unwrap().push(cctx);
            Ok(Some(pb::EventBook {
                pages: vec![event_page(bump(&key))],
                ..Default::default()
            }))
        },
    )
}

pub(crate) fn bump_command(key: &str) -> Any {
    let mut any = cover_any(key);
    any.type_url = type_url("test.Bump");
    any
}

/// A delivery of `test.Bump(key)` over `history` (bumps of the given keys at
/// sequences 0..), resolved to `resolution` with expected sequence
/// `expected`, carrying `nonce`.
pub(crate) fn delivery(
    key: &str,
    history: &[&str],
    expected: u64,
    resolution: pb::ConflictResolution,
    nonce: &[u8],
) -> pb::ContextualCommand {
    let mut req = command_for(bump_command(key));
    req.events = Some(pb::EventBook {
        pages: history
            .iter()
            .zip(0..)
            .map(|(k, seq)| sequenced_event_page(seq, bump(k)))
            .collect(),
        next_sequence: history.len() as u64,
        ..Default::default()
    });
    req.command.as_mut().unwrap().handling = Some(pb::Handling {
        expected_sequence: Some(expected),
        conflict_resolution: resolution as i32,
    });
    req.conflict_nonce = nonce.to_vec().into();
    req
}

pub(crate) fn strict(key: &str, history: &[&str]) -> pb::ContextualCommand {
    delivery(
        key,
        history,
        history.len() as u64,
        pb::ConflictResolution::Strict,
        &[],
    )
}

// --- conflict delivery (HD-5, NC-1) -----------------------------------------

#[test]
fn strict_hands_the_handler_its_handling_and_no_conflict() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    list_aggregate(seen.clone())
        .dispatch(&strict("a", &["x", "y"]))
        .expect("dispatch");
    let cctx = seen.lock().unwrap()[0].clone();
    assert_eq!(cctx.expected_sequence, 2);
    assert_eq!(cctx.conflict_resolution, pb::ConflictResolution::Strict);
    assert!(cctx.conflict.is_none(), "STRICT has no window or nonce");
}

/// An AGGREGATE_HANDLES aggregate whose handler records the window it saw and
/// acknowledges with `ack(nonce)` (None: never calls resolve_conflicts).
fn handles(
    ack: impl Fn(&[u8]) -> Option<Vec<u8>> + Send + Sync + 'static,
    windows: Arc<Mutex<Vec<Vec<u64>>>>,
) -> AggregateDispatch<ListState> {
    AggregateDispatch::new("list", "list", list_rebuilder()).on_command(
        "test.Bump",
        move |_cmd, _state, cctx| {
            let conflict = cctx
                .conflict
                .as_ref()
                .expect("AGGREGATE_HANDLES carries a conflict");
            windows
                .lock()
                .unwrap()
                .push(conflict.window().iter().map(|p| p.sequence).collect());
            if let Some(value) = ack(conflict.nonce()) {
                conflict.resolve_conflicts(conflict.window(), &value);
            }
            Ok(Some(pb::EventBook {
                pages: vec![event_page(bump("a"))],
                ..Default::default()
            }))
        },
    )
}

#[test]
fn aggregate_handles_delivers_the_window_and_keeps_resolved_output() {
    let windows = Arc::new(Mutex::new(Vec::new()));
    let d = handles(|nonce| Some(nonce.to_vec()), windows.clone());
    let resp = d
        .dispatch(&delivery(
            "a",
            &["x", "y", "z"],
            1,
            pb::ConflictResolution::AggregateHandles,
            b"n-1",
        ))
        .expect("resolved");
    assert_eq!(*windows.lock().unwrap(), vec![vec![1, 2]], "pages E..head");
    assert_eq!(resp.events.expect("events").pages.len(), 1);
}

#[test]
fn aggregate_handles_without_resolve_conflicts_fails_stale() {
    let d = handles(|_| None, Arc::default());
    let err = d
        .dispatch(&delivery(
            "a",
            &["x"],
            0,
            pb::ConflictResolution::AggregateHandles,
            b"n-1",
        ))
        .expect_err("the handler ignored the window");
    assert_eq!(err.code, codes::CONFLICT_UNRESOLVED);
    assert_eq!(err.grpc, GrpcCode::Aborted);
}

#[test]
fn aggregate_handles_with_another_nonce_fails_stale() {
    let d = handles(|_| Some(b"made-up".to_vec()), Arc::default());
    let err = d
        .dispatch(&delivery(
            "a",
            &["x"],
            0,
            pb::ConflictResolution::AggregateHandles,
            b"n-1",
        ))
        .expect_err("a made-up nonce");
    assert_eq!(err.code, codes::CONFLICT_UNRESOLVED);
}

#[test]
fn aggregate_handles_with_an_empty_window_still_demands_the_nonce() {
    let windows = Arc::new(Mutex::new(Vec::new()));
    let d = handles(|_| None, windows.clone());
    let err = d
        .dispatch(&delivery(
            "a",
            &["x"],
            1,
            pb::ConflictResolution::AggregateHandles,
            b"n-1",
        ))
        .expect_err("uniform handler path");
    assert_eq!(err.code, codes::CONFLICT_UNRESOLVED);
    assert_eq!(*windows.lock().unwrap(), vec![Vec::<u64>::new()]);
}

// --- COMMUTATIVE: the library resolver (CR-1) -------------------------------

fn commutative(seen: Arc<Mutex<Vec<CommandContext>>>) -> AggregateDispatch<ListState> {
    list_aggregate(seen).with_commutative(changed)
}

#[test]
fn commutative_keeps_a_stale_command_disjoint_from_the_window() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let resp = commutative(seen.clone())
        .dispatch(&delivery(
            "a",
            &["b", "b"],
            0,
            pb::ConflictResolution::Commutative,
            b"n",
        ))
        .expect("items[a] vs items[b]: disjoint");
    assert_eq!(resp.events.expect("events").pages.len(), 1);
    let cctx = seen.lock().unwrap()[0].clone();
    assert!(
        cctx.conflict.is_none(),
        "the library resolves; the handler never sees the nonce"
    );
    assert_eq!(
        cctx.conflict_resolution,
        pb::ConflictResolution::Commutative
    );
}

#[test]
fn commutative_fails_stale_on_overlap() {
    let err = commutative(Arc::default())
        .dispatch(&delivery(
            "b",
            &["a", "b"],
            1,
            pb::ConflictResolution::Commutative,
            b"n",
        ))
        .expect_err("items[b] in both window and mine");
    assert_eq!(err.code, codes::CONFLICT_OVERLAP);
    assert_eq!(err.grpc, GrpcCode::Aborted);
}

// Owner ruling 2026-10-19: a fold of the command's own new events (not yet
// persisted) that fails is still a handler bug, said differently.
#[test]
fn commutative_folding_an_emitted_event_that_fails_says_the_command_emitted_it() {
    let emits_corrupt = AggregateDispatch::new("list", "list", list_rebuilder())
        .on_command("test.Bump", |_cmd, _state, _cctx| {
            Ok(Some(pb::EventBook {
                pages: vec![sequenced_event_page(9, corrupt_cover_any())],
                ..Default::default()
            }))
        })
        .with_commutative(changed);
    let err = emits_corrupt
        .dispatch(&delivery(
            "a",
            &["b"],
            0,
            pb::ConflictResolution::Commutative,
            b"n",
        ))
        .expect_err("the emitted event does not fold");
    let type_url = type_url(&cover_full_name());
    assert_fold_failed(
        &err,
        crate::error::messages::FOLD_FAILED_EMITTED,
        &[
            (crate::error::extras::SEQUENCE, "1"),
            (crate::error::extras::TYPE_URL, type_url.as_str()),
        ],
        "",
    );
}

#[test]
fn commutative_folding_a_window_event_that_fails_says_it_was_accepted() {
    let mut req = delivery(
        "a",
        &["b", "b"],
        0,
        pb::ConflictResolution::Commutative,
        b"n",
    );
    req.events.as_mut().unwrap().pages[1].payload = Some(corrupt_cover_any());
    let err = commutative(Arc::default())
        .dispatch(&req)
        .expect_err("the window does not fold");
    assert_eq!(err.message, crate::error::messages::FOLD_FAILED);
    assert_eq!(err.extras[crate::error::extras::SEQUENCE], "1");
}

#[test]
fn commutative_only_counts_events_from_the_expected_sequence() {
    // items[b] changed at sequence 0, which the command saw (expected 1).
    commutative(Arc::default())
        .dispatch(&delivery(
            "b",
            &["b", "a"],
            1,
            pb::ConflictResolution::Commutative,
            b"n",
        ))
        .expect("window is items[a] only");
}

#[test]
fn commutative_decides_on_the_head() {
    let states = Arc::new(Mutex::new(Vec::new()));
    let sink = states.clone();
    let d = AggregateDispatch::new("list", "list", list_rebuilder())
        .with_commutative(changed)
        .on_command("test.Bump", move |_c, state: &mut ListState, _cctx| {
            sink.lock().unwrap().push(state.clone());
            Ok(None)
        });
    d.dispatch(&delivery(
        "a",
        &["b", "c"],
        0,
        pb::ConflictResolution::Commutative,
        b"n",
    ))
    .expect("an empty mine is disjoint");
    assert_eq!(
        states.lock().unwrap()[0].items.len(),
        2,
        "decide saw the window's effects"
    );
}

// Decide's own mutations of the state are discarded: the command's units
// are changed(head, head + its events), never what decide did to the state.
#[test]
fn commutative_restores_the_head_before_folding_the_commands_events() {
    let d = AggregateDispatch::new("list", "list", list_rebuilder())
        .with_commutative(changed)
        .on_command("test.Bump", |_c, state: &mut ListState, _cctx| {
            *state.items.entry("b".to_string()).or_default() += 10;
            Ok(Some(pb::EventBook {
                pages: vec![event_page(bump("a"))],
                ..Default::default()
            }))
        });
    d.dispatch(&delivery(
        "a",
        &["b"],
        0,
        pb::ConflictResolution::Commutative,
        b"n",
    ))
    .expect("mine is items[a] only: disjoint from the window's items[b]");
}

#[test]
fn commutative_without_a_changed_function_is_unsupported() {
    let err = list_aggregate(Arc::default())
        .dispatch(&delivery(
            "a",
            &["b"],
            0,
            pb::ConflictResolution::Commutative,
            b"n",
        ))
        .expect_err("no conflict units registered");
    assert_eq!(err.code, codes::CONFLICT_RESOLUTION_UNSUPPORTED);
    assert_eq!(err.grpc, GrpcCode::InvalidArgument);
}

// --- the overlap rule (options.proto conflict_units, "Overlap") -------------

fn units(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

#[test]
fn overlap_follows_the_conflict_units_rules() {
    assert!(
        !overlap(&units(&[]), &units(&["items[a]"])),
        "an empty set is disjoint"
    );
    assert!(
        overlap(&units(&["*notes"]), &units(&["title"])),
        "a wildcard vs non-empty"
    );
    assert!(
        !overlap(&units(&["*notes"]), &units(&[])),
        "a wildcard vs empty"
    );
    assert!(
        overlap(&units(&["items[42]"]), &units(&["items[42]"])),
        "equal units"
    );
    assert!(
        overlap(&units(&["items[42]"]), &units(&["items[42].weight"])),
        "a prefix at a `.` boundary"
    );
    assert!(
        overlap(&units(&["settings"]), &units(&["settings.cooldown_days"])),
        "a prefix at a `.` boundary"
    );
    assert!(
        overlap(&units(&["columns"]), &units(&["columns[due]"])),
        "a prefix at `[`"
    );
    assert!(
        !overlap(&units(&["items[4]"]), &units(&["items[42]"])),
        "no boundary"
    );
    assert!(
        !overlap(&units(&["title"]), &units(&["titles"])),
        "no boundary"
    );
}

// --- command upcasting (VC-1) -----------------------------------------------

/// A list aggregate whose chain upcasts BumpV1 → Bump and BumpOld →
/// BumpLost (a type nothing handles), handling Bump as a command and
/// FactV2 as a fact, and recording the type URL each handler received.
fn upcasting_aggregate(handled: Arc<Mutex<Vec<String>>>) -> AggregateDispatch<ListState> {
    let rename = |to: &'static str| {
        move |any: &Any| -> Result<Any, crate::error::HandlerError> {
            Ok(Any {
                type_url: type_url(to),
                value: any.value.clone(),
            })
        }
    };
    let upcaster = UpcasterDispatch::new("cmd-upcaster", "list")
        .on_event("test.BumpV1", rename("test.Bump"))
        .on_event("test.BumpOld", rename("test.BumpLost"))
        .on_event("test.FactV1", rename("test.FactV2"));
    let facts = handled.clone();
    AggregateDispatch::new("list", "list", list_rebuilder())
        .with_command_upcaster(upcaster)
        .on_command("test.Bump", move |cmd, _s, _c| {
            handled.lock().unwrap().push(cmd.type_url.clone());
            Ok(None)
        })
        .on_fact("test.FactV2", move |fact, _s| {
            facts.lock().unwrap().push(fact.type_url.clone());
            Ok(crate::aggregate::FactRecord::from(fact.clone()))
        })
}

fn typed(name: &str) -> Any {
    Any {
        type_url: type_url(name),
        value: Vec::new(),
    }
}

#[test]
fn an_old_command_type_is_upcast_then_handled_as_the_new_type() {
    let handled = Arc::new(Mutex::new(Vec::new()));
    upcasting_aggregate(handled.clone())
        .dispatch(&command_for(typed("test.BumpV1")))
        .expect("upcast to test.Bump");
    assert_eq!(*handled.lock().unwrap(), vec![type_url("test.Bump")]);
}

// Owner ruling: upcasting runs on both arms.
#[test]
fn an_old_fact_type_is_upcast_then_recorded_as_the_new_type() {
    let handled = Arc::new(Mutex::new(Vec::new()));
    let resp = upcasting_aggregate(handled.clone())
        .dispatch(&contextual(fact_page(typed("test.FactV1"))))
        .expect("upcast to test.FactV2");
    assert_eq!(*handled.lock().unwrap(), vec![type_url("test.FactV2")]);
    assert_eq!(
        resp.events.unwrap().pages[0]
            .payload
            .as_ref()
            .unwrap()
            .type_url,
        type_url("test.FactV2"),
        "the upcast fact is what is recorded"
    );
}

// Owner ruling: the refusal names both types: the message names the type
// URL the client sent, the extras carry the last one the chain produced.
#[test]
fn an_upcast_type_with_no_handler_is_refused_naming_both_types() {
    let err = upcasting_aggregate(Arc::default())
        .dispatch(&command_for(typed("test.BumpOld")))
        .expect_err("BumpLost reaches no handler");
    assert_eq!(err.code, codes::NO_HANDLER_REGISTERED);
    assert_eq!(err.grpc, GrpcCode::InvalidArgument);
    assert_eq!(
        err.message,
        format!("no handler for type URL {}", type_url("test.BumpOld"))
    );
    assert_eq!(
        err.extras.get("type_url").map(String::as_str),
        Some(type_url("test.BumpOld").as_str())
    );
    assert_eq!(
        err.extras.get("upcast_type_url").map(String::as_str),
        Some(type_url("test.BumpLost").as_str())
    );
}

#[test]
fn a_type_with_no_handler_and_no_upcaster_path_is_refused_naming_the_sent_type() {
    let err = upcasting_aggregate(Arc::default())
        .dispatch(&command_for(typed("test.BumpV0")))
        .expect_err("no path to a handler");
    assert_eq!(err.grpc, GrpcCode::InvalidArgument);
    assert_eq!(
        err.message,
        format!("no handler for type URL {}", type_url("test.BumpV0"))
    );
    assert_eq!(
        err.extras.get("upcast_type_url").map(String::as_str),
        Some(type_url("test.BumpV0").as_str()),
        "the chain left it unchanged"
    );
}

// --- the rules version every response reports -------------------------------
//
// command_handler.proto CommandHandlerResponse.rules_version and
// process_manager.proto ProcessManagerHandleResponse.rules_version: "the
// client library fills it from the domain's declaration on every response"
// (decision 12.6; backlog RV-2). Owner ruling: the binary's full baked
// version string; snapshot validity is the coordinator's, not the router's.

const RULES: &str = "1.4.2+build.7";

#[test]
fn every_aggregate_response_reports_the_rules_version() {
    let d = list_aggregate(Arc::default())
        .with_rules_version(RULES)
        .on_fact(&cover_full_name(), |fact, _s| {
            Ok(crate::aggregate::FactRecord::from(fact.clone()))
        });
    let command = d.dispatch(&strict("a", &[])).expect("command");
    assert_eq!(command.rules_version, RULES);
    let fact = d
        .dispatch(&contextual(fact_page(cover_any("f"))))
        .expect("fact");
    assert_eq!(fact.rules_version, RULES);
}

#[test]
fn an_undeclared_rules_version_reports_empty() {
    let resp = list_aggregate(Arc::default())
        .dispatch(&strict("a", &[]))
        .expect("command");
    assert_eq!(resp.rules_version, "");
}

fn shipped(domain: &str) -> pb::ProcessManagerHandleRequest {
    pb::ProcessManagerHandleRequest {
        trigger: Some(pb::EventBook {
            cover: Some(pb::Cover {
                domain: domain.to_string(),
                ..Default::default()
            }),
            pages: vec![event_page(Any {
                type_url: type_url("test.Shipped"),
                value: Vec::new(),
            })],
            ..Default::default()
        }),
        process_state: None,
    }
}

#[test]
fn every_process_manager_response_reports_the_rules_version() {
    let pm = ProcessManagerDispatch::new("pm", "pm", ["inventory"], fresh_rebuilder())
        .with_rules_version(RULES)
        .on_event("orders", "test.Shipped", |_e, _s, _c| {
            Ok(pb::ProcessManagerHandleResponse::default())
        });
    assert_eq!(
        pm.dispatch(&shipped("orders"))
            .expect("handled")
            .rules_version,
        RULES
    );
    assert_eq!(
        pm.dispatch(&shipped("billing"))
            .expect("outside the sources")
            .rules_version,
        RULES,
        "even a trigger the PM does not consume"
    );
}

#[test]
// Owner ruling: the router always sets the declared version, overwriting a
// handler's.
fn a_process_manager_handlers_rules_version_is_overwritten() {
    let pm = ProcessManagerDispatch::new("pm", "pm", ["inventory"], fresh_rebuilder()).on_event(
        "orders",
        "test.Shipped",
        |_e, _s, _c| {
            Ok(pb::ProcessManagerHandleResponse {
                rules_version: "from-the-handler".to_string(),
                ..Default::default()
            })
        },
    );
    assert_eq!(
        pm.dispatch(&shipped("orders"))
            .expect("handled")
            .rules_version,
        "",
        "none declared: the declared (empty) version still wins"
    );
    let pm = pm.with_rules_version(RULES);
    assert_eq!(pm.rules_version(), RULES);
    assert_eq!(
        pm.dispatch(&shipped("orders"))
            .expect("handled")
            .rules_version,
        RULES
    );
}

// A router answers its PM's version even for a trigger the PM does not
// consume.
#[test]
fn a_router_of_a_process_manager_reports_the_version_for_an_unconsumed_trigger() {
    let router = crate::router::RouterBuilder::new()
        .process_manager(
            ProcessManagerDispatch::new("pm", "pm", ["inventory"], fresh_rebuilder())
                .with_rules_version(RULES)
                .on_event("orders", "test.Shipped", |_e, _s, _c| {
                    Ok(pb::ProcessManagerHandleResponse::default())
                }),
        )
        .build()
        .expect("build");
    assert_eq!(
        router
            .dispatch_process_manager(&shipped("billing"))
            .expect("no PM consumes billing")
            .rules_version,
        RULES
    );
}
