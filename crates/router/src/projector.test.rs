//! ProjectorDispatch contracts, transliterated from client-go's projector
//! dispatch (engine.go ProjectorDispatch.Dispatch + features/projector.go):
//! one instance folds every declared page (C-0086), undeclared domains fold
//! nothing but still finish (C-0032), unhandled event types are skipped
//! (C-0031), and a missing cover is the coded MISSING_EVENT_BOOK_COVER.

use std::sync::{Arc, Mutex};

use prost::Message;
use prost_types::Any;

use crate::error::{codes, HandlerError};
use crate::pb;
use crate::projector::{PageContext, ProjectorDispatch};
use crate::test_support::*;
use crate::type_url;

/// The fold-observing projection the Go bank calls the write model: records
/// each folded event's domain, in order.
#[derive(Default)]
struct Folded {
    entries: Vec<String>,
}

/// Folds a Cover event into the write log by domain.
fn fold_cover(p: &mut Folded, any: &Any, _ctx: &PageContext<'_>) -> Result<(), HandlerError> {
    let c =
        pb::Cover::decode(any.value.as_slice()).map_err(|e| HandlerError::Other(e.to_string()))?;
    p.entries.push(c.domain);
    Ok(())
}

/// An EventBook whose cover carries `domain`, over the given pages.
fn book(domain: &str, pages: Vec<pb::EventPage>) -> pb::EventBook {
    pb::EventBook {
        cover: Some(pb::Cover {
            domain: domain.to_string(),
            ..Default::default()
        }),
        pages,
        ..Default::default()
    }
}

/// A projector whose Finish reports the fold count as the projection
/// sequence — so the public dispatch result observes how many folds ran.
/// `domains == None` consumes every domain.
fn counting_projector(domains: Option<&[&str]>) -> ProjectorDispatch<Folded> {
    let mut d = ProjectorDispatch::new("write-model", Folded::default)
        .on_event(&cover_full_name(), fold_cover);
    if let Some(ds) = domains {
        d = d.for_domains(ds.iter().copied());
    }
    d.finish(|p, events| {
        Ok(pb::Projection {
            cover: events.cover.clone(),
            projector: "write-model".to_string(),
            next_sequence: p.entries.len() as u64,
            ..Default::default()
        })
    })
}

fn cover_pages(domain: &str, n: usize) -> Vec<pb::EventPage> {
    (0..n).map(|_| event_page(cover_any(domain))).collect()
}

#[test]
fn every_page_folds_into_one_projection() {
    let d = counting_projector(None);
    let proj = d
        .dispatch(&book("order", cover_pages("order", 3)))
        .expect("dispatch");
    assert_eq!(
        proj.next_sequence, 3,
        "all three pages fold into one projection"
    );
}

#[test]
fn single_instance_reused_across_delivery() {
    // A fresh instance per page would leave the fold count at most 1; five
    // folds into one sequence pins C-0086 (one instance per delivery).
    let d = counting_projector(None);
    let proj = d
        .dispatch(&book("order", cover_pages("order", 5)))
        .expect("dispatch");
    assert_eq!(proj.next_sequence, 5);
}

#[test]
fn unhandled_event_type_is_skipped() {
    // Two declared Cover events plus one undeclared type: only the declared
    // pair folds (C-0031), the unknown is skipped without error.
    let mut pages = cover_pages("order", 2);
    pages.insert(
        1,
        event_page(Any {
            type_url: type_url("test.Unregistered"),
            value: Vec::new(),
        }),
    );
    let d = counting_projector(None);
    let proj = d.dispatch(&book("order", pages)).expect("dispatch");
    assert_eq!(
        proj.next_sequence, 2,
        "unknown type skipped, declared pair folds"
    );
}

#[test]
fn undeclared_domain_folds_nothing_but_finishes() {
    // ForDomains("order") with a book in "inventory": no page folds (C-0032),
    // but Finish still runs and returns its projection.
    let d = counting_projector(Some(&["order"]));
    let proj = d
        .dispatch(&book("inventory", cover_pages("inventory", 3)))
        .expect("dispatch");
    assert_eq!(proj.next_sequence, 0, "undeclared domain folds nothing");
}

#[test]
fn declared_domain_folds() {
    let d = counting_projector(Some(&["order"]));
    let proj = d
        .dispatch(&book("order", cover_pages("order", 2)))
        .expect("dispatch");
    assert_eq!(proj.next_sequence, 2);
}

#[test]
fn the_wildcard_domain_folds_every_domain() {
    let d = counting_projector(Some(&[crate::WILDCARD_DOMAIN]));
    let proj = d
        .dispatch(&book("inventory", cover_pages("inventory", 3)))
        .expect("dispatch");
    assert_eq!(proj.next_sequence, 3, "\"*\" consumes every domain");
}

