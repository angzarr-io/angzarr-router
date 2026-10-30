//! Who a policy's command addresses, and the correlation it carries
//! (decision 14.36 as amended by the owner, 2026-10-28: the router does not
//! differentiate process managers; angzarr-project spec/20261019-draft:
//! policy.proto PolicyResponse, process_manager.proto Stream, types.proto
//! Cover.root; features/coordinator-contract/policy_addressing.feature, the
//! router's half of C-0656 to C-0660 and C-0665).
//!
//! The router's half (decision 14.42): a policy names its commands' roots
//! itself, whatever the target (aggregate or process manager alike; there is
//! no root callback and no default root), reading its trigger's metadata to
//! do so, and the router answers its commands as business code returned
//! them. Checking the destination, edition, root and correlation, and
//! stamping the trigger's edition, are the policy's coordinator's.

use prost::Message;
use prost_types::Any;

use crate::pb;
use crate::policy::PolicyDispatch;
use crate::test_support::*;

/// The trigger's correlation (C-0656's ...0040, as a test UUID).
const CORRELATION: u8 = 0x40;
/// The fulfillment instance the policy names (C-0658's ...00aa).
const NAMED: u8 = 0xaa;
/// The inventory aggregate the policy names ("inv-7").
const INVENTORY: u8 = 7;

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
        assert_eq!(commands_of(&resp).len(), 1);
        assert_eq!(cover_of(&commands_of(&resp)[0]).root, Some(uuid_of(root)));
        keeps_the_triggers_correlation(&commands_of(&resp)[0], CORRELATION);
    }
}

// C-0690: a command naming no root, carrying another correlation, naming
// another edition or bound for another domain is answered unchanged, for
// any target; the coordinator refuses it (decision 14.42).
#[test]
fn a_command_is_answered_as_business_code_returned_it_for_any_target() {
    let unchecked = |mut book: pb::CommandBook| {
        let cover = book.cover.as_mut().unwrap();
        cover.root = None;
        cover.domain = "elsewhere".to_string();
        cover.edition = Some("another".to_string());
        book.page.as_mut().unwrap().correlation_id = Some(uuid_of(CORRELATION + 1));
        book
    };
    for (domain, command) in [
        ("inventory", unchecked(reserve(None, 3))),
        ("fulfillment", unchecked(start(None, 3))),
    ] {
        let sent = command.clone();
        let policy = PolicyDispatch::new("OrderTo", "order", domain)
            .on_event(&fq::<OrderPlaced>(), move |_event, _cover| {
                Ok(vec![sent.clone()])
            });
        let resp = policy
            .dispatch(&order_source(
                Some("what-if"),
                vec![placed(4, 3, CORRELATION)],
            ))
            .expect("answered unchecked");
        assert_eq!(commands_of(&resp), vec![command]);
    }
}

// The router stamps nothing: a command naming no edition is answered with
// none, in an edition's trigger too; the coordinator stamps the trigger's.
#[test]
fn a_command_naming_no_edition_is_answered_with_none() {
    let resp = order_to_fulfillment()
        .dispatch(&order_source(
            Some("what-if"),
            vec![placed(4, 3, CORRELATION)],
        ))
        .expect("dispatched");
    assert_eq!(cover_of(&commands_of(&resp)[0]).edition, None);
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
    assert_eq!(commands_of(&inventory).len(), 1);
    assert_eq!(commands_of(&fulfillment).len(), 1);
    assert_eq!(
        cover_of(&commands_of(&inventory)[0]).root,
        Some(uuid_of(INVENTORY))
    );
    assert_eq!(
        cover_of(&commands_of(&fulfillment)[0]).root,
        Some(uuid_of(NAMED))
    );
    keeps_the_triggers_correlation(&commands_of(&inventory)[0], CORRELATION);
    keeps_the_triggers_correlation(&commands_of(&fulfillment)[0], CORRELATION);
}

// --- the trigger's metadata: what a policy may name as the root --------------

// Decision 14.36: a policy names the root itself, and for a process manager
// it may name its trigger's correlation. So the policy reads the triggering
// page's metadata: its correlation and when it was recorded.
#[test]
fn a_policy_reads_its_triggers_correlation_and_recorded_at() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = seen.clone();
    let policy = PolicyDispatch::new("ByCorrelation", "order", "fulfillment")
        .on_event_with_context(&fq::<OrderPlaced>(), move |event, ctx| {
            let meta = ctx.meta.expect("the trigger's metadata");
            sink.lock().unwrap().push(meta.recorded_at);
            Ok(vec![start(meta.correlation_id.clone(), quantity_of(event))])
        });
    let mut trigger = placed(4, 3, CORRELATION);
    let recorded_at = prost_types::Timestamp {
        seconds: 1_700_000_000,
        nanos: 5,
    };
    trigger.meta.as_mut().unwrap().recorded_at = Some(recorded_at);
    let resp = policy
        .dispatch(&order_source(None, vec![trigger]))
        .expect("dispatched");
    assert_eq!(
        cover_of(&commands_of(&resp)[0]).root,
        Some(uuid_of(CORRELATION)),
        "the root the policy named: its trigger's correlation"
    );
    assert_eq!(*seen.lock().unwrap(), vec![Some(recorded_at)]);
}
