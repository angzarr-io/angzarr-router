//! Cross-cutting helper contracts: type-URL parsing, next-sequence and
//! deferred stamping (transliterated from engine_boundaries_test.go).

use super::*;

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

// --- notification payload discrimination ----------------------------------
