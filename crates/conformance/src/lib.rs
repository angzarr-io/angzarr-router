//! Conformance harness support: the generated fixture types, a descriptor
//! pool for parsing `.txtpb` fixtures, the CounterAggregate built on the
//! router core, and the "parse a skeleton, set the scenario's data" helpers
//! the step definitions call. The cucumber step defs live in
//! `tests/cucumber.rs`.

use std::sync::{Arc, Mutex, OnceLock};

use angzarr_router::domain::{DomainDispatch, FactRecord};
use angzarr_router::error::{CodedError, HandlerError};
use angzarr_router::policy::PolicyDispatch;
use angzarr_router::projector::ProjectorDispatch;
use angzarr_router::rebuild::Rebuilder;
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage};

/// Generated `test.counter` fixture messages (Increased, IncreaseBy, …).
pub mod counter {
    include!(concat!(env!("OUT_DIR"), "/test.counter.rs"));
}

/// Generated `orders.v1` fixture messages: the handler-contract tier's
/// order vocabulary.
pub mod orders {
    pub mod v1 {
        include!(concat!(env!("OUT_DIR"), "/orders.v1.rs"));
    }
}

/// Re-export the router's framework types under `pb`.
pub use angzarr_router::pb;
pub use counter::CounterState;

const FILE_DESCRIPTOR_SET: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/conformance_fds.bin"));

// The orthogonal envelope skeletons — structural scaffold only; the
// test-meaningful field is omitted and supplied by the step definitions.
const SKEL_INCREASE: &str = include_str!("../../../conformance/fixtures/command_increase.txtpb");
const SKEL_FAILHARD: &str = include_str!("../../../conformance/fixtures/command_failhard.txtpb");
const SKEL_UNHANDLED: &str = include_str!("../../../conformance/fixtures/command_unhandled.txtpb");
const SKEL_INCREASED_EVENT: &str =
    include_str!("../../../conformance/fixtures/event_increased.txtpb");

/// The CounterAggregate's business rejection code (`IncreaseBy { n: 0 }`).
/// A fixture code, not a router code.
pub const VALUE_NOT_POSITIVE: &str = "VALUE_NOT_POSITIVE";

const CONTEXTUAL_COMMAND: &str = "io.angzarr.v1.ContextualCommand";
const EVENT_PAGE: &str = "io.angzarr.v1.EventPage";

/// What the IncreaseBy handler observed at dispatch time: the historical-state
/// evidence the framework supplies (`next_sequence`, `had_prior_events`) and
/// the rebuilt `count`. Recorded into a harness-owned sink, since host state
/// never crosses the boundary — this is how scenarios assert that evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observed {
    pub next_sequence: u64,
    pub had_prior_events: bool,
    pub count: u32,
}

/// Sink the harness passes into the fixture to capture each [`Observed`].
pub type ObservedSink = Arc<Mutex<Vec<Observed>>>;

/// The descriptor pool over the fixture + framework protos — resolves both
/// `io.angzarr.v1.*` envelopes and `test.counter.*` payloads, so Any-expanded
/// textproto fixtures decode.
pub fn pool() -> &'static DescriptorPool {
    static POOL: OnceLock<DescriptorPool> = OnceLock::new();
    POOL.get_or_init(|| {
        DescriptorPool::decode(FILE_DESCRIPTOR_SET).expect("conformance descriptor set must decode")
    })
}

/// Parse a `.txtpb` fixture (textproto, Any-expansion allowed) into a typed
/// prost message. `full_name` is the fixture's root message type.
pub fn parse_txtpb<M>(full_name: &str, text: &str) -> M
where
    M: prost::Message + Default,
{
    let descriptor = pool()
        .get_message_by_name(full_name)
        .unwrap_or_else(|| panic!("{full_name} not in conformance pool"));
    let dynamic = DynamicMessage::parse_text_format(descriptor, text)
        .unwrap_or_else(|e| panic!("parse {full_name} txtpb: {e}"));
    dynamic
        .transcode_to::<M>()
        .unwrap_or_else(|e| panic!("transcode {full_name}: {e}"))
}

// ---------------------------------------------------------------------------
// The fixture: CounterAggregate on the router core (see FIXTURE.md).
// ---------------------------------------------------------------------------

