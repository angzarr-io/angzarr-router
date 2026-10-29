//! Who a policy's command addresses, and the correlation it carries
//! (decision 14.36; angzarr-project spec/20261019-draft at ff562b4:
//! policy.proto PolicyResponse, process_manager.proto Stream, types.proto
//! Cover.root; features/coordinator-contract/policy_addressing.feature,
//! C-0656 to C-0660 and C-0665, the router's half of each).
//!
//! The router's half: policy dispatch refuses a command that carries a
//! correlation other than its trigger's; refuses a command to an aggregate
//! that names no root; leaves a process manager command's root UNSET when
//! the policy implements no root callback (filling the default, the
//! trigger's correlation, is the framework coordinator code's, shared from
//! angzarr-core, never the router's); and addresses it to the root the
//! callback derives from the triggering event when the policy implements
//! one (`root_<Command>(event) -> UUID`, opt-in by writing it).

use prost::Message;
use prost_types::Any;

use crate::error::{codes, extras, CodedError, GrpcCode, HandlerError};
use crate::pb;
use crate::policy::PolicyDispatch;
use crate::test_support::*;

/// The trigger's correlation (C-0656's ...0040, as a test UUID).
const CORRELATION: u8 = 0x40;
/// The root a root callback names (C-0658's ...00aa).
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

