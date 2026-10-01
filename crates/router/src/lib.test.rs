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
    assert_eq!(
        type_name_from_url("examples.CardsDealt"),
        "examples.CardsDealt"
    );
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
    let rejection =
        pb::RejectionNotification::decode(notification.payload.expect("payload").value.as_slice())
            .expect("rejection");
    assert_eq!(
        extract_rejection_key(&rejection),
        ("inventory".to_string(), "examples.ReserveStock".to_string())
    );
}

// --- deferred stamping of saga/PM-emitted commands -------------------------

fn cmd_with_pages(domain: &str, n: usize) -> pb::CommandBook {
    pb::CommandBook {
        cover: Some(pb::Cover {
            domain: domain.to_string(),
            ..Default::default()
        }),
        pages: (0..n).map(|_| pb::CommandPage::default()).collect(),
    }
}

fn deferred(page: &pb::CommandPage) -> Option<&pb::AngzarrDeferredSequence> {
    match page.header.as_ref()?.sequence_type.as_ref()? {
        pb::page_header::SequenceType::AngzarrDeferred(d) => Some(d),
        _ => None,
    }
}

fn source_cover() -> pb::Cover {
    pb::Cover {
        domain: "order".to_string(),
        root: Some(pb::Uuid {
            value: vec![1, 2, 3],
        }),
        correlation_id: "corr".to_string(),
        ..Default::default()
    }
}

#[test]
fn stamp_deferred_records_source_seq_and_emission_index() {
    let mut cmds = vec![
        cmd_with_pages("inventory", 2),
        cmd_with_pages("fulfillment", 1),
    ];
    stamp_deferred(&mut cmds, Some(&source_cover()), 4);
    for (index, cmd) in cmds.iter().enumerate() {
        for page in &cmd.pages {
            let d = deferred(page).expect("every page is deferred");
            assert_eq!(d.source.as_ref(), Some(&source_cover()));
            assert_eq!(d.source_seq, 4);
            assert_eq!(d.command_index, index as u32);
            assert!(
                d.source_component.is_empty(),
                "the coordinator stamps the component"
            );
        }
    }
}

#[test]
fn stamp_deferred_replaces_an_explicit_sequence_and_keeps_sync_mode() {
    let mut cmd = cmd_with_pages("inventory", 1);
    cmd.pages[0].header = Some(pb::PageHeader {
        sequence_type: Some(pb::page_header::SequenceType::Sequence(9)),
        sync_mode: Some(2),
    });
    let mut cmds = vec![cmd];
    stamp_deferred(&mut cmds, Some(&source_cover()), 1);
    let header = cmds[0].pages[0].header.as_ref().unwrap();
    assert!(
        deferred(&cmds[0].pages[0]).is_some(),
        "no explicit sequence survives"
    );
    assert_eq!(header.sync_mode, Some(2), "a per-command sync_mode is kept");
}

#[test]
fn stamp_deferred_without_a_source_cover_leaves_source_unset() {
    let mut cmds = vec![cmd_with_pages("inventory", 1)];
    stamp_deferred(&mut cmds, None, 0);
    let d = deferred(&cmds[0].pages[0]).unwrap();
    assert_eq!(d.source, None);
    assert_eq!(d.command_index, 0);
}

// --- compensation keys: "fq.Type" or "domain:fq.Type" ----------------------

fn keyed(keys: &[&str]) -> std::collections::HashMap<String, &'static str> {
    let labels = ["a", "b", "c"];
    keys.iter()
        .zip(labels)
        .map(|(k, l)| (k.to_string(), l))
        .collect()
}

#[test]
fn unqualified_key_matches_any_target_domain() {
    let map = keyed(&["inv.Reserve"]);
    assert_eq!(
        compensation_lookup(&map, "warehouse", "inv.Reserve"),
        Some(&"a")
    );
    assert_eq!(
        compensation_lookup(&map, "inventory", "inv.Reserve"),
        Some(&"a")
    );
    assert_eq!(compensation_lookup(&map, "inventory", "inv.Other"), None);
}