/// Build the CounterAggregate dispatch table (appliers and handlers).
pub fn counter_aggregate(observed: ObservedSink) -> DomainDispatch<CounterState> {
    let rebuilder = Rebuilder::of_message()
        .apply(
            "test.counter.Increased",
            |state: &mut CounterState, event| {
                // Decode the payload so a corrupt persisted event fails the fold
                // (UNHANDLED_HANDLER_ERROR: a failing fold is a handler bug).
                // Increased is empty, so every
                // well-formed event decodes and simply increments.
                counter::Increased::decode(event.value.as_slice())?;
                state.count += 1;
                Ok(())
            },
        )
        .with_snapshot(|state: &mut CounterState, snapshot| {
            // Seed state from the snapshot; pages at or below its sequence are
            // already folded in and must not re-apply (covered-page boundary).
            state.count = counter::CounterState::decode(snapshot.value.as_slice())?.count;
            Ok(())
        });

    DomainDispatch::new("counter-aggregate", "counter", rebuilder)
        // n > 0 emits n Increased; n == 0 rejects VALUE_NOT_POSITIVE.
        .on_command("test.counter.IncreaseBy", move |any, state, ctx| {
            // Record the historical-state evidence (host state never crosses).
            observed.lock().unwrap().push(Observed {
                next_sequence: ctx.next_sequence,
                had_prior_events: ctx.had_prior_events,
                count: state.count,
            });
            let cmd = counter::IncreaseBy::decode(any.value.as_slice())
                .map_err(|e| HandlerError::Other(format!("decode IncreaseBy: {e}")))?;
            if cmd.n == 0 {
                return Err(HandlerError::Coded(CodedError::rejection_invalid_argument(
                    VALUE_NOT_POSITIVE,
                    "increase amount must be positive",
                    [],
                )));
            }
            let pages = (0..cmd.n)
                .map(|_| pb::EventPage {
                    payload: Some(increased_any()),
                    ..Default::default()
                })
                .collect();
            Ok(Some(pb::EventBook {
                pages,
                ..Default::default()
            }))
        })
        // Unclassified failure → UNHANDLED_HANDLER_ERROR.
        .on_command("test.counter.FailHard", |_any, _state, _ctx| {
            Err(HandlerError::Other("hard failure".to_string()))
        })
}

fn increased_any() -> prost_types::Any {
    prost_types::Any {
        type_url: angzarr_router::type_url("test.counter.Increased"),
        value: counter::Increased {}.encode_to_vec(),
    }
}

// ---------------------------------------------------------------------------
// The fixture: CounterProjector on the router core (read-side dispatch).
// ---------------------------------------------------------------------------

/// The projection the CounterProjector folds events into. Host state — it
/// never crosses the boundary; the harness reads the fold count back out of
/// the finished Projection.
#[derive(Default)]
pub struct ProjectorState {
    pub count: u32,
}

/// Build the CounterProjector dispatch table: over `domains` (`"*"` is every
/// domain) it folds each Increased event into a running count, then finishes
/// into a Projection whose sequence carries that count and whose payload is
/// the CounterState. A book from any other domain folds nothing (C-0032).
pub fn counter_projector(domains: &[&str]) -> ProjectorDispatch<ProjectorState> {
    ProjectorDispatch::new("counter-projector", ProjectorState::default)
        .for_domains(domains.iter().copied())
        .on_event(
            "test.counter.Increased",
            |state: &mut ProjectorState, event, _ctx| {
                // Decode so a corrupt event fails the fold, exactly as the
                // aggregate applier does. Increased is empty — every
                // well-formed event decodes and increments.
                counter::Increased::decode(event.value.as_slice())
                    .map_err(|e| HandlerError::Other(format!("decode Increased: {e}")))?;
                state.count += 1;
                Ok(())
            },
        )
        .finish(|state: &mut ProjectorState, events| {
            Ok(pb::Projection {
                cover: events.cover.clone(),
                projector: "counter-projector".to_string(),
                next_sequence: u64::from(state.count),
                projection: Some(prost_types::Any {
                    type_url: angzarr_router::type_url("test.counter.CounterState"),
                    value: CounterState { count: state.count }.encode_to_vec(),
                }),
            })
        })
}

