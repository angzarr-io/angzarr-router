//! Conformance harness support: the generated fixture types, a descriptor
//! pool for parsing `.txtpb` fixtures, the CounterAggregate built on the
//! router core, and the "parse a skeleton, set the scenario's data" helpers
//! the step definitions call. The cucumber step defs live in
//! `tests/cucumber.rs`.

use std::sync::{Arc, Mutex, OnceLock};

use angzarr_router::aggregate::{AggregateDispatch, FactRecord};
use angzarr_router::error::{CodedError, HandlerError};
use angzarr_router::process_manager::ProcessManagerDispatch;
use angzarr_router::projector::ProjectorDispatch;
use angzarr_router::rebuild::Rebuilder;
use angzarr_router::saga::SagaDispatch;
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

/// Build the CounterAggregate dispatch table (appliers and handlers).
pub fn counter_aggregate(observed: ObservedSink) -> AggregateDispatch<CounterState> {
    let rebuilder = Rebuilder::new(CounterState::default)
        .apply(
            "test.counter.Increased",
            |state: &mut CounterState, event| {
                // Decode the payload so a corrupt persisted event fails the fold
                // (PERSISTED_EVENT_CORRUPT). Increased is empty, so every
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
                    VALUE_NOT_POSITIVE,
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
// The fixture: OrderSaga on the router core (translation-side dispatch).
// ---------------------------------------------------------------------------

/// Build the OrderSaga dispatch table: it translates each Increased source
/// event into one Reserve command for "inventory" (passed through unstamped),
/// recording each triggering event's sequence in `seen`. Undeclared events
/// are skipped.
pub fn order_saga(seen: SequenceSink) -> SagaDispatch {
    SagaDispatch::new("order-saga", "order", ["inventory"]).on_event_with_context(
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
        pages: vec![pb::CommandPage {
            payload: Some(pb::command_page::Payload::Command(prost_types::Any {
                type_url: angzarr_router::type_url("test.counter.Reserve"),
                value: Vec::new(),
            })),
            ..Default::default()
        }],
    }
}

/// A SagaHandleRequest whose source carries one event of `event_fq` in the
/// "order" domain, at sequence `seq` when given.
pub fn saga_event_source(event_fq: &str, seq: Option<u32>) -> pb::SagaHandleRequest {
    pb::SagaHandleRequest {
        source: Some(pb::EventBook {
            cover: Some(pb::Cover {
                domain: "order".to_string(),
                ..Default::default()
            }),
            pages: vec![pb::EventPage {
                payload: Some(pb::event_page::Payload::Event(prost_types::Any {
                    type_url: angzarr_router::type_url(event_fq),
                    value: Vec::new(),
                })),
                header: seq.map(sequence_header),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A page header carrying an explicit sequence.
pub fn sequence_header(seq: u32) -> pb::PageHeader {
    pb::PageHeader {
        sequence_type: Some(pb::page_header::SequenceType::Sequence(seq)),
        ..Default::default()
    }
}

/// A SagaHandleRequest whose source book carries no pages → EMPTY_SAGA_SOURCE.
pub fn saga_empty_source() -> pb::SagaHandleRequest {
    pb::SagaHandleRequest {
        source: Some(pb::EventBook::default()),
        ..Default::default()
    }
}

/// A SagaHandleRequest with no source at all → MISSING_SAGA_SOURCE.
pub fn saga_missing_source() -> pb::SagaHandleRequest {
    pb::SagaHandleRequest {
        source: None,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// The fixture: OrderProcessManager on the router core (stateful dispatch).
// ---------------------------------------------------------------------------

/// The PM's own event-sourced state. Host state — it never crosses the
/// boundary; the harness reads reactions out of the response.
#[derive(Default)]
pub struct ProcessManagerState {
    pub count: u32,
}

/// Build the OrderProcessManager dispatch table: over the "counter" domain it
/// reacts to the NEWEST Increased trigger by emitting one Reserve command for
/// "inventory" plus one unaddressed process event. Its own state
/// counts prior Increased process events, so a rebuild is observable.
pub fn order_pm() -> ProcessManagerDispatch<ProcessManagerState> {
    let rebuilder = Rebuilder::new(ProcessManagerState::default).apply(
        "test.counter.Increased",
        |state: &mut ProcessManagerState, event| {
            counter::Increased::decode(event.value.as_slice())?;
            state.count += 1;
            Ok(())
        },
    );

    ProcessManagerDispatch::new("order-pm", "order-pm", ["inventory"], rebuilder).on_event(
        "counter",
        "test.counter.Increased",
        |_event, state: &mut ProcessManagerState, _cover| {
            let cmd = reserve_command_to("inventory");
            // Emit one fact per prior state event so the rebuild is
            // observable in the response.
            let facts = (0..state.count).map(|_| pm_process_event()).collect();
            Ok(pb::ProcessManagerHandleResponse {
                process_events: vec![pm_process_event()],
                commands: vec![cmd],
                facts,
                ..Default::default()
            })
        },
    )
}

/// The AuditProcessManager's own state: a different type from
/// [`ProcessManagerState`], so a PM folding into another's state is detectable.
#[derive(Default)]
pub struct AuditState {
    pub seen: Vec<String>,
}

/// Cover domain the AuditProcessManager stamps on its facts and process
/// events, so scenarios can tell its reactions from the order PM's.
pub const AUDIT_MARK: &str = "audit";

/// Build the AuditProcessManager dispatch table (domain "audit-pm"): co-resident
/// with the order PM over the same "counter" Increased trigger, but over its
/// own state type. It reacts with one "audit" fact per prior state event and
/// no commands.
pub fn audit_pm() -> ProcessManagerDispatch<AuditState> {
    let rebuilder = Rebuilder::new(AuditState::default).apply(
        "test.counter.Increased",
        |state: &mut AuditState, event| {
            counter::Increased::decode(event.value.as_slice())?;
            state.seen.push("Increased".to_string());
            Ok(())
        },
    );

    ProcessManagerDispatch::new(
        "AuditProcessManager",
        "audit-pm",
        Vec::<String>::new(),
        rebuilder,
    )
    .on_event(
        "counter",
        "test.counter.Increased",
        |_event, state: &mut AuditState, _cover| {
            let facts = state.seen.iter().map(|_| audit_book()).collect();
            Ok(pb::ProcessManagerHandleResponse {
                facts,
                ..Default::default()
            })
        },
    )
}

fn audit_book() -> pb::EventBook {
    pb::EventBook {
        cover: Some(pb::Cover {
            domain: AUDIT_MARK.to_string(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A PM process-state book of `n` Increased events owned by `pm_domain` (its
/// cover addresses the owning PM).
pub fn pm_state_in(pm_domain: &str, n: u32) -> pb::EventBook {
    let mut book = pm_state_of(n);
    book.cover = Some(pb::Cover {
        domain: pm_domain.to_string(),
        ..Default::default()
    });
    book
}

fn pm_process_event() -> pb::EventBook {
    pb::EventBook {
        pages: vec![pb::EventPage::default()],
        ..Default::default()
    }
}

/// A PM request whose trigger carries the given event pages in `domain`, the
/// newest at sequence `newest_seq` when given. `process_state` is the PM's
/// prior state book.
pub fn pm_trigger_request(
    domain: &str,
    event_fqs: &[&str],
    process_state: Option<pb::EventBook>,
    newest_seq: Option<u32>,
) -> pb::ProcessManagerHandleRequest {
    let pages = event_fqs
        .iter()
        .map(|fq| pb::EventPage {
            payload: Some(pb::event_page::Payload::Event(prost_types::Any {
                type_url: angzarr_router::type_url(fq),
                value: Vec::new(),
            })),
            ..Default::default()
        })
        .collect();
    let mut pages: Vec<pb::EventPage> = pages;
    if let (Some(seq), Some(last)) = (newest_seq, pages.last_mut()) {
        last.header = Some(sequence_header(seq));
    }
    pb::ProcessManagerHandleRequest {
        trigger: Some(pb::EventBook {
            cover: Some(pb::Cover {
                domain: domain.to_string(),
                ..Default::default()
            }),
            pages,
            ..Default::default()
        }),
        process_state,
    }
}

/// A PM request with an empty trigger book → EMPTY_PM_TRIGGER.
pub fn pm_empty_trigger() -> pb::ProcessManagerHandleRequest {
    pb::ProcessManagerHandleRequest {
        trigger: Some(pb::EventBook::default()),
        ..Default::default()
    }
}

/// A PM request with no trigger at all → MISSING_PM_TRIGGER.
pub fn pm_missing_trigger() -> pb::ProcessManagerHandleRequest {
    pb::ProcessManagerHandleRequest {
        trigger: None,
        ..Default::default()
    }
}

/// A PM process-state book of `n` Increased events (drives the rebuild).
pub fn pm_state_of(n: u32) -> pb::EventBook {
    pb::EventBook {
        pages: (0..n)
            .map(|_| pb::EventPage {
                payload: Some(pb::event_page::Payload::Event(increased_any())),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
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

/// A well-known opaque linkage a command's cover carries on behalf of a
/// parent.
pub fn parent_linkage() -> prost_types::Any {
    prost_types::Any {
        type_url: angzarr_router::type_url("test.counter.Parent"),
        value: vec![1, 2, 3],
    }
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

/// Rewrites every Any type URL in `cmd` (the command) and its prior history
/// (the events) to `prefix` + the fully-qualified name.
pub fn with_type_url_prefix(mut cmd: pb::ContextualCommand, prefix: &str) -> pb::ContextualCommand {
    let rewrite = |any: &mut prost_types::Any| {
        any.type_url = format!(
            "{prefix}{}",
            angzarr_router::type_name_from_url(&any.type_url)
        );
    };
    if let Some(book) = cmd.command.as_mut() {
        for page in &mut book.pages {
            if let Some(pb::command_page::Payload::Command(any)) = page.payload.as_mut() {
                rewrite(any);
            }
        }
    }
    if let Some(book) = cmd.events.as_mut() {
        for page in &mut book.pages {
            if let Some(pb::event_page::Payload::Event(any)) = page.payload.as_mut() {
                rewrite(any);
            }
        }
    }
    cmd
}

// ---------------------------------------------------------------------------
// Context fixtures: facts, replay, cover access, projector page context
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
            value: root_of(label),
        }),
        ..Default::default()
    }
}

/// Covers a handler observed.
pub type CoverSink = Arc<Mutex<Vec<Option<pb::Cover>>>>;

/// Page sequences an applier observed.
pub type SequenceSink = Arc<Mutex<Vec<u32>>>;

fn increased_page(seq: Option<u32>) -> pb::EventPage {
    pb::EventPage {
        header: seq.map(sequence_header),
        payload: Some(pb::event_page::Payload::Event(increased_any())),
        ..Default::default()
    }
}

/// The ledger aggregate (domain "ledger") over CounterState: Increased folds
/// count += 1 and records the page sequence it applied; a snapshot loads
/// CounterState; IncreaseBy records the handled cover and emits one Increased
/// whose cover carries the ledger's own linkage ([`ledger_linkage`]); the
/// only declared fact, Increased, is recorded as received and flagged by a
/// CounterState carrying the count it brings the ledger to.
pub fn ledger_aggregate(seen: CoverSink, applied: SequenceSink) -> AggregateDispatch<CounterState> {
    let rebuilder = Rebuilder::new(CounterState::default)
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
    AggregateDispatch::new("Ledger", "ledger", rebuilder)
        .on_command("test.counter.IncreaseBy", move |_cmd, _state, cctx| {
            seen.lock().unwrap().push(cctx.cover.clone());
            Ok(Some(pb::EventBook {
                cover: Some(pb::Cover {
                    ext: Some(ledger_linkage()),
                    ..Default::default()
                }),
                pages: vec![pb::EventPage {
                    payload: Some(pb::event_page::Payload::Event(increased_any())),
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

/// A FactRequest of `facts` pages of `test.counter.<fact>` in "ledger" over
/// `prior` Increased events.
pub fn fact_request(fact: &str, facts: u32, prior: u32) -> pb::FactRequest {
    let page = pb::EventPage {
        payload: Some(pb::event_page::Payload::Event(prost_types::Any {
            type_url: angzarr_router::type_url(&format!("test.counter.{fact}")),
            value: Vec::new(),
        })),
        ..Default::default()
    };
    pb::FactRequest {
        facts: Some(pb::EventBook {
            cover: Some(pb::Cover {
                domain: "ledger".to_string(),
                ..Default::default()
            }),
            pages: (0..facts).map(|_| page.clone()).collect(),
            ..Default::default()
        }),
        prior_events: Some(pb::EventBook {
            pages: (0..prior).map(|i| increased_page(Some(i))).collect(),
            next_sequence: prior,
            ..Default::default()
        }),
    }
}

/// A ReplayRequest: a snapshot of `count` at sequence 1, then `events`
/// Increased events at sequences 2...
pub fn replay_request(count: u32, events: u32) -> pb::ReplayRequest {
    pb::ReplayRequest {
        base_snapshot: Some(pb::Snapshot {
            sequence: 1,
            state: Some(prost_types::Any {
                type_url: angzarr_router::type_url("test.counter.CounterState"),
                value: CounterState { count }.encode_to_vec(),
            }),
            ..Default::default()
        }),
        events: (0..events).map(|i| increased_page(Some(2 + i))).collect(),
    }
}

/// A ReplayRequest of `events` Increased events at sequences 0...
pub fn events_replay_request(events: u32) -> pb::ReplayRequest {
    pb::ReplayRequest {
        base_snapshot: None,
        events: (0..events).map(|i| increased_page(Some(i))).collect(),
    }
}

/// An IncreaseBy command for the ledger root `label`.
/// The parent linkage the ledger sets on its own events.
pub fn ledger_linkage() -> prost_types::Any {
    prost_types::Any {
        type_url: angzarr_router::type_url("test.counter.Parent"),
        value: vec![4, 5, 6],
    }
}

/// [`ledger_command`] on behalf of a parent ([`parent_linkage`]).
pub fn ledger_command_with_linkage(label: &str) -> pb::ContextualCommand {
    let mut cc = ledger_command(label);
    cc.command.as_mut().unwrap().cover.as_mut().unwrap().ext = Some(parent_linkage());
    cc
}

pub fn ledger_command(label: &str) -> pb::ContextualCommand {
    pb::ContextualCommand {
        command: Some(pb::CommandBook {
            cover: Some(cover_of("ledger", label)),
            pages: vec![pb::CommandPage {
                payload: Some(pb::command_page::Payload::Command(prost_types::Any {
                    type_url: angzarr_router::type_url("test.counter.IncreaseBy"),
                    value: counter::IncreaseBy { n: 1 }.encode_to_vec(),
                })),
                ..Default::default()
            }],
        }),
        events: None,
    }
}

/// The reserving process-manager (domain "reserving-pm", target
/// "inventory") over CounterState (Increased folds count += 1): an Increased
/// trigger from "counter" records the trigger cover and emits nothing.
pub fn reserving_pm(seen: CoverSink) -> ProcessManagerDispatch<CounterState> {
    ProcessManagerDispatch::new(
        "Reserving",
        "reserving-pm",
        ["inventory"],
        Rebuilder::new(CounterState::default).apply(
            "test.counter.Increased",
            |state: &mut CounterState, _| {
                state.count += 1;
                Ok(())
            },
        ),
    )
    .on_event("counter", "test.counter.Increased", move |_e, _s, cover| {
        seen.lock().unwrap().push(cover.cloned());
        Ok(pb::ProcessManagerHandleResponse::default())
    })
}

/// An Increased trigger from "counter" root `label` at sequence `seq`.
pub fn reserving_trigger(label: &str, seq: u32) -> pb::ProcessManagerHandleRequest {
    pb::ProcessManagerHandleRequest {
        trigger: Some(pb::EventBook {
            cover: Some(cover_of("counter", label)),
            pages: vec![increased_page(Some(seq))],
            ..Default::default()
        }),
        process_state: None,
    }
}

/// (root bytes, sequence) pairs a projector fold observed.
pub type PageSink = Arc<Mutex<Vec<(Vec<u8>, u32)>>>;

/// The tracking projector: every Increased fold records its book's root and
/// the page's sequence.
pub fn tracking_projector(seen: PageSink) -> ProjectorDispatch<()> {
    ProjectorDispatch::new("Tracker", || ()).on_event(
        "test.counter.Increased",
        move |_p, _e, ctx| {
            let root = ctx
                .cover
                .and_then(|c| c.root.as_ref())
                .map(|r| r.value.clone())
                .unwrap_or_default();
            seen.lock().unwrap().push((root, ctx.sequence));
            Ok(())
        },
    )
}

/// A book of Increased events of "counter" root `label` at `sequences`.
pub fn tracked_book(label: &str, sequences: &[u32]) -> pb::EventBook {
    pb::EventBook {
        cover: Some(cover_of("counter", label)),
        pages: sequences.iter().map(|s| increased_page(Some(*s))).collect(),
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
        let any = angzarr_router::command_payload(&book.pages[0]).expect("command any");
        assert!(any.type_url.ends_with("test.counter.IncreaseBy"));
    }
}