#[test]
fn the_wildcard_beside_named_domains_still_folds_every_domain() {
    let d = counting_projector(Some(&["order", crate::WILDCARD_DOMAIN]));
    let proj = d
        .dispatch(&book("billing", cover_pages("billing", 2)))
        .expect("dispatch");
    assert_eq!(proj.next_sequence, 2);
}

#[test]
fn empty_book_finishes_with_zero_folds() {
    let d = counting_projector(None);
    let proj = d.dispatch(&book("order", vec![])).expect("dispatch");
    assert_eq!(proj.next_sequence, 0);
}

#[test]
fn missing_cover_is_coded_error() {
    let d = counting_projector(None);
    let no_cover = pb::EventBook {
        pages: cover_pages("order", 1),
        ..Default::default()
    };
    let err = d.dispatch(&no_cover).expect_err("missing cover must fail");
    assert_eq!(err.code, codes::MISSING_EVENT_BOOK_COVER);
}

#[test]
fn no_finish_returns_default_projection() {
    // Without a finisher, dispatch returns a default Projection carrying the
    // cover and projector name (sequence 0, no payload).
    let d = ProjectorDispatch::new("write-model", Folded::default)
        .on_event(&cover_full_name(), fold_cover);
    let proj = d
        .dispatch(&book("order", cover_pages("order", 3)))
        .expect("dispatch");
    assert_eq!(proj.projector, "write-model");
    assert_eq!(proj.cover.expect("cover").domain, "order");
    assert_eq!(proj.next_sequence, 0);
    assert!(proj.projection.is_none());
}

#[test]
fn finish_customizes_projection() {
    let d = ProjectorDispatch::new("write-model", Folded::default)
        .on_event(&cover_full_name(), fold_cover)
        .finish(|p, events| {
            Ok(pb::Projection {
                cover: events.cover.clone(),
                projector: "write-model".to_string(),
                next_sequence: 99,
                projection: Some(cover_any(&p.entries.join(","))),
            })
        });
    let proj = d
        .dispatch(&book("order", cover_pages("order", 2)))
        .expect("dispatch");
    assert_eq!(proj.next_sequence, 99);
    assert!(proj.projection.is_some(), "finish sets a custom payload");
}

#[test]
fn accessors_report_name_and_registered_types() {
    let d = counting_projector(None);
    assert_eq!(d.name(), "write-model");
    assert_eq!(d.event_types(), vec![cover_full_name()]);
}

// Owner ruling 2026-10-19: any fold of accepted events that fails is a
// handler bug, named by its page and the projection type.
#[test]
fn a_failing_fold_is_a_handler_bug_naming_its_page() {
    for failure in [
        HandlerError::Other("boom".to_string()),
        HandlerError::Coded(crate::error::CodedError::invalid_argument(
            "SOME_CODE",
            "coded",
            [],
        )),
    ] {
        let failure = std::sync::Mutex::new(Some(failure));
        let d = ProjectorDispatch::new("write-model", Folded::default)
            .with_projection_type("test.Board")
            .on_event(&cover_full_name(), move |_p, _any, ctx| {
                if ctx.sequence == 1 {
                    return Err(failure.lock().unwrap().take().expect("one failure"));
                }
                Ok(())
            });
        let mut events = book("order", cover_pages("order", 2));
        events.pages[1].sequence = 1;
        let err = d
            .dispatch(&events)
            .expect_err("handler error must fail dispatch");
        let type_url = crate::type_url(&cover_full_name());
        assert_fold_failed(
            &err,
            crate::error::messages::FOLD_FAILED,
            &[
                (crate::error::extras::SEQUENCE, "1"),
                (crate::error::extras::TYPE_URL, type_url.as_str()),
            ],
            "/test.Board",
        );
        assert!(!err.extras[crate::error::extras::FOLD_MESSAGE].is_empty());
    }
}

#[test]
fn a_finish_failure_is_not_a_fold_failure() {
    let d = ProjectorDispatch::new("write-model", Folded::default)
        .finish(|_p, _events| Err(HandlerError::Other("boom".to_string())));
    let err = d
        .dispatch(&book("order", cover_pages("order", 1)))
        .expect_err("finish failed");
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
    assert_ne!(err.message, crate::error::messages::FOLD_FAILED);
}

