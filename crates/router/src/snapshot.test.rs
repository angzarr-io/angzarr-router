//! The post-command snapshot (owner ruling, round 7): the router fills
//! `events.snapshot.state` with the state after the emitted events on every
//! domain's answer (an aggregate's or a process manager's, a PM being a domain;
//! decision 14.31). Business code never decides when; the
//! coordinator stamps the snapshot's sequence and rules version.

use std::collections::BTreeMap;

use prost::Message;
use prost_types::Any;

use crate::domain::{DomainDispatch, FactRecord};
use crate::error::codes;
use crate::pb;
use crate::rebuild::Rebuilder;
use crate::test_support::*;
use crate::type_url;

/// One counter per item; each Cover event bumps the item its domain names.
#[derive(Clone, PartialEq, prost::Message)]
struct Items {
    #[prost(btree_map = "string, uint32", tag = "1")]
    items: BTreeMap<String, u32>,
}

impl prost::Name for Items {
    const NAME: &'static str = "Items";
    const PACKAGE: &'static str = "test.items";
}

fn items_rebuilder() -> Rebuilder<Items> {
    Rebuilder::of_message().apply(&cover_full_name(), |s: &mut Items, any| {
        let key = pb::Cover::decode(any.value.as_slice())?.domain;
        *s.items.entry(key).or_default() += 1;
        Ok(())
    })
}

/// The snapshot image of an Items state.
fn image(pairs: &[(&str, u32)]) -> Any {
    Any {
        type_url: "/test.items.Items".to_string(),
        value: Items {
            items: pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        }
        .encode_to_vec(),
    }
}

/// The aggregate: `test.Bump(key)` emits one bump of `key`, `test.Nothing`
/// emits nothing, `test.Refuse` is rejected; the fact `test.Note(key)` is
/// recorded and flagged with a bump of "flag".
fn items(rebuilder: Rebuilder<Items>) -> DomainDispatch<Items> {
    DomainDispatch::new("items", "items", rebuilder)
        .with_rules_version("7")
        .on_command("test.Bump", |cmd, _state, _cctx| {
            let key = pb::Cover::decode(cmd.value.as_slice()).unwrap().domain;
            Ok(Some(pb::EventBook {
                pages: vec![event_page(cover_any(&key))],
                ..Default::default()
            }))
        })
        .on_command("test.Nothing", |_cmd, _state, _cctx| Ok(None))
        .on_command("test.Refuse", |_cmd, _state, _cctx| {
            Err(crate::error::HandlerError::Coded(
                crate::error::CodedError::rejection("NO", "no", []),
            ))
        })
        .on_fact("test.Note", |fact, _state| {
            Ok(FactRecord {
                fact: cover_any(&pb::Cover::decode(fact.value.as_slice()).unwrap().domain),
                flags: vec![cover_any("flag")],
            })
        })
        .with_commutative(|before: &Items, after: &Items| {
            let keys: std::collections::BTreeSet<&String> =
                before.items.keys().chain(after.items.keys()).collect();
            keys.into_iter()
                .filter(|k| before.items.get(*k) != after.items.get(*k))
                .map(|k| format!("items[{k}]"))
                .collect()
        })
}

fn typed(fq: &str, key: &str) -> Any {
    let mut any = cover_any(key);
    any.type_url = type_url(fq);
    any
}

/// `command` over a history bumping `history` at sequences 0...
fn over(mut req: pb::ContextualCommand, history: &[&str]) -> pb::ContextualCommand {
    req.events = Some(pb::EventBook {
        pages: history
            .iter()
            .zip(0..)
            .map(|(k, seq)| sequenced_event_page(seq, cover_any(k)))
            .collect(),
        next_sequence: history.len() as u64,
        ..Default::default()
    });
    req
}

fn snapshot_of(resp: &pb::CommandHandlerResponse) -> pb::Snapshot {
    resp.events
        .as_ref()
        .and_then(|b| b.snapshot.clone())
        .expect("every answer carries the post-command snapshot")
}

#[test]
fn a_command_answers_the_state_after_its_events() {
    let resp = items(items_rebuilder())
        .dispatch(&over(command_for(typed("test.Bump", "a")), &["a", "b"]))
        .expect("accepted");
    let snapshot = snapshot_of(&resp);
    assert_eq!(snapshot.state, Some(image(&[("a", 2), ("b", 1)])));
    assert_eq!(snapshot.sequence, 0, "the coordinator stamps the sequence");
    assert_eq!(snapshot.rules_version, "", "and the rules version");
    assert_eq!(resp.events.unwrap().pages.len(), 1);
}

#[test]
fn a_command_emitting_nothing_still_answers_the_current_state() {
    let resp = items(items_rebuilder())
        .dispatch(&over(command_for(typed("test.Nothing", "")), &["a"]))
        .expect("accepted");
    assert_eq!(snapshot_of(&resp).state, Some(image(&[("a", 1)])));
    assert!(resp.events.unwrap().pages.is_empty());
}

