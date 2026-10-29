//! Who a policy's command addresses, and the correlation it carries
//! (decision 14.36 as amended by the owner, 2026-10-28: the router does not
//! differentiate process managers; angzarr-project spec/20261019-draft:
//! policy.proto PolicyResponse, process_manager.proto Stream, types.proto
//! Cover.root; features/coordinator-contract/policy_addressing.feature, the
//! router's half of C-0656 to C-0660 and C-0665).
//!
//! The router's half: every command a policy sends names its root, whatever
//! its target (aggregate or process manager alike; there is no root
//! callback and no default root), and one that names none is refused; and
//! a command carrying a correlation other than its own trigger's is
//! refused.

use prost::Message;
use prost_types::Any;

use crate::error::{codes, extras, CodedError, GrpcCode};
use crate::pb;
use crate::policy::PolicyDispatch;
use crate::test_support::*;

/// The trigger's correlation (C-0656's ...0040, as a test UUID).
const CORRELATION: u8 = 0x40;
/// The fulfillment instance the policy names (C-0658's ...00aa).
const NAMED: u8 = 0xaa;
/// The inventory aggregate the policy names ("inv-7").
const INVENTORY: u8 = 7;

/// A target for the no-root test: its domain, a builder of its command
/// from a root, and that command's type.
type Target = (
    &'static str,
    fn(Option<pb::Uuid>) -> pb::CommandBook,
    String,
);

// --- fixtures ----------------------------------------------------------------

/// An OrderPlaced page at `sequence` of order 1, carrying `correlation`.
fn placed(sequence: u64, quantity: u32, correlation: u8) -> pb::EventPage {
    let mut page = sequenced_event_page(sequence, any_of(&OrderPlaced { quantity }));
    page.meta = Some(pb::EventMeta {
        correlation_id: Some(uuid_of(correlation)),
        ..Default::default()
    });
    page
}

/// The source book: order 1 in `edition`, over `pages`.
fn order_source(edition: Option<&str>, pages: Vec<pb::EventPage>) -> pb::EventBook {
    pb::EventBook {
        cover: Some(stream("order", edition, 1)),
        pages,
        ..Default::default()
    }
}

/// One OrderPlaced at sequence 4 of order 1 on main, correlation 0x40.
fn one_placed() -> pb::EventBook {
    order_source(None, vec![placed(4, 3, CORRELATION)])
}

/// A command book to `domain`, root `root` (None: unset), carrying
/// `command`, its identity left for the coordinator.
fn command_to(domain: &str, root: Option<pb::Uuid>, command: Any) -> pb::CommandBook {
    pb::CommandBook {
        cover: Some(pb::Cover {
            domain: domain.to_string(),
            root,
            edition: None,
        }),
        page: Some(command_page(command)),
        ..Default::default()
    }
}

fn reserve(root: Option<pb::Uuid>, quantity: u32) -> pb::CommandBook {
    command_to("inventory", root, any_of(&ReserveStock { quantity }))
}

fn start(root: Option<pb::Uuid>, quantity: u32) -> pb::CommandBook {
    command_to("fulfillment", root, any_of(&StartFulfillment { quantity }))
}

fn quantity_of(event: &Any) -> u32 {
    OrderPlaced::decode(event.value.as_slice())
        .expect("OrderPlaced")
        .quantity
}

/// OrderToInventory (an aggregate target): ReserveStock to inventory 7.
fn order_to_inventory() -> PolicyDispatch {
    PolicyDispatch::new("OrderToInventory", "order", "inventory")
        .on_event(&fq::<OrderPlaced>(), |event, _cover| {
            Ok(vec![reserve(Some(uuid_of(INVENTORY)), quantity_of(event))])
        })
}

/// OrderToFulfillment (a process-manager target, built the same way):
/// StartFulfillment to the instance it names, root 0xaa.
fn order_to_fulfillment() -> PolicyDispatch {
    PolicyDispatch::new("OrderToFulfillment", "order", "fulfillment")
        .on_event(&fq::<OrderPlaced>(), |event, _cover| {
            Ok(vec![start(Some(uuid_of(NAMED)), quantity_of(event))])
        })
}