// Each fold sees its page's place: the book's cover (root) and the page's
// sequence, for idempotent per-page handling.
#[test]
fn folds_observe_the_book_cover_and_page_sequence() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = seen.clone();
    let d = ProjectorDispatch::new("write-model", Folded::default).on_event(
        &cover_full_name(),
        move |_p, _any, ctx| {
            captured.lock().unwrap().push((
                ctx.cover
                    .and_then(|c| c.root.clone())
                    .map(|r| r.value.to_vec()),
                ctx.sequence,
            ));
            Ok(())
        },
    );
    let mut events = book("order", cover_pages("order", 2));
    events.cover.as_mut().unwrap().root = Some(uuid_of(9));
    events.pages[0].sequence = 4;
    d.dispatch(&events).expect("dispatch");
    assert_eq!(
        *seen.lock().unwrap(),
        vec![(Some(vec![9; 16]), 4), (Some(vec![9; 16]), 0)]
    );
}

// --- the batched fold (router-owned projections across the C ABI) -----------

/// Each batched fold call: its page sequences and cover domain.
type FoldCalls = Arc<Mutex<Vec<(Vec<u64>, String)>>>;

/// A projector whose batched fold records each call's page sequences and
/// cover domain, and counts the pages folded.
fn batched(calls: FoldCalls) -> ProjectorDispatch<Folded> {
    ProjectorDispatch::new("write-model", Folded::default)
        .for_domains(["order"])
        .with_fold(move |p: &mut Folded, pages, cover| {
            calls.lock().unwrap().push((
                pages.iter().map(|pg| pg.sequence).collect(),
                cover.domain.clone(),
            ));
            p.entries.extend(pages.iter().map(|_| "folded".to_string()));
            Ok(())
        })
        .finish(|p, events| {
            Ok(pb::Projection {
                cover: events.cover.clone(),
                next_sequence: p.entries.len() as u64,
                ..Default::default()
            })
        })
}

#[test]
fn a_batched_fold_sees_every_page_of_a_consumed_book_in_one_call() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut events = book("order", cover_pages("order", 3));
    for (seq, page) in events.pages.iter_mut().enumerate() {
        page.sequence = seq as u64 + 4;
    }
    let proj = batched(calls.clone()).dispatch(&events).expect("dispatch");
    assert_eq!(proj.next_sequence, 3, "all three pages folded");
    assert_eq!(
        *calls.lock().unwrap(),
        vec![(vec![4, 5, 6], "order".to_string())]
    );
}

#[test]
fn a_batched_fold_is_not_called_for_an_undeclared_domain_or_no_pages() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let d = batched(calls.clone());
    let proj = d
        .dispatch(&book("billing", cover_pages("billing", 2)))
        .expect("dispatch");
    assert_eq!(proj.next_sequence, 0, "finish still runs");
    d.dispatch(&book("order", vec![])).expect("dispatch");
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn a_batched_fold_failure_names_its_page_or_the_batchs_range() {
    let mut events = book("order", cover_pages("order", 3));
    for (page, seq) in events.pages.iter_mut().zip(4..) {
        page.sequence = seq;
    }
    let failing = |page_index: Option<usize>| {
        ProjectorDispatch::new("write-model", Folded::default)
            .with_projection_type("test.Board")
            .with_fold(move |_p, _pages, _c| {
                Err(crate::rebuild::FoldFailure {
                    page_index,
                    message: "boom".to_string(),
                })
            })
    };
    let err = failing(Some(2))
        .dispatch(&events)
        .expect_err("the fold failed");
    assert_fold_failed(
        &err,
        crate::error::messages::FOLD_FAILED,
        &[(crate::error::extras::SEQUENCE, "6")],
        "/test.Board",
    );
    let err = failing(Some(9))
        .dispatch(&events)
        .expect_err("the fold failed");
    assert_fold_failed(
        &err,
        crate::error::messages::FOLD_FAILED,
        &[
            (crate::error::extras::FIRST_SEQUENCE, "4"),
            (crate::error::extras::LAST_SEQUENCE, "6"),
        ],
        "/test.Board",
    );
}

// types.proto Projection: without a finish step the router still answers the
// projector's position: the last page's sequence + 1, or the book's
// next_sequence when it has no pages (as the C ABI does).
#[test]
fn without_a_finish_the_projection_reports_its_next_sequence() {
    let d = ProjectorDispatch::new("write-model", Folded::default);
    let mut events = book("order", cover_pages("order", 2));
    events.pages[0].sequence = 4;
    events.pages[1].sequence = 5;
    events.next_sequence = 9;
    assert_eq!(d.dispatch(&events).expect("dispatch").next_sequence, 6);
    let mut empty = book("order", vec![]);
    empty.next_sequence = 9;
    assert_eq!(d.dispatch(&empty).expect("dispatch").next_sequence, 9);
}
