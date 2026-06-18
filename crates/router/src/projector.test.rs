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
use crate::projector::ProjectorDispatch;
use crate::test_support::*;
use crate::type_url;

/// The fold-observing projection the Go bank calls the write model: records
/// each folded event's domain, in order.
#[derive(Default)]
struct Folded {
    entries: Vec<String>,
}

/// Folds a Cover event into the write log by domain.
fn fold_cover(p: &mut Folded, any: &Any) -> Result<(), HandlerError> {
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
            sequence: p.entries.len() as u32,
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
    assert_eq!(proj.sequence, 3, "all three pages fold into one projection");
}

#[test]
fn single_instance_reused_across_delivery() {
    // A fresh instance per page would leave the fold count at most 1; five
    // folds into one sequence pins C-0086 (one instance per delivery).
    let d = counting_projector(None);
    let proj = d
        .dispatch(&book("order", cover_pages("order", 5)))
        .expect("dispatch");
    assert_eq!(proj.sequence, 5);
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
        proj.sequence, 2,
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
    assert_eq!(proj.sequence, 0, "undeclared domain folds nothing");
}

#[test]
fn declared_domain_folds() {
    let d = counting_projector(Some(&["order"]));
    let proj = d
        .dispatch(&book("order", cover_pages("order", 2)))
        .expect("dispatch");
    assert_eq!(proj.sequence, 2);
}

#[test]
fn empty_book_finishes_with_zero_folds() {
    let d = counting_projector(None);
    let proj = d.dispatch(&book("order", vec![])).expect("dispatch");
    assert_eq!(proj.sequence, 0);
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
    assert_eq!(proj.sequence, 0);
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
                sequence: 99,
                projection: Some(cover_any(&p.entries.join(","))),
            })
        });
    let proj = d
        .dispatch(&book("order", cover_pages("order", 2)))
        .expect("dispatch");
    assert_eq!(proj.sequence, 99);
    assert!(proj.projection.is_some(), "finish sets a custom payload");
}

#[test]
fn on_unknown_observes_unhandled_type_url() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = seen.clone();
    let d = ProjectorDispatch::new("write-model", Folded::default)
        .on_event(&cover_full_name(), fold_cover)
        .on_unknown(move |url| captured.lock().unwrap().push(url.to_string()));
    let pages = vec![event_page(Any {
        type_url: type_url("test.Unregistered"),
        value: Vec::new(),
    })];
    d.dispatch(&book("order", pages)).expect("dispatch");
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[type_url("test.Unregistered")]
    );
}

#[test]
fn accessors_report_name_and_registered_types() {
    let d = counting_projector(None);
    assert_eq!(d.name(), "write-model");
    assert_eq!(d.event_types(), vec![cover_full_name()]);
}

#[test]
fn handler_error_propagates_as_unhandled() {
    let d = ProjectorDispatch::new("write-model", Folded::default)
        .on_event(&cover_full_name(), |_p, _any| {
            Err(HandlerError::Other("boom".to_string()))
        });
    let err = d
        .dispatch(&book("order", cover_pages("order", 1)))
        .expect_err("handler error must fail dispatch");
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
}
