//! Cross-cutting helper contracts: exact-match notification detection,
//! type-URL parsing, and degenerate rejection-key shapes (transliterated
//! from engine_boundaries_test.go).

use super::*;
use crate::test_support::*;

#[test]
fn type_name_from_url_strips_prefix() {
    assert_eq!(
        type_name_from_url("type.googleapis.com/examples.CardsDealt"),
        "examples.CardsDealt"
    );
    assert_eq!(type_name_from_url("examples.CardsDealt"), "examples.CardsDealt");
}

#[test]
fn notification_detection_matches_full_fqn_any_prefix() {
    // The bare canonical form angzarr produces.
    assert!(is_notification_type_url(NOTIFICATION_TYPE_URL));
    // What other-language bindings emit (Any.Pack default) MUST also route —
    // recognition is prefix-agnostic on the full FQN.
    assert!(is_notification_type_url(
        "type.googleapis.com/io.angzarr.v1.Notification"
    ));
    // Full-FQN (not suffix) match: a user type ending in "Notification"
    // never misroutes.
    assert!(!is_notification_type_url(
        "type.googleapis.com/examples.PaymentNotification"
    ));
    // A different (e.g. pre-rename) package is a different FQN — no match.
    assert!(!is_notification_type_url(
        "/angzarr_client.proto.angzarr.v1.Notification"
    ));
}

#[test]
fn next_sequence_absent_book_is_zero() {
    assert_eq!(next_sequence(None), 0);
    assert_eq!(
        next_sequence(Some(&pb::EventBook {
            next_sequence: 9,
            ..Default::default()
        })),
        9
    );
}

#[test]
fn extract_rejection_key_degenerate_shapes() {
    // no rejected command
    assert_eq!(
        extract_rejection_key(&pb::RejectionNotification::default()),
        (String::new(), String::new())
    );

    // no cover
    let no_cover = pb::RejectionNotification {
        rejected_command: Some(pb::CommandBook {
            pages: vec![pb::CommandPage {
                payload: Some(pb::command_page::Payload::Command(prost_types::Any {
                    type_url: format!("{TYPE_URL_PREFIX}test.Cmd"),
                    value: Vec::new(),
                })),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        extract_rejection_key(&no_cover),
        (String::new(), "test.Cmd".to_string())
    );

    // no pages
    let no_pages = pb::RejectionNotification {
        rejected_command: Some(pb::CommandBook {
            cover: Some(pb::Cover {
                domain: "orders".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        extract_rejection_key(&no_pages),
        ("orders".to_string(), String::new())
    );
}

#[test]
fn well_formed_rejection_key_extracts_both_parts() {
    let page = notification_page_for("examples.ReserveStock");
    let event = page_event(&page).expect("event");
    use prost::Message;
    let notification = pb::Notification::decode(event.value.as_slice()).expect("notification");
    let rejection = pb::RejectionNotification::decode(
        notification.payload.expect("payload").value.as_slice(),
    )
    .expect("rejection");
    assert_eq!(
        extract_rejection_key(&rejection),
        ("inventory".to_string(), "examples.ReserveStock".to_string())
    );
}
