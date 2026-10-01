//! Rebuilder contracts, transliterated from client-go's engine_test.go +
//! engine_boundaries_test.go (mutation-hardened bank, kill 0.972). The
//! boundary knowledge these encode — covered-page inclusivity,
//! HadPriorEvents shapes, gap pages never terminal — is the contract the
//! shared core must satisfy byte-for-byte.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use prost::Message;

use crate::error::codes;
use crate::pb;
use crate::test_support::*;

#[test]
fn applies_all_pages_in_order() {
    let r = cover_applier(fresh_rebuilder());

    let (state, info) = r
        .rebuild(Some(&book_of_covers(&["a", "b", "c"])))
        .expect("rebuild");
    assert_eq!(state.applied, vec!["a", "b", "c"]);
    assert!(
        info.had_prior_events,
        "had_prior_events = false with 3 pages (the Exists() bug)"
    );
    assert_eq!(info.applied_count, 3);
}

#[test]
fn absent_book_normalizes_to_fresh_state() {
    let r = fresh_rebuilder();
    let (state, info) = r.rebuild(None).expect("rebuild(None)");
    assert!(state.applied.is_empty(), "state must be fresh");
    assert!(
        !info.had_prior_events,
        "had_prior_events for an absent book"
    );
}

#[test]
fn corrupt_payload_fails_with_coded_error() {
    let r = cover_applier(fresh_rebuilder());
    let book = pb::EventBook {
        pages: vec![event_page(corrupt_cover_any())],
        ..Default::default()
    };

    let err = r
        .rebuild(Some(&book))
        .expect_err("corrupt payload must fail the rebuild, not yield factory-default state");
    assert_eq!(err.code, codes::PERSISTED_EVENT_CORRUPT);
}

#[test]
fn unknown_event_type_skipped() {
    let r = cover_applier(fresh_rebuilder());
    let book = pb::EventBook {
        pages: vec![
            // no applier registered — not all events fold into state
            event_page(any_of(&pb::Notification::default())),
            event_page(cover_any("a")),
        ],
        ..Default::default()
    };

    let (state, info) = r.rebuild(Some(&book)).expect("rebuild");
    assert_eq!(state.applied, vec!["a"], "want exactly the Cover");
    assert!(
        info.had_prior_events,
        "had_prior_events must reflect page presence, not applier hits"
    );
    assert_eq!(
        info.applied_count, 1,
        "applied_count counts applier hits only"
    );
}

#[test]
fn snapshot_applied_before_pages() {
    let r = cover_applier(fresh_rebuilder().with_snapshot(|s, payload| {
        let c = pb::Cover::decode(payload.value.as_slice())?;
        s.applied.push(format!("snap:{}", c.domain));
        Ok(())
    }));

    let mut book = book_of_covers(&["after"]);
    book.snapshot = Some(pb::Snapshot {
        state: Some(cover_any("base")),
        ..Default::default()
    });

    let (state, info) = r.rebuild(Some(&book)).expect("rebuild");
    assert_eq!(state.applied, vec!["snap:base", "after"]);
    assert!(info.had_prior_events, "a snapshot alone is prior history");
}

#[test]
fn pages_covered_by_snapshot_are_skipped() {
    let r = cover_applier(fresh_rebuilder().with_snapshot(|s, _| {
        s.applied.push("snap".to_string());
        Ok(())
    }));

    // Pages 1..3; snapshot covers through sequence 2 — re-applying covered
    // pages would double-fold their effects into state.
    let book = pb::EventBook {
        snapshot: Some(pb::Snapshot {
            sequence: 2,
            state: Some(cover_any("")),
            ..Default::default()
        }),
        pages: vec![
            sequenced_event_page(1, cover_any("covered1")),
            sequenced_event_page(2, cover_any("covered2")),
            sequenced_event_page(3, cover_any("after")),
        ],
        ..Default::default()
    };
    let (state, _) = r.rebuild(Some(&book)).expect("rebuild");
    assert_eq!(
        state.applied,
        vec!["snap", "after"],
        "covered pages skipped"
    );
}

