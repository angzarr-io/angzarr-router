//! Destinations: the declared output domains, in declaration order.

use prost::Message;
use sha2::{Digest, Sha256};

use crate::destinations::Destinations;
use crate::error::{codes, extras, GrpcCode};
use crate::pb;

#[test]
fn has_domain_reflects_declared_domains() {
    let d = Destinations::new(["inventory", "fulfillment"]);
    assert!(d.has_domain("inventory"));
    assert!(d.has_domain("fulfillment"));
    assert!(!d.has_domain("shipping"));
    assert!(!d.has_domain(""));
}

#[test]
fn domains_keep_declaration_order() {
    let d = Destinations::new(["inventory", "fulfillment"]);
    assert_eq!(
        d.domains(),
        ["inventory".to_string(), "fulfillment".to_string()]
    );
}

#[test]
fn no_declared_domains_has_none() {
    let d = Destinations::new(Vec::<String>::new());
    assert!(!d.has_domain("inventory"));
    assert!(d.domains().is_empty());
}

fn source_cover() -> pb::Cover {
    pb::Cover {
        domain: "order".to_string(),
        root: Some(pb::Uuid {
            value: (0x00u8..=0x0f).collect(),
        }),
        correlation_id: "corr-1".to_string(),
        ..Default::default()
    }
}

/// The wire_parity.feature fixture: one command page to "inventory".
fn foo_command() -> pb::CommandBook {
    pb::CommandBook {
        cover: Some(pb::Cover {
            domain: "inventory".to_string(),
            root: Some(pb::Uuid {
                value: (0x10u8..=0x1f).collect(),
            }),
            correlation_id: "corr-1".to_string(),
            ..Default::default()
        }),
        pages: vec![pb::CommandPage {
            payload: Some(pb::command_page::Payload::Command(prost_types::Any {
                type_url: "/example.Foo".to_string(),
                value: vec![1, 2, 3, 4],
            })),
            ..Default::default()
        }],
    }
}

fn deferred(page: &pb::CommandPage) -> &pb::AngzarrDeferredSequence {
    match page.header.as_ref().and_then(|h| h.sequence_type.as_ref()) {
        Some(pb::page_header::SequenceType::AngzarrDeferred(d)) => d,
        other => panic!("not deferred: {other:?}"),
    }
}

#[test]
fn stamp_command_makes_every_page_deferred_and_keeps_sync_mode() {
    let mut cmd = foo_command();
    cmd.pages.push(pb::CommandPage {
        header: Some(pb::PageHeader {
            sequence_type: Some(pb::page_header::SequenceType::Sequence(9)),
            sync_mode: Some(pb::SyncMode::Cascade as i32),
        }),
        ..Default::default()
    });
    Destinations::new(["inventory"])
        .stamp_command(&mut cmd, "inventory", &source_cover(), 3, 2)
        .expect("a declared domain");
    for page in &cmd.pages {
        let d = deferred(page);
        assert_eq!(d.source.as_ref(), Some(&source_cover()));
        assert_eq!(d.source_seq, 3);
        assert_eq!(d.command_index, 2);
        assert_eq!(
            d.source_component, "",
            "the coordinator stamps the component"
        );
    }
    assert_eq!(
        cmd.pages[1].header.as_ref().unwrap().sync_mode,
        Some(pb::SyncMode::Cascade as i32),
        "a page's sync_mode is kept"
    );
    assert_eq!(cmd.pages[0].header.as_ref().unwrap().sync_mode, None);
}

#[test]
fn stamp_command_refuses_an_undeclared_domain_and_stamps_nothing() {
    let mut cmd = foo_command();
    let err = Destinations::new(["inventory"])
        .stamp_command(&mut cmd, "shipping", &source_cover(), 3, 0)
        .expect_err("an undeclared domain");
    assert_eq!(err.code, codes::UNDECLARED_OUTPUT_DOMAIN);
    assert_eq!(err.grpc, GrpcCode::InvalidArgument);
    assert_eq!(
        err.extras.get(extras::DOMAIN).map(String::as_str),
        Some("shipping")
    );
    assert_eq!(cmd.pages[0].header, None, "nothing stamped on error");
}

fn stamped_hash(command_index: u32) -> String {
    let mut cmd = foo_command();
    Destinations::new(["inventory"])
        .stamp_command(&mut cmd, "inventory", &source_cover(), 3, command_index)
        .expect("a declared domain");
    format!("{:x}", Sha256::digest(cmd.encode_to_vec()))
}

/// parity/client/wire_parity.feature C-0182.
#[test]
fn stamp_command_wire_parity_first_command() {
    assert_eq!(
        stamped_hash(0),
        "10b1ce23a470f107662591a7da41830c724fdc0c9562824130f09b0a12f011f5"
    );
}

/// parity/client/wire_parity.feature C-0183.
#[test]
fn stamp_command_wire_parity_later_command() {
    assert_eq!(
        stamped_hash(1),
        "a028d427e91c0e03b63b50dabe7c5184417ed7aaf558d62dfdb9baed1e3affd0"
    );
}

#[test]
fn deferred_header_records_the_provenance_without_a_sync_mode() {
    let header = Destinations::deferred_header(&source_cover(), 4, 1);
    assert_eq!(header.sync_mode, None);
    let page = pb::CommandPage {
        header: Some(header),
        ..Default::default()
    };
    let d = deferred(&page);
    assert_eq!(d.source.as_ref(), Some(&source_cover()));
    assert_eq!((d.source_seq, d.command_index), (4, 1));
    assert_eq!(d.source_component, "");
}