/// An EventBook of `n` Increased events whose cover carries `domain` — the
/// projector's delivery.
pub fn delivery(domain: &str, n: u32) -> pb::EventBook {
    pb::EventBook {
        cover: Some(pb::Cover {
            domain: domain.to_string(),
            ..Default::default()
        }),
        pages: (0..n)
            .map(|_| pb::EventPage {
                payload: Some(increased_any()),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

/// A delivery with no cover → drives MISSING_EVENT_BOOK_COVER.
pub fn delivery_without_cover(n: u32) -> pb::EventBook {
    let mut book = delivery("counter", n);
    book.cover = None;
    book
}

// ---------------------------------------------------------------------------
// The fixture: OrderPolicy on the router core (translation-side dispatch).
// ---------------------------------------------------------------------------

/// Build the OrderPolicy dispatch table: it translates each Increased source
/// event into one Reserve command for "inventory" (passed through unstamped),
/// recording each triggering event's sequence in `seen`. Undeclared events
/// are skipped.
pub fn order_policy(seen: SequenceSink) -> PolicyDispatch {
    PolicyDispatch::new("order-policy", "order", "inventory").on_event_with_context(
        "test.counter.Increased",
        move |_any, source| {
            seen.lock().unwrap().push(source.sequence);
            Ok(vec![reserve_command_to("inventory")])
        },
    )
}

fn reserve_command_to(domain: &str) -> pb::CommandBook {
    pb::CommandBook {
        cover: Some(pb::Cover {
            domain: domain.to_string(),
            ..Default::default()
        }),
        page: Some(pb::CommandPage {
            body: Some(pb::command_page::Body::Command(pb::Command {
                payload: Some(prost_types::Any {
                    type_url: angzarr_router::type_url("test.counter.Reserve"),
                    value: Vec::new(),
                }),
            })),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A policy source carrying one event of `event_fq` in the "order" domain at
/// sequence `seq`.
pub fn policy_event_source(event_fq: &str, seq: u64) -> pb::EventBook {
    pb::EventBook {
        cover: Some(pb::Cover {
            domain: "order".to_string(),
            ..Default::default()
        }),
        pages: vec![pb::EventPage {
            payload: Some(prost_types::Any {
                type_url: angzarr_router::type_url(event_fq),
                value: Vec::new(),
            }),
            sequence: seq,
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// A policy source carrying no pages → EMPTY_POLICY_SOURCE.
pub fn policy_empty_source() -> pb::EventBook {
    pb::EventBook::default()
}

// ---------------------------------------------------------------------------
// Skeleton → command helpers: parse the orthogonal envelope, then SET the
// scenario's data by field (never string-templating the textproto).
// ---------------------------------------------------------------------------

/// An IncreaseBy command with the scenario's `n` set on the inner message.
pub fn increase_command(n: u32) -> pb::ContextualCommand {
    let mut cc: pb::ContextualCommand = parse_txtpb(CONTEXTUAL_COMMAND, SKEL_INCREASE);
    let any = inner_command_any(&mut cc);
    let mut inner =
        counter::IncreaseBy::decode(any.value.as_slice()).expect("decode IncreaseBy skeleton");
    inner.n = n;
    any.value = inner.encode_to_vec();
    cc
}

/// The FailHard command (no scenario data).
pub fn failhard_command() -> pb::ContextualCommand {
    parse_txtpb(CONTEXTUAL_COMMAND, SKEL_FAILHARD)
}

/// A command whose type has no registered handler (drives NO_HANDLER_REGISTERED).
pub fn unhandled_command() -> pb::ContextualCommand {
    parse_txtpb(CONTEXTUAL_COMMAND, SKEL_UNHANDLED)
}

// Envelope-guard negatives: a well-formed skeleton with exactly one structural
// field cleared, so the guard fires regardless of the rest being valid.

/// No command book at all → MISSING_COMMAND_BOOK.
pub fn command_missing_book() -> pb::ContextualCommand {
    let mut cc = increase_command(1);
    cc.command = None;
    cc
}

/// A command book with no page → MISSING_COMMAND_PAGE.
pub fn command_missing_page() -> pb::ContextualCommand {
    let mut cc = increase_command(1);
    cc.command.as_mut().expect("command book").page = None;
    cc
}

/// A command page carrying no body → MISSING_COMMAND_PAYLOAD.
pub fn command_missing_payload() -> pb::ContextualCommand {
    let mut cc = increase_command(1);
    cc.command
        .as_mut()
        .expect("command book")
        .page
        .as_mut()
        .expect("command page")
        .body = None;
    cc
}

/// Prior history of `n` confirmed increases: the Increased skeleton replayed
/// at consecutive sequences, with `next_sequence` continuing past them.
pub fn prior_history(n: u32) -> Option<pb::EventBook> {
    if n == 0 {
        return None;
    }
    let page: pb::EventPage = parse_txtpb(EVENT_PAGE, SKEL_INCREASED_EVENT);
    let pages = (0..u64::from(n))
        .map(|seq| pb::EventPage {
            sequence: seq,
            ..page.clone()
        })
        .collect();
    Some(pb::EventBook {
        pages,
        next_sequence: u64::from(n),
        ..Default::default()
    })
}

/// Prior history whose single Increased event carries undecodable payload
/// bytes (a truncated varint) → the applier fails the fold, surfacing
/// UNHANDLED_HANDLER_ERROR (a handler bug) when a known command rebuilds
/// over it.
pub fn corrupt_prior_history() -> Option<pb::EventBook> {
    let mut page: pb::EventPage = parse_txtpb(EVENT_PAGE, SKEL_INCREASED_EVENT);
    if let Some(any) = page.payload.as_mut() {
        any.value = vec![0xff, 0xff, 0xff];
    }
    Some(pb::EventBook {
        pages: vec![page],
        next_sequence: 1,
        ..Default::default()
    })
}

/// Prior history seeded by a snapshot of `count == 10` at sequence 10, plus a
/// covered page (sequence 10, already in the snapshot → skipped) and one
/// uncovered page (sequence 11 → applied). A rebuild therefore observes
/// `count == 11`: snapshot loaded, covered page not refolded, newer page applied.
pub fn snapshot_history() -> Option<pb::EventBook> {
    let increased_at = |seq: u64| pb::EventPage {
        sequence: seq,
        ..parse_txtpb(EVENT_PAGE, SKEL_INCREASED_EVENT)
    };
    Some(pb::EventBook {
        snapshot: Some(pb::Snapshot {
            state: Some(prost_types::Any {
                type_url: angzarr_router::type_url("test.counter.CounterState"),
                value: CounterState { count: 10 }.encode_to_vec(),
            }),
            sequence: 10,
            ..Default::default()
        }),
        pages: vec![increased_at(10), increased_at(11)],
        next_sequence: 12,
        ..Default::default()
    })
}

fn inner_command_any(cc: &mut pb::ContextualCommand) -> &mut prost_types::Any {
    let book = cc.command.as_mut().expect("command book");
    match book.page.as_mut().and_then(|p| p.body.as_mut()) {
        Some(pb::command_page::Body::Command(pb::Command { payload: Some(any) })) => any,
        other => panic!("conformance fixtures carry an inline command, got {other:?}"),
    }
}

/// Rewrites every Any type URL in `cmd` (the command) and its prior history
/// (the events) to `prefix` + the fully-qualified name.
pub fn with_type_url_prefix(mut cmd: pb::ContextualCommand, prefix: &str) -> pb::ContextualCommand {
    let rewrite = |any: &mut prost_types::Any| {
        any.type_url = format!(
            "{prefix}{}",
            angzarr_router::type_name_from_url(&any.type_url)
        );
    };
    if let Some(Some(pb::command_page::Body::Command(pb::Command { payload: Some(any) }))) = cmd
        .command
        .as_mut()
        .and_then(|book| book.page.as_mut())
        .map(|page| page.body.as_mut())
    {
        rewrite(any);
    }
    if let Some(book) = cmd.events.as_mut() {
        for page in &mut book.pages {
            if let Some(any) = page.payload.as_mut() {
                rewrite(any);
            }
        }
    }
    cmd
}

// ---------------------------------------------------------------------------
// Context fixtures: facts, cover access, projector page context
// (context.feature).
// ---------------------------------------------------------------------------

/// The root bytes for a label: UUID v5 in the OID namespace.
pub fn root_of(label: &str) -> Vec<u8> {
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, label.as_bytes())
        .as_bytes()
        .to_vec()
}

/// A cover in `domain` with the root for `label`.
pub fn cover_of(domain: &str, label: &str) -> pb::Cover {
    pb::Cover {
        domain: domain.to_string(),
        root: Some(pb::Uuid {
            value: root_of(label).into(),
        }),
        ..Default::default()
    }
}

/// Covers a handler observed.
pub type CoverSink = Arc<Mutex<Vec<Option<pb::Cover>>>>;

/// Page sequences an applier observed.
pub type SequenceSink = Arc<Mutex<Vec<u64>>>;

fn increased_page(seq: u64) -> pb::EventPage {
    pb::EventPage {
        payload: Some(increased_any()),
        sequence: seq,
        ..Default::default()
    }
}

/// The ledger aggregate (domain "ledger") over CounterState: Increased folds
/// count += 1 and records the page sequence it applied; a snapshot loads
/// CounterState; IncreaseBy records the handled cover and emits one
/// Increased; the only fact, Increased, is recorded as received and flagged
/// by a CounterState carrying the count it brings the ledger to.
pub fn ledger_aggregate(seen: CoverSink, applied: SequenceSink) -> DomainDispatch<CounterState> {
    let rebuilder = Rebuilder::of_message()
        .apply_with_context(
            "test.counter.Increased",
            move |state: &mut CounterState, _, ctx| {
                state.count += 1;
                applied.lock().unwrap().push(ctx.sequence);
                Ok(())
            },
        )
        .with_snapshot(|state: &mut CounterState, any| {
            *state = CounterState::decode(any.value.as_slice())?;
            Ok(())
        });
    DomainDispatch::new("Ledger", "ledger", rebuilder)
        .on_command("test.counter.IncreaseBy", move |_cmd, _state, cctx| {
            seen.lock().unwrap().push(cctx.cover.clone());
            Ok(Some(pb::EventBook {
                pages: vec![pb::EventPage {
                    payload: Some(increased_any()),
                    ..Default::default()
                }],
                ..Default::default()
            }))
        })
        .on_fact("test.counter.Increased", |fact, state: &CounterState| {
            Ok(FactRecord {
                fact: fact.clone(),
                flags: vec![prost_types::Any {
                    type_url: angzarr_router::type_url("test.counter.CounterState"),
                    value: CounterState {
                        count: state.count + 1,
                    }
                    .encode_to_vec(),
                }],
            })
        })
}

/// An IncreaseBy command for the ledger root `label`.
pub fn ledger_command(label: &str) -> pb::ContextualCommand {
    pb::ContextualCommand {
        command: Some(pb::CommandBook {
            cover: Some(cover_of("ledger", label)),
            page: Some(pb::CommandPage {
                body: Some(pb::command_page::Body::Command(pb::Command {
                    payload: Some(prost_types::Any {
                        type_url: angzarr_router::type_url("test.counter.IncreaseBy"),
                        value: counter::IncreaseBy { n: 1 }.encode_to_vec(),
                    }),
                })),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// [`ledger_command`] over prior Increased events at `sequences`.
pub fn ledger_command_over(label: &str, sequences: &[u64]) -> pb::ContextualCommand {
    let mut cc = ledger_command(label);
    cc.events = Some(pb::EventBook {
        pages: sequences.iter().map(|s| increased_page(*s)).collect(),
        next_sequence: sequences.last().map_or(0, |s| s + 1),
        ..Default::default()
    });
    cc
}

/// A `test.counter.<fact>` fact (empty payload) for the ledger, delivered in
/// the Fact arm over `prior` Increased events.
pub fn ledger_fact(fact: &str, prior: u64) -> pb::ContextualCommand {
    let mut cc = ledger_command("ledger-1");
    cc.command.as_mut().expect("command book").page = Some(pb::CommandPage {
        body: Some(pb::command_page::Body::Fact(pb::Fact {
            payload: Some(prost_types::Any {
                type_url: angzarr_router::type_url(&format!("test.counter.{fact}")),
                value: Vec::new(),
            }),
        })),
        ..Default::default()
    });
    if prior > 0 {
        cc.events = Some(pb::EventBook {
            pages: (0..prior).map(increased_page).collect(),
            next_sequence: prior,
            ..Default::default()
        });
    }
    cc
}

/// (root bytes, sequence) pairs a projector fold observed.
pub type PageSink = Arc<Mutex<Vec<(Vec<u8>, u64)>>>;

/// The tracking projector: every Increased fold records its book's root and
/// the page's sequence.
pub fn tracking_projector(seen: PageSink) -> ProjectorDispatch<()> {
    ProjectorDispatch::new("Tracker", || ()).on_event(
        "test.counter.Increased",
        move |_p, _e, ctx| {
            let root = ctx
                .cover
                .and_then(|c| c.root.as_ref())
                .map(|r| r.value.to_vec())
                .unwrap_or_default();
            seen.lock().unwrap().push((root, ctx.sequence));
            Ok(())
        },
    )
}

/// A book of Increased events of "counter" root `label` at `sequences`.
pub fn tracked_book(label: &str, sequences: &[u64]) -> pb::EventBook {
    pb::EventBook {
        cover: Some(cover_of("counter", label)),
        pages: sequences.iter().map(|s| increased_page(*s)).collect(),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// The order aggregate and the Fulfillment process manager: the
// handler-contract tier (angzarr-project
// features/handler-contract/handling.feature, C-0533..C-0550).
// ---------------------------------------------------------------------------

/// How the order aggregate's AddItem handler settles a conflict window under
/// AGGREGATE_HANDLES.
#[derive(Debug, Clone, Default)]
pub enum Settle {
    /// resolve_conflicts with the nonce it was given.
    #[default]
    Delivered,
    /// Never call resolve_conflicts.
    Never,
    /// resolve_conflicts with this value instead.
    With(Vec<u8>),
}

/// What business code saw when it ran.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderObserved {
    /// The fully-qualified type business code received (after upcasting).
    pub handled: String,
    pub item: String,
    pub expected_sequence: u64,
    pub conflict_resolution: pb::ConflictResolution,
    /// The conflict window's sequences, when business code got a conflict.
    pub window: Option<Vec<u64>>,
    /// The nonce, when business code got a conflict.
    pub nonce: Option<Vec<u8>>,
    /// The item keys of the state it decided on, sorted.
    pub items: Vec<String>,
}

/// Sink the harness passes into the order fixtures.
pub type OrderSink = Arc<Mutex<Vec<OrderObserved>>>;

/// The generated `changed(before, after)` for `OrderState` (backlog CR-1):
/// `items[<item>]` per item, `shipping_address`, and the wildcard `*notes`
/// for the unmarked notes.
pub fn order_changed(
    before: &orders::v1::OrderState,
    after: &orders::v1::OrderState,
) -> Vec<String> {
    let keys: std::collections::BTreeSet<&String> =
        before.items.keys().chain(after.items.keys()).collect();
    let mut units: Vec<String> = keys
        .into_iter()
        .filter(|k| before.items.get(*k) != after.items.get(*k))
        .map(|k| format!("items[{k}]"))
        .collect();
    if before.shipping_address != after.shipping_address {
        units.push("shipping_address".to_string());
    }
    if before.notes != after.notes {
        units.push("*notes".to_string());
    }
    units
}

fn orders_any<M: prost::Message + prost::Name>(msg: &M) -> prost_types::Any {
    prost_types::Any {
        type_url: angzarr_router::type_url(&M::full_name()),
        value: msg.encode_to_vec(),
    }
}

fn rename(
    to: &'static str,
) -> impl Fn(&prost_types::Any) -> Result<prost_types::Any, HandlerError> {
    move |any| {
        Ok(prost_types::Any {
            type_url: angzarr_router::type_url(to),
            value: any.value.clone(),
        })
    }
}

fn sorted_items(state: &orders::v1::OrderState) -> Vec<String> {
    let mut items: Vec<String> = state.items.keys().cloned().collect();
    items.sort();
    items
}

/// The order aggregate (domain "order") at `rules_version`: ItemAdded bumps
/// an item, OrderNoteAdded sets the notes, a snapshot loads OrderState;
/// AddItem records what business code saw in `seen`, settles per `settle`
/// and records ItemAdded; the fact ItemDelivered is recorded as received;
/// the upcaster chain renames AddItemV1 to AddItem, ItemDeliveredV1 to
/// ItemDelivered and AddLine to AddLineItem (which nothing handles);
/// COMMUTATIVE runs through [`order_changed`].
pub fn order_aggregate(
    rules_version: &str,
    settle: Settle,
    seen: OrderSink,
) -> DomainDispatch<orders::v1::OrderState> {
    let rebuilder = Rebuilder::of_message()
        .apply(
            "orders.v1.ItemAdded",
            |state: &mut orders::v1::OrderState, event| {
                let item = orders::v1::ItemAdded::decode(event.value.as_slice())?.item;
                *state.items.entry(item).or_default() += 1;
                Ok(())
            },
        )
        .apply(
            "orders.v1.ItemDelivered",
            |state: &mut orders::v1::OrderState, event| {
                let item = orders::v1::ItemDelivered::decode(event.value.as_slice())?.item;
                state.delivered.push(item);
                Ok(())
            },
        )
        .apply(
            "orders.v1.OrderNoteAdded",
            |state: &mut orders::v1::OrderState, event| {
                state.notes = orders::v1::OrderNoteAdded::decode(event.value.as_slice())?.note;
                Ok(())
            },
        )
        .with_snapshot(|state: &mut orders::v1::OrderState, any| {
            *state = orders::v1::OrderState::decode(any.value.as_slice())?;
            Ok(())
        });
    let upcaster = angzarr_router::upcaster::UpcasterDispatch::new("order-payloads", "order")
        .on_event("orders.v1.AddItemV1", rename("orders.v1.AddItem"))
        .on_event(
            "orders.v1.ItemDeliveredV1",
            rename("orders.v1.ItemDelivered"),
        )
        .on_event("orders.v1.AddLine", rename("orders.v1.AddLineItem"));
    let facts = seen.clone();
    DomainDispatch::new("order", "order", rebuilder)
        .with_rules_version(rules_version)
        .with_commutative(order_changed)
        .with_command_upcaster(upcaster)
        .on_command("orders.v1.AddItem", move |cmd, state, cctx| {
            let item = orders::v1::AddItem::decode(cmd.value.as_slice())
                .map_err(|e| HandlerError::Other(format!("decode AddItem: {e}")))?
                .item;
            seen.lock().unwrap().push(OrderObserved {
                handled: angzarr_router::type_name_from_url(&cmd.type_url).to_string(),
                item: item.clone(),
                expected_sequence: cctx.expected_sequence,
                conflict_resolution: cctx.conflict_resolution,
                window: cctx
                    .conflict
                    .as_ref()
                    .map(|c| c.window().iter().map(|p| p.sequence).collect()),
                nonce: cctx.conflict.as_ref().map(|c| c.nonce().to_vec()),
                items: sorted_items(state),
            });
            if let Some(conflict) = cctx.conflict.as_ref() {
                match &settle {
                    Settle::Delivered => {
                        conflict.resolve_conflicts(conflict.window(), conflict.nonce())
                    }
                    Settle::Never => {}
                    Settle::With(value) => conflict.resolve_conflicts(conflict.window(), value),
                }
            }
            Ok(Some(pb::EventBook {
                pages: vec![pb::EventPage {
                    payload: Some(orders_any(&orders::v1::ItemAdded { item })),
                    ..Default::default()
                }],
                ..Default::default()
            }))
        })
        // Acknowledge: business code answers with no events.
        .on_command("orders.v1.Acknowledge", |_cmd, _state, _cctx| Ok(None))
        .on_fact("orders.v1.ItemDelivered", move |fact, state| {
            let item = orders::v1::ItemDelivered::decode(fact.value.as_slice())
                .map_err(|e| HandlerError::Other(format!("decode ItemDelivered: {e}")))?
                .item;
            facts.lock().unwrap().push(OrderObserved {
                handled: angzarr_router::type_name_from_url(&fact.type_url).to_string(),
                item,
                expected_sequence: 0,
                conflict_resolution: pb::ConflictResolution::Unspecified,
                window: None,
                nonce: None,
                items: sorted_items(state),
            });
            Ok(FactRecord::from(fact.clone()))
        })
}

fn order_event(kind: &str, value: &str) -> prost_types::Any {
    match kind {
        "ItemAdded" => orders_any(&orders::v1::ItemAdded {
            item: value.to_string(),
        }),
        "OrderNoteAdded" => orders_any(&orders::v1::OrderNoteAdded {
            note: value.to_string(),
        }),
        other => panic!("no order event {other}"),
    }
}

/// An `orders.v1.<command>` command for `item` to the order aggregate, over
/// `history` ((event kind, value) at sequences 0..) and an optional delivered
/// snapshot, resolved to `(expected_sequence, resolution, nonce)` when given.
pub fn order_command(
    command: &str,
    item: &str,
    history: &[(String, String)],
    handling: Option<(u64, pb::ConflictResolution, Vec<u8>)>,
    snapshot: Option<pb::Snapshot>,
) -> pb::ContextualCommand {
    let (handling, nonce) = match handling {
        Some((expected, resolution, nonce)) => (
            Some(pb::Handling {
                expected_sequence: Some(expected),
                conflict_resolution: resolution as i32,
            }),
            nonce,
        ),
        None => (None, Vec::new()),
    };
    pb::ContextualCommand {
        command: Some(pb::CommandBook {
            cover: Some(pb::Cover {
                domain: "order".to_string(),
                ..Default::default()
            }),
            page: Some(pb::CommandPage {
                body: Some(pb::command_page::Body::Command(pb::Command {
                    payload: Some(prost_types::Any {
                        type_url: angzarr_router::type_url(&format!("orders.v1.{command}")),
                        // Every orders.v1 command carries its item in field 1.
                        value: orders::v1::AddItem {
                            item: item.to_string(),
                        }
                        .encode_to_vec(),
                    }),
                })),
                ..Default::default()
            }),
            handling,
        }),
        events: Some(pb::EventBook {
            snapshot,
            pages: history
                .iter()
                .zip(0u64..)
                .map(|((kind, value), sequence)| pb::EventPage {
                    payload: Some(order_event(kind, value)),
                    sequence,
                    ..Default::default()
                })
                .collect(),
            next_sequence: history.len() as u64,
            ..Default::default()
        }),
        conflict_nonce: nonce.into(),
    }
}

/// An `orders.v1.<fact>` fact for `item`, delivered in the Fact arm.
pub fn order_fact(fact: &str, item: &str) -> pb::ContextualCommand {
    let mut cc = order_command("AddItem", item, &[], None, None);
    let page = cc
        .command
        .as_mut()
        .and_then(|b| b.page.as_mut())
        .expect("command page");
    page.body = Some(pb::command_page::Body::Fact(pb::Fact {
        payload: Some(prost_types::Any {
            type_url: angzarr_router::type_url(&format!("orders.v1.{fact}")),
            value: orders::v1::ItemDelivered {
                item: item.to_string(),
            }
            .encode_to_vec(),
        }),
    }));
    cc
}

/// A delivered snapshot of OrderState holding `items`, recorded under
/// `rules_version` (which the router never judges: decision 14.1).
pub fn order_snapshot(items: &[&str], rules_version: &str) -> pb::Snapshot {
    pb::Snapshot {
        sequence: 0,
        state: Some(orders_any(&orders::v1::OrderState {
            items: items.iter().map(|i| (i.to_string(), 1)).collect(),
            ..Default::default()
        })),
        rules_version: rules_version.to_string(),
    }
}

/// The Fulfillment process manager at `rules_version`: a domain (decision
/// 14.31) whose StartFulfillment command, its starting input, records
/// nothing.
pub fn fulfillment_pm(rules_version: &str) -> DomainDispatch<orders::v1::FulfillmentState> {
    fulfillment_pm_writing(rules_version, false)
}

/// [`fulfillment_pm`] that, when `writes`, answers StartFulfillment with one
/// FulfillmentStarted event, which counts into its state's `placed`.
pub fn fulfillment_pm_writing(
    rules_version: &str,
    writes: bool,
) -> DomainDispatch<orders::v1::FulfillmentState> {
    let rebuilder = Rebuilder::of_message().apply(
        "orders.v1.FulfillmentStarted",
        |state: &mut orders::v1::FulfillmentState, _event| {
            state.placed += 1;
            Ok(())
        },
    );
    DomainDispatch::process_manager("Fulfillment", "fulfillment", rebuilder)
        .with_rules_version(rules_version)
        .on_command("orders.v1.StartFulfillment", move |_c, _s, _ctx| {
            Ok(writes.then(|| pb::EventBook {
                pages: vec![pb::EventPage {
                    payload: Some(orders_any(&orders::v1::FulfillmentStarted {})),
                    ..Default::default()
                }],
                ..Default::default()
            }))
        })
        .starts("orders.v1.StartFulfillment")
}

/// A StartFulfillment command for the Fulfillment instance of correlation
/// "order-1" (its root; decision 14.30), over no prior events.
pub fn start_fulfillment() -> pb::ContextualCommand {
    let mut book = order_command_for("fulfillment", "StartFulfillment");
    book.cover.as_mut().expect("a cover").root = Some(pb::Uuid {
        value: root_of("order-1").into(),
    });
    pb::ContextualCommand {
        command: Some(book),
        ..Default::default()
    }
}

/// The Fulfillment process manager's state with `placed`, as its process
/// state image.
pub fn fulfillment_state(placed: u32) -> prost_types::Any {
    orders_any(&orders::v1::FulfillmentState { placed })
}

/// An order state image read back: its type URL, its items (sorted) with
/// their quantities, and its delivered items in order. (Map encoding order
/// is unspecified, so images compare decoded.)
pub fn read_order_state(image: &prost_types::Any) -> (String, Vec<(String, u32)>, Vec<String>) {
    let state = orders::v1::OrderState::decode(image.value.as_slice()).expect("OrderState");
    let mut items: Vec<(String, u32)> = state.items.into_iter().collect();
    items.sort();
    (image.type_url.clone(), items, state.delivered)
}

/// An `orders.v1.<command>` command book for `domain` (empty payload).
pub fn order_command_for(domain: &str, command: &str) -> pb::CommandBook {
    pb::CommandBook {
        cover: Some(pb::Cover {
            domain: domain.to_string(),
            ..Default::default()
        }),
        page: Some(pb::CommandPage {
            body: Some(pb::command_page::Body::Command(pb::Command {
                payload: Some(prost_types::Any {
                    type_url: angzarr_router::type_url(&format!("orders.v1.{command}")),
                    value: Vec::new(),
                }),
            })),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A policy source: one OrderPlaced event from "order".
pub fn order_placed_source() -> pb::EventBook {
    pb::EventBook {
        cover: Some(pb::Cover {
            domain: "order".to_string(),
            ..Default::default()
        }),
        pages: vec![pb::EventPage {
            payload: Some(orders_any(&orders::v1::OrderPlaced {})),
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[cfg(test)]
mod smoke {
    use super::*;

    /// The whole pipeline works: compile counter.proto, extern the framework
    /// types to the router crate, build the pool, and parse an Any-expanded
    /// envelope skeleton into the router's own ContextualCommand.
    #[test]
    fn parses_increase_envelope_skeleton() {
        let cc: pb::ContextualCommand = parse_txtpb(CONTEXTUAL_COMMAND, SKEL_INCREASE);
        let book = cc.command.expect("command book");
        assert_eq!(book.cover.as_ref().expect("cover").domain, "counter");
        let any = angzarr_router::command_payload(book.page.as_ref().expect("page"))
            .expect("command any");
        assert!(any.type_url.ends_with("test.counter.IncreaseBy"));
    }
}