// The covered-page skip is an exact boundary: a snapshot covering through
// sequence 1 skips the sequence-1 page and nothing after it.
#[test]
fn covered_boundary_exact_sequence_skipped() {
    let r = cover_applier(fresh_rebuilder().with_snapshot(|s, _| {
        s.applied.push("snap".to_string());
        Ok(())
    }));

    let book = pb::EventBook {
        snapshot: Some(pb::Snapshot {
            sequence: 1,
            state: Some(cover_any("")),
            ..Default::default()
        }),
        pages: vec![
            sequenced_event_page(1, cover_any("covered")),
            sequenced_event_page(2, cover_any("after")),
        ],
        ..Default::default()
    };
    let (state, _) = r.rebuild(Some(&book)).expect("rebuild");
    assert_eq!(
        state.applied,
        vec!["snap", "after"],
        "sequence 1 covered, sequence 2 not"
    );
}

#[test]
fn empty_book_no_prior_history() {
    let r = fresh_rebuilder();
    let (_, info) = r.rebuild(Some(&pb::EventBook::default())).expect("rebuild");
    assert!(
        !info.had_prior_events,
        "pageless, snapshotless history is fresh state"
    );
}

#[test]
fn snapshot_only_is_prior_history() {
    let r = fresh_rebuilder();
    let book = pb::EventBook {
        snapshot: Some(pb::Snapshot {
            sequence: 5,
            state: Some(cover_any("")),
            ..Default::default()
        }),
        ..Default::default()
    };
    let (_, info) = r.rebuild(Some(&book)).expect("rebuild");
    assert!(info.had_prior_events, "a snapshot alone is prior history");
}

#[test]
fn no_snapshot_in_book_loader_not_invoked() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let r = fresh_rebuilder().with_snapshot(move |_, _| {
        seen.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    r.rebuild(Some(&book_of_covers(&["a"]))).expect("rebuild");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "snapshot loader ran with no snapshot in the book"
    );
}

#[test]
fn snapshot_without_state_loader_not_invoked() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let r = fresh_rebuilder().with_snapshot(move |_, _| {
        seen.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let book = pb::EventBook {
        snapshot: Some(pb::Snapshot {
            sequence: 3, // no state payload
            ..Default::default()
        }),
        ..Default::default()
    };
    r.rebuild(Some(&book)).expect("rebuild");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "snapshot loader ran for a stateless snapshot"
    );
}

#[test]
fn snapshot_without_loader_ignored_safely() {
    let r = cover_applier(fresh_rebuilder());
    let mut book = book_of_covers(&["after"]);
    book.snapshot = Some(pb::Snapshot {
        state: Some(cover_any("base")),
        ..Default::default()
    });

    let (state, _) = r.rebuild(Some(&book)).expect("rebuild");
    assert_eq!(
        state.applied,
        vec!["after"],
        "no loader registered — snapshot state cannot fold"
    );
}

// A pageless entry (no event payload) is skipped, never ends the fold.
#[test]
fn pageless_entries_do_not_end_the_fold() {
    let r = cover_applier(fresh_rebuilder());
    let book = pb::EventBook {
        pages: vec![
            pb::EventPage::default(), // no payload
            event_page(cover_any("after-gap")),
        ],
        ..Default::default()
    };
    let (state, _) = r.rebuild(Some(&book)).expect("rebuild");
    assert_eq!(
        state.applied,
        vec!["after-gap"],
        "the gap page is skipped, not terminal"
    );
}

// A snapshot at sequence 0 covers page 0: the loaded state already holds it.
#[test]
fn snapshot_at_sequence_zero_covers_page_zero() {
    let r = cover_applier(fresh_rebuilder().with_snapshot(|s, _| {
        s.applied.push("snapshot".to_string());
        Ok(())
    }));
    let book = pb::EventBook {
        snapshot: Some(pb::Snapshot {
            sequence: 0,
            state: Some(cover_any("snap")),
            ..Default::default()
        }),
        pages: vec![
            sequenced_event_page(0, cover_any("covered0")),
            sequenced_event_page(1, cover_any("after")),
        ],
        ..Default::default()
    };
    let (state, info) = r.rebuild(Some(&book)).expect("rebuild");
    assert_eq!(state.applied, vec!["snapshot", "after"]);
    assert_eq!(info.applied_count, 1);
}

