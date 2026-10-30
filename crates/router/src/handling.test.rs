//! The handler-facing handling surface: conflict delivery, the COMMUTATIVE
//! resolver and the overlap rule (docs/design-handling.md).
//! Also the rules version every response reports.
//!
//! Spec governing each group (angzarr-project spec/20261019-draft):
//! - rules version: types.proto EventMeta.rules_version,
//!   command_handler.proto CommandHandlerResponse.rules_version (a process
//!   manager's too: it is a domain, decision 14.31);
//!   decision 12.6; backlog RV-2.
//! - conflict delivery: types.proto Handling, ConflictResolution,
//!   ContextualCommand.conflict_nonce; decision 12.7; backlog HD-5, NC-1.
//! - COMMUTATIVE resolver and overlap: options.proto conflict_units region
//!   ("Overlap", "The resolver"); backlog CR-1.
//!
//! The behaviour-level scenarios are angzarr-project's handler-contract
//! tier (features/handler-contract/handling.feature, C-0533..C-0550), bound
//! by crates/conformance/tests/handling.rs. Snapshot selection is the
//! coordinator's (decision 14.1): the router folds a delivered snapshot
//! unchecked.

use std::sync::{Arc, Mutex};

use prost_types::Any;

use crate::conflict::overlap;
use crate::domain::{CommandContext, DomainDispatch};
use crate::error::{codes, GrpcCode};
use crate::pb;
use crate::rebuild::Rebuilder;
use crate::test_support::*;
use crate::type_url;

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
pub(crate) fn list_aggregate(seen: Arc<Mutex<Vec<CommandContext>>>) -> DomainDispatch<ListState> {
    DomainDispatch::new("list", "list", list_rebuilder()).on_command(
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

// --- the context a runtime passes on with a refusal ----------------------------

/// `a` and `b` field by field (a `Conflict` has no equality): its window
/// and nonce.
fn same_context(a: &CommandContext, b: &CommandContext) {
    assert_eq!(
        (a.next_sequence, a.had_prior_events, a.expected_sequence),
        (b.next_sequence, b.had_prior_events, b.expected_sequence)
    );
    assert_eq!(a.cover, b.cover);
    assert_eq!(a.conflict_resolution, b.conflict_resolution);
    assert_eq!(
        a.conflict
            .as_ref()
            .map(|c| (c.window().to_vec(), c.nonce().to_vec())),
        b.conflict
            .as_ref()
            .map(|c| (c.window().to_vec(), c.nonce().to_vec()))
    );
    assert_eq!(
        (&a.command_id, &a.correlation_id, &a.fact_origin),
        (&b.command_id, &b.correlation_id, &b.fact_origin)
    );
}

// command_context answers, from the ContextualCommand alone, the context its
// handler is handed: under STRICT with history, under AGGREGATE_HANDLES
// behind the head (a window and the nonce), and on a new stream.
#[test]
fn command_context_is_the_context_the_handler_is_handed() {
    let deliveries = [
        identified(strict("a", &["x", "y"])),
        identified(delivery(
            "a",
            &["x", "y", "z"],
            1,
            pb::ConflictResolution::AggregateHandles,
            &[5, 5],
        )),
        identified(strict("a", &[])),
    ];
    for req in deliveries {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let d = DomainDispatch::new("list", "list", list_rebuilder()).on_command(
            "test.Bump",
            move |_cmd, _state, cctx| {
                if let Some(conflict) = &cctx.conflict {
                    conflict.resolve_conflicts(conflict.window(), conflict.nonce());
                }
                sink.lock().unwrap().push(cctx);
                Ok(None)
            },
        );
        d.dispatch(&req).expect("dispatch");
        same_context(
            &crate::domain::command_context(&req),
            &seen.lock().unwrap()[0],
        );
    }
}

// --- the input's identity (types.proto, CommandPage) ---------------------------

/// `req` with its page identified: command id 0x11, correlation 0x22.
fn identified(mut req: pb::ContextualCommand) -> pb::ContextualCommand {
    let page = req.command.as_mut().unwrap().page.as_mut().unwrap();
    page.command_id = Some(uuid_of(0x11));
    page.correlation_id = Some(uuid_of(0x22));
    req
}

// A command handler reads the input's identity: its command id and its
// correlation (a command has no fact origin).
#[test]
fn a_command_handler_reads_the_commands_identity() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    list_aggregate(seen.clone())
        .dispatch(&identified(strict("a", &["x"])))
        .expect("dispatch");
    let cctx = seen.lock().unwrap()[0].clone();
    assert_eq!(cctx.command_id, Some(uuid_of(0x11)));
    assert_eq!(cctx.correlation_id, Some(uuid_of(0x22)));
    assert_eq!(cctx.fact_origin, None, "a command has no fact origin");
}

// A fact handler gets the same context as a command handler, its origin
// included (an existing angzarr-embedded handler reads fact_origin).
#[test]
fn a_fact_handler_reads_the_facts_identity_and_origin() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let d = DomainDispatch::new("list", "list", list_rebuilder()).on_fact_with_context(
        "test.Paid",
        move |fact, _state, cctx| {
            sink.lock().unwrap().push(cctx);
            Ok(crate::domain::FactRecord::from(fact.clone()))
        },
    );
    let origin = pb::FactOrigin {
        external_id: "stripe:evt_1".to_string(),
        description: "Stripe webhook".to_string(),
    };
    let mut req = identified(contextual(fact_page(Any {
        type_url: type_url("test.Paid"),
        value: Vec::new(),
    })));
    req.command
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .fact_origin = Some(origin.clone());
    d.dispatch(&req).expect("recorded");
    let cctx = seen.lock().unwrap()[0].clone();
    assert_eq!(cctx.command_id, Some(uuid_of(0x11)));
    assert_eq!(cctx.correlation_id, Some(uuid_of(0x22)));
    assert_eq!(cctx.fact_origin, Some(origin));
}

