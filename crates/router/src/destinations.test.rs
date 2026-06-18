//! Destinations contracts, transliterated from client-go's destinations.go:
//! the coordinator supplies next-sequences per output domain; the saga/PM
//! reads them (sequence_for/has/domains) and stamps every page of an emitted
//! command (stamp_command), and a domain with no supplied sequence is the
//! coded MISSING_DESTINATION_SEQUENCE.

use std::collections::HashMap;

use crate::destinations::Destinations;
use crate::error::{codes, extras};
use crate::pb;

/// A sequence map over the given (domain, seq) pairs.
fn seqs(pairs: &[(&str, u32)]) -> HashMap<String, u32> {
    pairs.iter().map(|(d, s)| (d.to_string(), *s)).collect()
}

/// A command book targeting `domain` with `n` empty pages.
fn command_book(domain: &str, n: usize) -> pb::CommandBook {
    pb::CommandBook {
        cover: Some(pb::Cover {
            domain: domain.to_string(),
            ..Default::default()
        }),
        pages: (0..n).map(|_| pb::CommandPage::default()).collect(),
    }
}

fn page_sequence(page: &pb::CommandPage) -> Option<u32> {
    match page.header.as_ref().and_then(|h| h.sequence_type.as_ref()) {
        Some(pb::page_header::SequenceType::Sequence(seq)) => Some(*seq),
        _ => None,
    }
}

#[test]
fn sequence_for_returns_supplied_sequence() {
    let d = Destinations::new(seqs(&[("inventory", 7), ("fulfillment", 3)]));
    assert_eq!(d.sequence_for("inventory"), Some(7));
    assert_eq!(d.sequence_for("fulfillment"), Some(3));
}

#[test]
fn sequence_for_absent_domain_is_none() {
    let d = Destinations::new(seqs(&[("inventory", 7)]));
    assert_eq!(d.sequence_for("shipping"), None);
}

#[test]
fn has_reflects_presence() {
    let d = Destinations::new(seqs(&[("inventory", 7)]));
    assert!(d.has("inventory"));
    assert!(!d.has("shipping"));
}

#[test]
fn domains_lists_every_supplied_domain() {
    let d = Destinations::new(seqs(&[("inventory", 7), ("fulfillment", 3)]));
    let mut got = d.domains();
    got.sort();
    assert_eq!(got, vec!["fulfillment".to_string(), "inventory".to_string()]);
}

#[test]
fn empty_destinations_have_no_domains() {
    let d = Destinations::new(HashMap::new());
    assert!(d.domains().is_empty());
    assert!(!d.has("inventory"));
    assert_eq!(d.sequence_for("inventory"), None);
}

#[test]
fn stamp_command_sets_sequence_on_every_page() {
    let d = Destinations::new(seqs(&[("inventory", 42)]));
    let mut cmd = command_book("inventory", 3);
    d.stamp_command(&mut cmd, "inventory").expect("stamp");
    for page in &cmd.pages {
        assert_eq!(page_sequence(page), Some(42), "every page carries the sequence");
    }
}

#[test]
fn stamp_command_missing_domain_is_coded() {
    let d = Destinations::new(seqs(&[("inventory", 7)]));
    let mut cmd = command_book("shipping", 1);
    let err = d
        .stamp_command(&mut cmd, "shipping")
        .expect_err("missing destination sequence must fail");
    assert_eq!(err.code, codes::MISSING_DESTINATION_SEQUENCE);
    assert_eq!(err.extras.get(extras::DOMAIN), Some(&"shipping".to_string()));
}

#[test]
fn stamp_command_missing_domain_leaves_pages_unstamped() {
    let d = Destinations::new(HashMap::new());
    let mut cmd = command_book("inventory", 2);
    let _ = d.stamp_command(&mut cmd, "inventory");
    for page in &cmd.pages {
        assert_eq!(page_sequence(page), None, "no sequence stamped on failure");
    }
}