// Without a loader the snapshot state never folds, so no page is covered by
// it: skipping would silently lose that history.
#[test]
fn snapshot_without_loader_covers_no_pages() {
    let r = cover_applier(fresh_rebuilder());
    let book = pb::EventBook {
        snapshot: Some(pb::Snapshot {
            sequence: 2,
            state: Some(cover_any("snap")),
            ..Default::default()
        }),
        pages: vec![
            sequenced_event_page(1, cover_any("one")),
            sequenced_event_page(2, cover_any("two")),
            sequenced_event_page(3, cover_any("three")),
        ],
        ..Default::default()
    };
    let (state, _) = r.rebuild(Some(&book)).expect("rebuild");
    assert_eq!(state.applied, vec!["one", "two", "three"]);
}

// A snapshot with no state payload loads nothing, so it covers nothing.
#[test]
fn stateless_snapshot_covers_no_pages() {
    let r = cover_applier(fresh_rebuilder().with_snapshot(|s, _| {
        s.applied.push("snapshot".to_string());
        Ok(())
    }));
    let book = pb::EventBook {
        snapshot: Some(pb::Snapshot {
            sequence: 2,
            ..Default::default()
        }),
        pages: vec![
            sequenced_event_page(1, cover_any("one")),
            sequenced_event_page(2, cover_any("two")),
        ],
        ..Default::default()
    };
    let (state, _) = r.rebuild(Some(&book)).expect("rebuild");
    assert_eq!(state.applied, vec!["one", "two"]);
}

// A page with no explicit sequence cannot be proven covered, so it folds.
#[test]
fn unsequenced_page_is_never_covered() {
    let r = cover_applier(fresh_rebuilder().with_snapshot(|s, _| {
        s.applied.push("snapshot".to_string());
        Ok(())
    }));
    let book = pb::EventBook {
        snapshot: Some(pb::Snapshot {
            sequence: 4,
            state: Some(cover_any("snap")),
            ..Default::default()
        }),
        pages: vec![
            sequenced_event_page(4, cover_any("covered")),
            event_page(cover_any("unsequenced")),
        ],
        ..Default::default()
    };
    let (state, _) = r.rebuild(Some(&book)).expect("rebuild");
    assert_eq!(state.applied, vec!["snapshot", "unsequenced"]);
}

// Appliers registered with context see the book's cover and each page's
// explicit sequence (0 when absent).
#[test]
fn context_appliers_see_the_book_cover_and_page_sequence() {
    let r = fresh_rebuilder().apply_with_context(&cover_full_name(), |s, _any, ctx| {
        s.applied.push(format!(
            "{}@{}",
            ctx.cover.map_or("", |c| c.domain.as_str()),
            ctx.sequence
        ));
        Ok(())
    });
    let mut book = pb::EventBook {
        cover: Some(pb::Cover {
            domain: "ledger".to_string(),
            ..Default::default()
        }),
        pages: vec![
            sequenced_event_page(4, cover_any("a")),
            event_page(cover_any("b")),
        ],
        ..Default::default()
    };
    let (state, _) = r.rebuild(Some(&book)).expect("rebuild");
    assert_eq!(state.applied, vec!["ledger@4", "ledger@0"]);

    book.cover = None;
    let mut state = TestState::default();
    assert!(r
        .apply_page(
            &mut state,
            &book.pages[0],
            Some(&pb::Cover {
                domain: "facts".to_string(),
                ..Default::default()
            })
        )
        .expect("apply"));
    assert_eq!(state.applied, vec!["facts@4"]);
    assert!(!r
        .apply_page(&mut state, &pb::EventPage::default(), None)
        .expect("no event"));
}
