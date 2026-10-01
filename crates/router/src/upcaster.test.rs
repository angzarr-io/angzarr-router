//! UpcasterDispatch: ordered rule chains over event pages (C-0136, C-0137).

use prost_types::Any;

use crate::error::{codes, HandlerError};
use crate::pb;
use crate::test_support::event_page;
use crate::type_url;
use crate::upcaster::UpcasterDispatch;

fn ev(fq: &str) -> Any {
    Any {
        type_url: type_url(fq),
        value: Vec::new(),
    }
}

fn to(fq: &'static str) -> impl Fn(&Any) -> Result<Any, HandlerError> + Send + Sync + 'static {
    move |_| Ok(ev(fq))
}

fn types(resp: &pb::UpcastResponse) -> Vec<String> {
    resp.events
        .iter()
        .map(|p| match p.payload.as_ref() {
            Some(pb::event_page::Payload::Event(any)) => {
                crate::type_name_from_url(&any.type_url).to_string()
            }
            _ => String::new(),
        })
        .collect()
}

fn request(domain: &str, fqs: &[&str]) -> pb::UpcastRequest {
    pb::UpcastRequest {
        domain: domain.to_string(),
        events: fqs.iter().map(|fq| event_page(ev(fq))).collect(),
    }
}

#[test]
fn chained_rules_transform_across_two_versions() {
    let u = UpcasterDispatch::new("order-upcaster", "order")
        .on_event("test.V1", to("test.V2"))
        .on_event("test.V2", to("test.V3"));
    let resp = u.dispatch(&request("order", &["test.V1"])).expect("upcast");
    assert_eq!(types(&resp), vec!["test.V3"], "C-0136");
}

#[test]
fn chain_stops_when_no_rule_matches_the_current_type() {
    let u = UpcasterDispatch::new("order-upcaster", "order")
        .on_event("test.V1", to("test.V2"))
        .on_event("test.V3", to("test.V4"));
    let resp = u.dispatch(&request("order", &["test.V1"])).expect("upcast");
    assert_eq!(types(&resp), vec!["test.V2"], "C-0137");
}

#[test]
fn rules_apply_in_registration_order_only() {
    // V2 -> V3 registered before V1 -> V2: one ordered pass yields V2.
    let u = UpcasterDispatch::new("order-upcaster", "order")
        .on_event("test.V2", to("test.V3"))
        .on_event("test.V1", to("test.V2"));
    let resp = u.dispatch(&request("order", &["test.V1"])).expect("upcast");
    assert_eq!(types(&resp), vec!["test.V2"]);
}

#[test]
fn pages_keep_order_headers_and_unmatched_events() {
    let u = UpcasterDispatch::new("order-upcaster", "order").on_event("test.V1", to("test.V2"));
    let mut req = request("order", &["test.Other", "test.V1"]);
    req.events[1].header = Some(pb::PageHeader {
        sequence_type: Some(pb::page_header::SequenceType::Sequence(3)),
        ..Default::default()
    });
    req.events.push(pb::EventPage::default());
    let resp = u.dispatch(&req).expect("upcast");
    assert_eq!(types(&resp), vec!["test.Other", "test.V2", ""]);
    assert_eq!(resp.events[1].header, req.events[1].header);
}

#[test]
fn another_domain_passes_through_unchanged() {
    let u = UpcasterDispatch::new("order-upcaster", "order").on_event("test.V1", to("test.V2"));
    let req = request("billing", &["test.V1"]);
    let resp = u.dispatch(&req).expect("upcast");
    assert_eq!(resp.events, req.events);
}

#[test]
fn a_failing_rule_is_coded() {
    let u = UpcasterDispatch::new("order-upcaster", "order").on_event("test.V1", |_| {
        Err(HandlerError::Other("bad payload".to_string()))
    });
    let err = u
        .dispatch(&request("order", &["test.V1"]))
        .expect_err("rule error");
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
}

#[test]
fn accessors_report_identity_and_source_types() {
    let u = UpcasterDispatch::new("order-upcaster", "order")
        .on_event("test.V1", to("test.V2"))
        .on_event("test.V2", to("test.V3"));
    assert_eq!(u.name(), "order-upcaster");
    assert_eq!(u.domain(), "order");
    assert_eq!(u.source_types(), vec!["test.V1", "test.V2"]);
}
