//! Conformance harness support: the generated fixture types, a descriptor
//! pool for parsing `.txtpb` fixtures, the CounterAggregate built on the
//! router core, and the "parse a skeleton, set the scenario's data" helpers
//! the step definitions call. The cucumber step defs live in
//! `tests/cucumber.rs`.

use std::sync::{Arc, Mutex, OnceLock};

use angzarr_router::aggregate::AggregateDispatch;
use angzarr_router::error::{CodedError, HandlerError};
use angzarr_router::projector::ProjectorDispatch;
use angzarr_router::rebuild::Rebuilder;
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage};

/// Generated `test.counter` fixture messages (Increased, IncreaseBy, …).
pub mod counter {
    include!(concat!(env!("OUT_DIR"), "/test.counter.rs"));
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

const CONTEXTUAL_COMMAND: &str = "io.angzarr.v1.ContextualCommand";
const EVENT_PAGE: &str = "io.angzarr.v1.EventPage";

/// What the IncreaseBy handler observed at dispatch time: the historical-state
/// evidence the framework supplies (`next_sequence`, `had_prior_events`) and
/// the rebuilt `count`. Recorded into a harness-owned sink, since host state
/// never crosses the boundary — this is how scenarios assert that evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observed {
    pub next_sequence: u32,
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

/// Build the CounterAggregate dispatch table (appliers, handlers, ordered
/// rejection compensators).
pub fn counter_aggregate(observed: ObservedSink) -> AggregateDispatch<CounterState> {
    let rebuilder = Rebuilder::new(CounterState::default)
        .apply("test.counter.Increased", |state: &mut CounterState, event| {
            // Decode the payload so a corrupt persisted event fails the fold
            // (PERSISTED_EVENT_CORRUPT). Increased is empty, so every
            // well-formed event decodes and simply increments.
            counter::Increased::decode(event.value.as_slice())?;
            state.count += 1;
            Ok(())
        })
        .with_snapshot(|state: &mut CounterState, snapshot| {
            // Seed state from the snapshot; pages at or below its sequence are
            // already folded in and must not re-apply (covered-page boundary).
            state.count = counter::CounterState::decode(snapshot.value.as_slice())?.count;
            Ok(())
        });

    AggregateDispatch::new("counter-aggregate", "counter", rebuilder)
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
                    "VALUE_NOT_POSITIVE",
                    "increase amount must be positive",
                    [],
                )));
            }
            let pages = (0..cmd.n)
                .map(|_| pb::EventPage {
                    payload: Some(pb::event_page::Payload::Event(increased_any())),
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
        // Two compensators for the same rejected command → ordered fan-out.
        .on_rejected("test.counter.Reserve", |_n, _r, _s, _c| {
            Ok(marker_response("CompensatedFirst"))
        })
        .on_rejected("test.counter.Reserve", |_n, _r, _s, _c| {
            Ok(marker_response("CompensatedSecond"))
        })
}

fn increased_any() -> prost_types::Any {
    prost_types::Any {
        type_url: angzarr_router::type_url("test.counter.Increased"),
        value: counter::Increased {}.encode_to_vec(),
    }
}

/// A single-page compensation response whose event type carries `label`, so
/// the fan-out order is observable in the merged book.
fn marker_response(label: &str) -> pb::BusinessResponse {
    pb::BusinessResponse {
        result: Some(pb::business_response::Result::Events(pb::EventBook {
            pages: vec![pb::EventPage {
                payload: Some(pb::event_page::Payload::Event(prost_types::Any {
                    type_url: angzarr_router::type_url(&format!("test.counter.{label}")),
                    value: Vec::new(),
                })),
                ..Default::default()
            }],
            ..Default::default()
        })),
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

/// Build the CounterProjector dispatch table: over the "counter" domain it
/// folds each Increased event into a running count, then finishes into a
/// Projection whose sequence carries that count and whose payload is the
/// CounterState. A book from any other domain folds nothing (C-0032).
pub fn counter_projector() -> ProjectorDispatch<ProjectorState> {
    ProjectorDispatch::new("counter-projector", ProjectorState::default)
        .for_domains(["counter"])
        .on_event(
            "test.counter.Increased",
            |state: &mut ProjectorState, event| {
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
                sequence: state.count,
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
                payload: Some(pb::event_page::Payload::Event(increased_any())),
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

/// A well-known opaque linkage stamped on a command's cover, used to prove
/// fill-only ext propagation onto emitted events.
pub fn parent_linkage() -> prost_types::Any {
    prost_types::Any {
        type_url: angzarr_router::type_url("test.counter.Parent"),
        value: vec![1, 2, 3],
    }
}

/// An IncreaseBy command carrying parent linkage on its cover.
pub fn increase_command_with_linkage(n: u32) -> pb::ContextualCommand {
    let mut cc = increase_command(n);
    cc.command
        .as_mut()
        .expect("command book")
        .cover
        .as_mut()
        .expect("cover")
        .ext = Some(parent_linkage());
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

/// A ContextualCommand carrying a rejection `Notification` for `fq_command`,
/// routed through the same dispatch entry as a command (the core detects the
/// notification type and takes the compensation path). Built by field — the
/// envelope nests Notification → RejectionNotification → the rejected book.
pub fn rejection_command(fq_command: &str) -> pb::ContextualCommand {
    let counter_cover = || {
        Some(pb::Cover {
            domain: "counter".to_string(),
            ..Default::default()
        })
    };
    let rejection = pb::RejectionNotification {
        rejected_command: Some(pb::CommandBook {
            cover: counter_cover(),
            pages: vec![pb::CommandPage {
                payload: Some(pb::command_page::Payload::Command(prost_types::Any {
                    type_url: angzarr_router::type_url(fq_command),
                    value: Vec::new(),
                })),
                ..Default::default()
            }],
        }),
        ..Default::default()
    };
    let notification = pb::Notification {
        payload: Some(prost_types::Any {
            type_url: angzarr_router::type_url("io.angzarr.v1.RejectionNotification"),
            value: rejection.encode_to_vec(),
        }),
        ..Default::default()
    };
    pb::ContextualCommand {
        command: Some(pb::CommandBook {
            cover: counter_cover(),
            pages: vec![pb::CommandPage {
                payload: Some(pb::command_page::Payload::Command(prost_types::Any {
                    type_url: angzarr_router::NOTIFICATION_TYPE_URL.to_string(),
                    value: notification.encode_to_vec(),
                })),
                ..Default::default()
            }],
        }),
        events: None,
    }
}

// Envelope-guard negatives: a well-formed skeleton with exactly one structural
// field cleared, so the guard fires regardless of the rest being valid.

/// No command book at all → MISSING_COMMAND_BOOK.
pub fn command_missing_book() -> pb::ContextualCommand {
    let mut cc = increase_command(1);
    cc.command = None;
    cc
}

/// A command book with no pages → MISSING_COMMAND_PAGE.
pub fn command_missing_page() -> pb::ContextualCommand {
    let mut cc = increase_command(1);
    cc.command.as_mut().expect("command book").pages.clear();
    cc
}

/// A command page carrying no payload → MISSING_COMMAND_PAYLOAD.
pub fn command_missing_payload() -> pb::ContextualCommand {
    let mut cc = increase_command(1);
    cc.command.as_mut().expect("command book").pages[0].payload = None;
    cc
}

/// Prior history of `n` confirmed increases: the Increased skeleton replayed
/// at consecutive sequences, with `next_sequence` continuing past them.
pub fn prior_history(n: u32) -> Option<pb::EventBook> {
    if n == 0 {
        return None;
    }
    let page: pb::EventPage = parse_txtpb(EVENT_PAGE, SKEL_INCREASED_EVENT);
    let pages = (0..n)
        .map(|seq| {
            let mut p = page.clone();
            p.header = Some(pb::PageHeader {
                sync_mode: None,
                sequence_type: Some(pb::page_header::SequenceType::Sequence(seq)),
            });
            p
        })
        .collect();
    Some(pb::EventBook {
        pages,
        next_sequence: n,
        ..Default::default()
    })
}

/// Prior history whose single Increased event carries undecodable payload
/// bytes (a truncated varint) → the applier fails the fold, surfacing
/// PERSISTED_EVENT_CORRUPT when a known command rebuilds over it.
pub fn corrupt_prior_history() -> Option<pb::EventBook> {
    let mut page: pb::EventPage = parse_txtpb(EVENT_PAGE, SKEL_INCREASED_EVENT);
    if let Some(pb::event_page::Payload::Event(any)) = page.payload.as_mut() {
        any.value = vec![0xff, 0xff, 0xff];
    }
    page.header = Some(pb::PageHeader {
        sync_mode: None,
        sequence_type: Some(pb::page_header::SequenceType::Sequence(0)),
    });
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
    let increased_at = |seq: u32| {
        let mut p: pb::EventPage = parse_txtpb(EVENT_PAGE, SKEL_INCREASED_EVENT);
        p.header = Some(pb::PageHeader {
            sync_mode: None,
            sequence_type: Some(pb::page_header::SequenceType::Sequence(seq)),
        });
        p
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
    match book.pages[0].payload.as_mut().expect("command payload") {
        pb::command_page::Payload::Command(any) => any,
        pb::command_page::Payload::External(_) => {
            panic!("conformance fixtures carry inline commands, not offloaded payloads")
        }
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
        let any = angzarr_router::command_payload(&book.pages[0]).expect("command any");
        assert!(any.type_url.ends_with("test.counter.IncreaseBy"));
    }
}