#[test]
fn qualified_key_matches_only_its_domain() {
    let map = keyed(&["inventory:inv.Reserve", "warehouse:inv.Reserve"]);
    assert_eq!(
        compensation_lookup(&map, "warehouse", "inv.Reserve"),
        Some(&"b")
    );
    assert_eq!(
        compensation_lookup(&map, "inventory", "inv.Reserve"),
        Some(&"a")
    );
    assert_eq!(compensation_lookup(&map, "billing", "inv.Reserve"), None);
}

#[test]
fn a_type_listed_both_unqualified_and_qualified_is_refused() {
    let err = validate_compensation_keys(["inv.Reserve", "inventory:inv.Reserve"])
        .expect_err("ambiguous");
    assert_eq!(err.code, crate::error::codes::AMBIGUOUS_COMPENSATION);
    assert!(validate_compensation_keys(["inv.Reserve", "inventory:inv.Other"]).is_ok());
    assert!(validate_compensation_keys(["inventory:inv.Reserve", "warehouse:inv.Reserve"]).is_ok());
}

// --- notification payload discrimination ----------------------------------

fn notification_any(payload: Option<prost_types::Any>) -> prost_types::Any {
    prost_types::Any {
        type_url: type_url(NOTIFICATION_FULL_NAME),
        value: prost::Message::encode_to_vec(&pb::Notification {
            payload,
            ..Default::default()
        }),
    }
}

#[test]
fn compensate_payload_decodes_under_any_prefix() {
    let compensate = pb::Compensate {
        command_type: "inv.AdjustStock".to_string(),
        sequences: vec![3],
        reason: "aborted".to_string(),
    };
    let any = notification_any(Some(prost_types::Any {
        type_url: format!("type.googleapis.com/{COMPENSATE_FULL_NAME}"),
        value: prost::Message::encode_to_vec(&compensate),
    }));
    let (_, payload) = decode_notification(&any).expect("decodes");
    assert_eq!(payload, NotificationPayload::Compensate(compensate));
}

#[test]
fn rejection_payload_decodes_and_an_untyped_payload_reads_as_rejection() {
    let rejection = pb::RejectionNotification {
        rejection_reason: "out_of_stock".to_string(),
        ..Default::default()
    };
    for url in [type_url(REJECTION_NOTIFICATION_FULL_NAME), String::new()] {
        let any = notification_any(Some(prost_types::Any {
            type_url: url,
            value: prost::Message::encode_to_vec(&rejection),
        }));
        let (_, payload) = decode_notification(&any).expect("decodes");
        assert_eq!(payload, NotificationPayload::Rejection(rejection.clone()));
    }
    let (_, payload) = decode_notification(&notification_any(None)).expect("decodes");
    assert_eq!(payload, NotificationPayload::Rejection(Default::default()));
}

#[test]
fn unknown_or_corrupt_payloads_are_coded() {
    let unknown = notification_any(Some(prost_types::Any {
        type_url: type_url("test.Other"),
        value: Vec::new(),
    }));
    assert_eq!(
        decode_notification(&unknown).unwrap_err().code,
        crate::error::codes::UNKNOWN_NOTIFICATION_PAYLOAD
    );
    let corrupt_compensate = notification_any(Some(prost_types::Any {
        type_url: type_url(COMPENSATE_FULL_NAME),
        value: vec![0xff, 0xff],
    }));
    assert_eq!(
        decode_notification(&corrupt_compensate).unwrap_err().code,
        crate::error::codes::COMPENSATE_DECODE_FAILED
    );
    let corrupt_rejection = notification_any(Some(prost_types::Any {
        type_url: type_url(REJECTION_NOTIFICATION_FULL_NAME),
        value: vec![0xff, 0xff],
    }));
    assert_eq!(
        decode_notification(&corrupt_rejection).unwrap_err().code,
        crate::error::codes::REJECTION_NOTIFICATION_DECODE_FAILED
    );
    let corrupt = prost_types::Any {
        type_url: type_url(NOTIFICATION_FULL_NAME),
        value: vec![0xff, 0xff],
    };
    assert_eq!(
        decode_notification(&corrupt).unwrap_err().code,
        crate::error::codes::NOTIFICATION_DECODE_FAILED
    );
}