fn cover_of(book: &pb::CommandBook) -> &pb::Cover {
    book.cover.as_ref().expect("a cover")
}

fn correlation_of(book: &pb::CommandBook) -> Option<&pb::Uuid> {
    book.page.as_ref().and_then(|p| p.correlation_id.as_ref())
}

/// The command carries its trigger's correlation, or leaves it for the
/// coordinator to stamp with exactly that; never another one.
fn keeps_the_triggers_correlation(book: &pb::CommandBook, trigger: u8) {
    let carried = correlation_of(book);
    assert!(
        carried.is_none() || carried == Some(&uuid_of(trigger)),
        "a policy's command carries its trigger's correlation, got {carried:?}"
    );
}

fn refused_internal(err: CodedError, code: &str) -> CodedError {
    assert_eq!(err.code, code);
    assert_eq!(err.grpc, GrpcCode::Internal, "a policy bug");
    err
}

// --- the policy names the root, for every target (C-0656, C-0658, C-0665) ----

// C-0656 and C-0658: a command lands on the root it names, an aggregate's
// or a process manager's alike, with the trigger's correlation.
#[test]
fn a_command_keeps_the_root_it_names_for_any_target() {
    for (policy, root) in [
        (order_to_inventory(), INVENTORY),
        (order_to_fulfillment(), NAMED),
    ] {
        let resp = policy.dispatch(&one_placed()).expect("dispatched");
        assert_eq!(resp.commands.len(), 1);
        assert_eq!(cover_of(&resp.commands[0]).root, Some(uuid_of(root)));
        keeps_the_triggers_correlation(&resp.commands[0], CORRELATION);
    }
}

// C-0665, for every target: a command naming no root is a policy bug,
// refused INTERNAL (dead-lettered and alerted) with no commands, and no
// root is filled in. A process manager is not differentiated: there is no
// default root. A root whose UUID holds no bytes names none either.
#[test]
fn a_command_naming_no_root_is_refused_for_any_target() {
    let targets: [Target; 2] = [
        ("inventory", |root| reserve(root, 3), fq::<ReserveStock>()),
        (
            "fulfillment",
            |root| start(root, 3),
            fq::<StartFulfillment>(),
        ),
    ];
    for (domain, build, command_type) in targets {
        for root in [None, Some(pb::Uuid::default())] {
            let policy = PolicyDispatch::new("OrderTo", "order", domain)
                .on_event(&fq::<OrderPlaced>(), move |_event, _cover| {
                    Ok(vec![build(root.clone())])
                });
            let err = refused_internal(
                policy
                    .dispatch(&one_placed())
                    .expect_err("a policy's command names its root"),
                codes::POLICY_COMMAND_NO_ROOT,
            );
            let extra = |key: &str| err.extras.get(key).map(String::as_str);
            assert_eq!(extra(extras::DOMAIN), Some(domain));
            assert_eq!(extra(extras::COMMAND_TYPE), Some(command_type.as_str()));
        }
    }
}

// --- correlation traces the causal chain --------------------------------------

// Every command a policy sends, to any target, carries its trigger's
// correlation unchanged: a policy cannot change it. One that sets another
// is a policy bug, refused INTERNAL with no commands.
#[test]
fn a_command_carrying_another_correlation_is_refused() {
    let other = |mut book: pb::CommandBook| {
        book.page.as_mut().unwrap().correlation_id = Some(uuid_of(CORRELATION + 1));
        book
    };
    let to_inventory = PolicyDispatch::new("OrderToInventory", "order", "inventory")
        .on_event(&fq::<OrderPlaced>(), move |_e, _c| {
            Ok(vec![other(reserve(Some(uuid_of(INVENTORY)), 3))])
        });
    let to_fulfillment = PolicyDispatch::new("OrderToFulfillment", "order", "fulfillment")
        .on_event(&fq::<OrderPlaced>(), move |_e, _c| {
            Ok(vec![other(start(Some(uuid_of(NAMED)), 3))])
        });
    for policy in [to_inventory, to_fulfillment] {
        refused_internal(
            policy
                .dispatch(&one_placed())
                .expect_err("another correlation"),
            codes::POLICY_COMMAND_OTHER_CORRELATION,
        );
    }
}