// A fact handler, like a command handler, knows whether the aggregate
// exists: the prior-events book carried history.
#[test]
fn a_fact_handler_knows_whether_the_aggregate_exists() {
    for (history, exists) in [
        (vec![sequenced_event_page(0, bump("b"))], true),
        (vec![], false),
    ] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let d = DomainDispatch::new("list", "list", list_rebuilder()).on_fact_with_context(
            "test.Paid",
            move |fact, _state, cctx| {
                sink.lock().unwrap().push(cctx.had_prior_events);
                Ok(crate::domain::FactRecord::from(fact.clone()))
            },
        );
        let mut req = contextual(fact_page(Any {
            type_url: type_url("test.Paid"),
            value: Vec::new(),
        }));
        req.events = Some(pb::EventBook {
            next_sequence: history.len() as u64,
            pages: history,
            ..Default::default()
        });
        d.dispatch(&req).expect("recorded");
        assert_eq!(*seen.lock().unwrap(), vec![exists]);
    }
}

/// An AGGREGATE_HANDLES aggregate whose handler records the window it saw and
/// acknowledges with `ack(nonce)` (None: never calls resolve_conflicts).
fn handles(
    ack: impl Fn(&[u8]) -> Option<Vec<u8>> + Send + Sync + 'static,
    windows: Arc<Mutex<Vec<Vec<u64>>>>,
) -> DomainDispatch<ListState> {
    DomainDispatch::new("list", "list", list_rebuilder()).on_command(
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

// --- the conflict report (decision 14.42) -----------------------------------
//
// The router reports a stale command's conflicts and the coordinator judges
// them (CommandHandlerResponse.conflict, ConflictReport). The router's own
// judgement stays until its removal step, so only the reports of accepted
// commands are visible here.

#[test]
fn aggregate_handles_reports_the_nonce_business_code_settled_with() {
    let d = handles(|nonce| Some(nonce.to_vec()), Arc::default());
    let resp = d
        .dispatch(&delivery(
            "a",
            &["x", "y", "z"],
            1,
            pb::ConflictResolution::AggregateHandles,
            b"n-1",
        ))
        .expect("resolved");
    assert_eq!(
        resp.conflict,
        Some(pb::ConflictReport {
            resolved_nonce: b"n-1".to_vec(),
            window_units: Vec::new(),
            command_units: Vec::new(),
        }),
        "the nonce passed to resolve_conflicts, and no units"
    );
}

#[test]
fn commutative_reports_the_window_units_and_the_command_units() {
    let resp = commutative(Arc::default())
        .dispatch(&delivery(
            "a",
            &["x", "b", "c"],
            1,
            pb::ConflictResolution::Commutative,
            b"n",
        ))
        .expect("items[a] vs items[b], items[c]: disjoint");
    assert_eq!(
        resp.conflict,
        Some(pb::ConflictReport {
            resolved_nonce: Default::default(),
            window_units: units(&["items[b]", "items[c]"]),
            command_units: units(&["items[a]"]),
        }),
        "window = changed(expected, head), mine = changed(head, head + events)"
    );
}

#[test]
fn strict_reports_no_conflict() {
    let resp = list_aggregate(Arc::default())
        .dispatch(&strict("a", &["b"]))
        .expect("strict");
    assert_eq!(resp.conflict, None, "STRICT has no window to report");
}

// --- COMMUTATIVE: the library resolver (CR-1) -------------------------------

fn commutative(seen: Arc<Mutex<Vec<CommandContext>>>) -> DomainDispatch<ListState> {
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

// The library resolves the window but still tells the handler whether the
// aggregate exists: the prior-events book carried history.
#[test]
fn commutative_tells_the_handler_whether_the_aggregate_exists() {
    for (history, exists) in [(&["b"][..], true), (&[][..], false)] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        commutative(seen.clone())
            .dispatch(&delivery(
                "a",
                history,
                0,
                pb::ConflictResolution::Commutative,
                b"n",
            ))
            .expect("disjoint");
        assert_eq!(
            seen.lock().unwrap()[0].had_prior_events,
            exists,
            "history {history:?}"
        );
    }
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
    let emits_corrupt = DomainDispatch::new("list", "list", list_rebuilder())
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
        "FOLD_FAILED_EMITTED",
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
    // Decision 14.37: a failing fold has its own key.
    assert_eq!(err.code, "FOLD_FAILED");
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
    let d = DomainDispatch::new("list", "list", list_rebuilder())
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
    let d = DomainDispatch::new("list", "list", list_rebuilder())
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
    crate::test_support::assert_bug(&err);
    assert_eq!(err.grpc, GrpcCode::Internal);
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

// --- the rules version every response reports -------------------------------
//
// command_handler.proto CommandHandlerResponse.rules_version (every domain's,
// a process manager's included; decision 14.31): "the
// client library fills it from the domain's declaration on every response"
// (decision 12.6; backlog RV-2). Owner ruling: the binary's full baked
// version string; snapshot validity is the coordinator's, not the router's.

const RULES: &str = "1.4.2+build.7";

#[test]
fn every_aggregate_response_reports_the_rules_version() {
    let d = list_aggregate(Arc::default())
        .with_rules_version(RULES)
        .on_fact(&cover_full_name(), |fact, _s| {
            Ok(crate::domain::FactRecord::from(fact.clone()))
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