#[test]
fn a_rejection_answers_a_status_and_no_body() {
    // command_handler.proto: a rejection is a gRPC error status, never a
    // response message, so it carries no snapshot.
    let err = items(items_rebuilder())
        .dispatch(&over(command_for(typed("test.Refuse", "")), &["a"]))
        .expect_err("rejected");
    assert_eq!(err.code, "NO");
}

#[test]
fn a_fact_answers_the_state_after_its_record() {
    let resp = items(items_rebuilder())
        .dispatch(&over(
            contextual(fact_page(typed("test.Note", "n"))),
            &["a"],
        ))
        .expect("recorded");
    assert_eq!(
        snapshot_of(&resp).state,
        Some(image(&[("a", 1), ("flag", 1), ("n", 1)]))
    );
}

fn handled(
    key: &str,
    history: &[&str],
    expected: u64,
    resolution: pb::ConflictResolution,
) -> pb::ContextualCommand {
    let mut req = over(command_for(typed("test.Bump", key)), history);
    req.command.as_mut().unwrap().handling = Some(pb::Handling {
        expected_sequence: Some(expected),
        conflict_resolution: resolution as i32,
    });
    req.conflict_nonce = b"n".to_vec().into();
    req
}

#[test]
fn a_commutative_command_answers_the_head_plus_its_events() {
    let resp = items(items_rebuilder())
        .dispatch(&handled(
            "a",
            &["b", "b"],
            0,
            pb::ConflictResolution::Commutative,
        ))
        .expect("disjoint");
    assert_eq!(snapshot_of(&resp).state, Some(image(&[("a", 1), ("b", 2)])));
}

#[test]
fn an_aggregate_handles_command_answers_the_head_plus_its_events() {
    let resolving = DomainDispatch::new("items", "items", items_rebuilder()).on_command(
        "test.Bump",
        |cmd, _state, cctx| {
            if let Some(conflict) = cctx.conflict.as_ref() {
                conflict.resolve_conflicts(conflict.window(), conflict.nonce());
            }
            let key = pb::Cover::decode(cmd.value.as_slice()).unwrap().domain;
            Ok(Some(pb::EventBook {
                pages: vec![event_page(cover_any(&key))],
                ..Default::default()
            }))
        },
    );
    let resp = resolving
        .dispatch(&handled(
            "a",
            &["b"],
            0,
            pb::ConflictResolution::AggregateHandles,
        ))
        .expect("resolved");
    assert_eq!(snapshot_of(&resp).state, Some(image(&[("a", 1), ("b", 1)])));
}

#[test]
fn a_rebuilder_with_no_state_message_answers_no_snapshot() {
    let resp = DomainDispatch::new("t", "t", cover_applier(fresh_rebuilder()))
        .on_command("test.Nothing", |_, _, _| Ok(None))
        .dispatch(&command_for(typed("test.Nothing", "")))
        .expect("accepted");
    assert!(resp.events.unwrap().snapshot.is_none());
}

#[test]
fn a_rebuilder_states_its_image_through_with_state_image() {
    let r: Rebuilder<Any> = Rebuilder::new(Any::default).with_state_image(|s: &Any| s.clone());
    assert_eq!(r.state_image(&image(&[])), Some(image(&[])));
    assert_eq!(fresh_rebuilder().state_image(&TestState::default()), None);
}

#[test]
fn an_emitted_event_that_does_not_fold_fails_the_command() {
    let corrupt = DomainDispatch::new("items", "items", items_rebuilder()).on_command(
        "test.Bump",
        |_cmd, _state, _cctx| {
            Ok(Some(pb::EventBook {
                pages: vec![event_page(corrupt_cover_any())],
                ..Default::default()
            }))
        },
    );
    let err = corrupt
        .dispatch(&command_for(typed("test.Bump", "a")))
        .expect_err("the emitted event does not fold");
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
    assert_eq!(err.message, crate::error::messages::FOLD_FAILED_EMITTED);
}

// The snapshot must equal what a rebuild would fold once the events are
// stamped, so the fold sees each emitted page at the sequence it will get
// (the head's next sequence onward); the answered pages stay unstamped.
#[test]
fn emitted_events_fold_at_the_sequences_they_will_be_given() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let record = seen.clone();
    let rebuilder = Rebuilder::<Items>::of_message().apply_with_context(
        &cover_full_name(),
        move |_s: &mut Items, _any, ctx| {
            record.lock().unwrap().push(ctx.sequence);
            Ok(())
        },
    );
    let resp = DomainDispatch::new("items", "items", rebuilder)
        .on_command("test.Bump", |_cmd, _state, _cctx| {
            Ok(Some(pb::EventBook {
                pages: vec![event_page(cover_any("x")), event_page(cover_any("y"))],
                ..Default::default()
            }))
        })
        .dispatch(&over(command_for(typed("test.Bump", "a")), &["a", "b"]))
        .expect("accepted");
    assert_eq!(*seen.lock().unwrap(), vec![0, 1, 2, 3]);
    let pages = resp.events.unwrap().pages;
    assert!(pages.iter().all(|p| p.sequence == 0), "answered unstamped");
}