/// OrderToFulfillment (a process-manager target), naming no root: it
/// implements no root_StartFulfillment, so its business code leaves the
/// cover's root unset.
fn order_to_fulfillment() -> PolicyDispatch {
    PolicyDispatch::to_process_manager("OrderToFulfillment", "order", "fulfillment")
        .on_event(&fq::<OrderPlaced>(), |event, _cover| {
            Ok(vec![start(None, quantity_of(event))])
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

// --- an aggregate target: the policy names the root (C-0656, C-0665) ----------

// C-0656: a command to an aggregate lands on the root it names, with the
// trigger's correlation.
#[test]
fn a_command_to_an_aggregate_keeps_the_root_it_names() {
    let resp = order_to_inventory()
        .dispatch(&one_placed())
        .expect("dispatched");
    assert_eq!(resp.commands.len(), 1);
    assert_eq!(cover_of(&resp.commands[0]).root, Some(uuid_of(INVENTORY)));
    keeps_the_triggers_correlation(&resp.commands[0], CORRELATION);
}

// C-0665: an aggregate command naming no root is a policy bug: refused
// INTERNAL (dead-lettered and alerted), with no commands, and no root is
// filled in. A root whose UUID holds no bytes names none either.
#[test]
fn a_command_to_an_aggregate_naming_no_root_is_refused() {
    for root in [None, Some(pb::Uuid::default())] {
        let policy = PolicyDispatch::new("OrderToInventory", "order", "inventory")
            .on_event(&fq::<OrderPlaced>(), move |_event, _cover| {
                Ok(vec![reserve(root.clone(), 3)])
            });
        let err = refused_internal(
            policy
                .dispatch(&one_placed())
                .expect_err("an aggregate command names its root"),
            codes::POLICY_COMMAND_NO_ROOT,
        );
        let extra = |key: &str| err.extras.get(key).map(String::as_str);
        assert_eq!(extra(extras::DOMAIN), Some("inventory"));
        assert_eq!(
            extra(extras::COMMAND_TYPE),
            Some(fq::<ReserveStock>().as_str())
        );
    }
}

// --- a process-manager target: naming the root is optional (C-0657, C-0658) ---

// C-0657, the router's half: with no root callback the command leaves its
// cover's root UNSET on the wire. The router does not fill the default
// (the trigger's correlation): that is the framework's coordinator code.
// Unset is the default here, not a policy bug.
#[test]
fn a_command_to_a_process_manager_naming_no_root_leaves_it_unset() {
    let resp = order_to_fulfillment()
        .dispatch(&one_placed())
        .expect("an unset root to a PM is the default, not malformed");
    assert_eq!(resp.commands.len(), 1);
    assert_eq!(
        cover_of(&resp.commands[0]).root,
        None,
        "the router never fills the default root"
    );
    keeps_the_triggers_correlation(&resp.commands[0], CORRELATION);
}

// C-0658: a policy implementing root_StartFulfillment names the instance:
// the command lands on that root and keeps the trigger's correlation.
#[test]
fn a_root_callback_names_the_process_manager_instance() {
    let policy =
        order_to_fulfillment().on_root(&fq::<StartFulfillment>(), |_event| Ok(uuid_of(NAMED)));
    let resp = policy.dispatch(&one_placed()).expect("dispatched");
    assert_eq!(resp.commands.len(), 1);
    assert_eq!(cover_of(&resp.commands[0]).root, Some(uuid_of(NAMED)));
    keeps_the_triggers_correlation(&resp.commands[0], CORRELATION);
}

// The callback derives the root from its triggering event alone (a policy
// looks nothing up): each page's command gets the root its own event
// derives.
#[test]
fn a_root_callback_sees_each_triggering_event() {
    let policy = order_to_fulfillment().on_root(&fq::<StartFulfillment>(), |event| {
        Ok(uuid_of(quantity_of(event) as u8))
    });
    let resp = policy
        .dispatch(&order_source(
            None,
            vec![placed(4, 0x11, CORRELATION), placed(5, 0x22, CORRELATION)],
        ))
        .expect("dispatched");
    let roots: Vec<_> = resp
        .commands
        .iter()
        .map(|c| cover_of(c).root.clone())
        .collect();
    assert_eq!(roots, vec![Some(uuid_of(0x11)), Some(uuid_of(0x22))]);
}

// The callback is per command type: a command of a type with none keeps
// its root unset, the default instance.
#[test]
fn a_root_callback_names_only_its_own_commands_root() {
    let policy = PolicyDispatch::to_process_manager("OrderToFulfillment", "order", "fulfillment")
        .on_event(&fq::<OrderPlaced>(), |_event, _cover| {
            Ok(vec![
                start(None, 3),
                command_to(
                    "fulfillment",
                    None,
                    any_of(&RecordStockReserved { quantity: 3 }),
                ),
            ])
        })
        .on_root(&fq::<StartFulfillment>(), |_event| Ok(uuid_of(NAMED)));
    let resp = policy.dispatch(&one_placed()).expect("dispatched");
    assert_eq!(cover_of(&resp.commands[0]).root, Some(uuid_of(NAMED)));
    assert_eq!(cover_of(&resp.commands[1]).root, None);
}

// A failing root callback is a policy bug, like a failing event handler.
#[test]
fn a_failing_root_callback_is_unhandled() {
    let policy = order_to_fulfillment().on_root(&fq::<StartFulfillment>(), |_event| {
        Err(HandlerError::Other("no root".to_string()))
    });
    let err = policy
        .dispatch(&one_placed())
        .expect_err("a failing root callback");
    refused_internal(err, codes::UNHANDLED_HANDLER_ERROR);
}

// --- correlation traces the causal chain --------------------------------------

// Every command a policy sends, to any target, carries its trigger's
// correlation unchanged: a policy cannot change it. One that sets another
// is a policy bug, refused INTERNAL with no commands, whether it goes to an
// aggregate or to a PM (with its root named or not).
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
    let to_default =
        PolicyDispatch::to_process_manager("OrderToFulfillment", "order", "fulfillment")
            .on_event(&fq::<OrderPlaced>(), move |_e, _c| {
                Ok(vec![other(start(None, 3))])
            });
    let to_named = PolicyDispatch::to_process_manager("OrderToFulfillment", "order", "fulfillment")
        .on_event(&fq::<OrderPlaced>(), move |_e, _c| {
            Ok(vec![other(start(None, 3))])
        })
        .on_root(&fq::<StartFulfillment>(), |_event| Ok(uuid_of(NAMED)));
    for policy in [to_inventory, to_default, to_named] {
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

// C-0660, the router's half: one OrderPlaced feeds both policies; the
// aggregate's command names inventory 7, the PM's leaves its root unset
// (the default), and neither carries another correlation.
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
    assert_eq!(cover_of(&fulfillment.commands[0]).root, None);
    keeps_the_triggers_correlation(&inventory.commands[0], CORRELATION);
    keeps_the_triggers_correlation(&fulfillment.commands[0], CORRELATION);
}

// --- the edition: unchanged (POLICY_COMMAND_OUTSIDE_EDITION) ------------------

// A PM command, root named or not, stays in its trigger's edition, as an
// aggregate command does.
#[test]
fn a_process_manager_command_stays_in_its_triggers_edition() {
    let named = order_to_fulfillment().on_root(&fq::<StartFulfillment>(), |_e| Ok(uuid_of(NAMED)));
    for policy in [order_to_fulfillment(), named] {
        let resp = policy
            .dispatch(&order_source(
                Some("what-if"),
                vec![placed(4, 3, CORRELATION)],
            ))
            .expect("dispatched");
        assert_eq!(
            cover_of(&resp.commands[0]).edition.as_deref(),
            Some("what-if")
        );
    }
    let stray = PolicyDispatch::to_process_manager("Stray", "order", "fulfillment").on_event(
        &fq::<OrderPlaced>(),
        |_e, _c| {
            let mut book = start(None, 3);
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