// Setting exactly the trigger's correlation is not changing it.
#[test]
fn a_command_carrying_its_triggers_correlation_passes() {
    let policy = PolicyDispatch::new("OrderToInventory", "order", "inventory").on_event(
        &fq::<OrderPlaced>(),
        |_e, _c| {
            let mut book = reserve(Some(uuid_of(INVENTORY)), 3);
            book.page.as_mut().unwrap().correlation_id = Some(uuid_of(CORRELATION));
            Ok(vec![book])
        },
    );
    let resp = policy.dispatch(&one_placed()).expect("its trigger's own");
    assert_eq!(
        correlation_of(&resp.commands[0]),
        Some(&uuid_of(CORRELATION))
    );
}

// Each page is its own trigger: a command is checked against the
// correlation of the event it was built from, not the book's first.
#[test]
fn each_command_is_checked_against_its_own_triggers_correlation() {
    let policy = PolicyDispatch::new("OrderToInventory", "order", "inventory").on_event(
        &fq::<OrderPlaced>(),
        |_event, _c| {
            let mut book = reserve(Some(uuid_of(INVENTORY)), 3);
            // The business code wrongly sets the first page's correlation
            // on every command.
            book.page.as_mut().unwrap().correlation_id = Some(uuid_of(CORRELATION));
            Ok(vec![book])
        },
    );
    let err = policy
        .dispatch(&order_source(
            None,
            vec![placed(4, 3, CORRELATION), placed(5, 3, CORRELATION + 2)],
        ))
        .expect_err("the second page's command carries the first's correlation");
    refused_internal(err, codes::POLICY_COMMAND_OTHER_CORRELATION);
}

// --- fan-out (C-0660) --------------------------------------------------------

// C-0660, the router's half: one OrderPlaced feeds both policies; each
// command names its root (inventory 7, fulfillment 0xaa), and neither
// carries another correlation.
#[test]
fn one_event_fans_out_to_an_aggregate_and_a_process_manager() {
    let source = one_placed();
    let inventory = order_to_inventory()
        .dispatch(&source)
        .expect("to the aggregate");
    let fulfillment = order_to_fulfillment().dispatch(&source).expect("to the PM");
    assert_eq!(inventory.commands.len(), 1);
    assert_eq!(fulfillment.commands.len(), 1);
    assert_eq!(
        cover_of(&inventory.commands[0]).root,
        Some(uuid_of(INVENTORY))
    );
    assert_eq!(
        cover_of(&fulfillment.commands[0]).root,
        Some(uuid_of(NAMED))
    );
    keeps_the_triggers_correlation(&inventory.commands[0], CORRELATION);
    keeps_the_triggers_correlation(&fulfillment.commands[0], CORRELATION);
}

// --- the edition: unchanged (POLICY_COMMAND_OUTSIDE_EDITION) ------------------

// A process manager's command stays in its trigger's edition, as an
// aggregate's does.
#[test]
fn a_process_manager_command_stays_in_its_triggers_edition() {
    let resp = order_to_fulfillment()
        .dispatch(&order_source(
            Some("what-if"),
            vec![placed(4, 3, CORRELATION)],
        ))
        .expect("dispatched");
    assert_eq!(
        cover_of(&resp.commands[0]).edition.as_deref(),
        Some("what-if")
    );
    let stray = PolicyDispatch::new("Stray", "order", "fulfillment").on_event(
        &fq::<OrderPlaced>(),
        |_e, _c| {
            let mut book = start(Some(uuid_of(NAMED)), 3);
            book.cover.as_mut().unwrap().edition = Some("elsewhere".to_string());
            Ok(vec![book])
        },
    );
    refused_internal(
        stray
            .dispatch(&order_source(
                Some("what-if"),
                vec![placed(4, 3, CORRELATION)],
            ))
            .expect_err("another edition"),
        codes::POLICY_COMMAND_OUTSIDE_EDITION,
    );
}
