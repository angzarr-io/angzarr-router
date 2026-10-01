//! The Rust-side ABI consumer test: drives the extern "C" surface through
//! raw pointers exactly as a foreign binding would — the ABI is proven
//! before any binding exists. The host side is a hand-rolled
//! CounterAggregate (the conformance fixture's shape): sessions keyed by
//! host_ctx, one C-visible gateway fn, callback ids selecting thunks.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;

use prost::Message;
use prost_types::Any;

use angzarr_router::pb;

use super::*;
use crate::abi::{STATUS_OK, STATUS_OK_EMPTY};
use crate::proto::google::rpc as rpc_pb;
use crate::proto::io::angzarr::router::ffi::v1 as abi_pb;

// --- the host's business messages (what generated protobuf classes would be)

#[derive(Clone, PartialEq, prost::Message)]
struct IncreaseBy {
    #[prost(uint32, tag = "1")]
    n: u32,
}

#[derive(Clone, PartialEq, prost::Message)]
struct Increased {}

#[derive(Clone, PartialEq, prost::Message)]
struct CounterState {
    #[prost(uint32, tag = "1")]
    value: u32,
}

const FQ_INCREASE_BY: &str = "test.counter.IncreaseBy";
const FQ_INCREASED: &str = "test.counter.Increased";
const FQ_FAIL_HARD: &str = "test.counter.FailHard";
const FQ_RETURN_NOTHING: &str = "test.counter.ReturnNothing";
const FQ_RESERVE: &str = "test.counter.Reserve";

const CB_APPLIER: u64 = 1;
const CB_INCREASE_BY: u64 = 2;
const CB_FAIL_HARD: u64 = 3;
const CB_COMP_A: u64 = 4;
const CB_COMP_B: u64 = 5;
const CB_RETURN_NOTHING: u64 = 6;
const CB_SNAPSHOT: u64 = 7;
const CB_PROJ_FOLD: u64 = 8;
const CB_PROJ_FINISH: u64 = 9;
const CB_SAGA_EVENT: u64 = 10;
const CB_SAGA_COMP: u64 = 11;
const CB_PM_EVENT: u64 = 12;
const CB_PM_COMP: u64 = 13;
const CB_PM2_EVENT: u64 = 14;
const CB_PM2_COMP: u64 = 15;
const CB_PROJ_FOLD_REJECTS: u64 = 16;
/// Succeeds with STATUS_OK and no output (an applier/fold that returns 0).
const CB_OK_ZERO: u64 = 17;
/// Succeeds with STATUS_OK_EMPTY and no output (a handler emitting nothing).
const CB_OK_EMPTY: u64 = 18;
/// Fails with -13 and no status payload.
const CB_FAILS: u64 = 19;
const CB_UNDO_RESERVE: u64 = 20;
const CB_FACT_ANNOTATE: u64 = 21;
const CB_PACK_STATE: u64 = 22;
const CB_PM_COMP_COMMANDS: u64 = 23;
const CB_FACT_KEEP: u64 = 24;
const CB_FACT_GARBAGE: u64 = 25;

const FQ_ORDER_CREATED: &str = "test.order.OrderCreated";
const FQ_RESERVE_STOCK: &str = "test.order.ReserveStock";
const FQ_ORDER_SHIPPED: &str = "test.order.OrderShipped";

// --- host-side session registry (state never crosses the boundary)

#[derive(Default, Clone)]
struct Session {
    counter: u32,
    observed_covers: Vec<Option<pb::Cover>>,
    observed_pages: Vec<(Option<pb::Cover>, u32)>,
    applied_pages: Vec<(Option<pb::Cover>, u32)>,
    observed_cctx: Vec<(u32, bool)>,
    markers: Vec<&'static str>,
}

static SESSIONS: Mutex<Option<HashMap<usize, Session>>> = Mutex::new(None);

fn with_session<R>(key: usize, f: impl FnOnce(&mut Session) -> R) -> R {
    let mut guard = SESSIONS.lock().unwrap();
    let sessions = guard.get_or_insert_with(HashMap::new);
    f(sessions.entry(key).or_default())
}

fn session_snapshot(key: usize) -> Session {
    with_session(key, |s| s.clone())
}

// --- the host gateway: one C-visible fn, callback_id selects the thunk

unsafe fn host_fill(out: *mut AngzarrBuf, bytes: &[u8]) {
    let ptr = angzarr_buf_alloc(bytes.len());
    if !bytes.is_empty() {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
    }
    (*out).data = ptr;
    (*out).len = bytes.len();
}

fn rejection_status(code: i32, reason: &str, message: &str) -> Vec<u8> {
    rpc_pb::Status {
        code,
        message: message.to_string(),
        details: vec![Any {
            type_url: "type.googleapis.com/google.rpc.ErrorInfo".to_string(),
            value: rpc_pb::ErrorInfo {
                reason: reason.to_string(),
                domain: "angzarr.io".to_string(),
                metadata: Default::default(),
            }
            .encode_to_vec(),
        }],
    }
    .encode_to_vec()
}

fn increased_book(n: u32) -> pb::EventBook {
    pb::EventBook {
        pages: (0..n)
            .map(|_| pb::EventPage {
                payload: Some(pb::event_page::Payload::Event(Any {
                    type_url: format!("type.googleapis.com/{FQ_INCREASED}"),
                    value: Increased {}.encode_to_vec(),
                })),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

unsafe extern "C" fn host_cb(
    host_ctx: *mut c_void,
    callback_id: u64,
    _type_url: *const u8,
    _type_url_len: usize,
    payload: *const u8,
    payload_len: usize,
    aux: *const u8,
    aux_len: usize,
    out: *mut AngzarrBuf,
) -> i32 {
    let key = host_ctx as usize;
    let payload = if payload_len > 0 {
        std::slice::from_raw_parts(payload, payload_len)
    } else {
        &[]
    };
    let aux = if aux_len > 0 {
        std::slice::from_raw_parts(aux, aux_len)
    } else {
        &[]
    };

    match callback_id {
        CB_APPLIER => {
            if Increased::decode(payload).is_err() {
                return -3;
            }
            let paux = abi_pb::ProjectorEventAux::decode(aux).expect("applier page aux");
            with_session(key, |s| {
                s.counter += 1;
                s.applied_pages.push((paux.cover, paux.sequence));
            });
            STATUS_OK_EMPTY
        }
        CB_SNAPSHOT => {
            let Ok(state) = CounterState::decode(payload) else {
                return -3;
            };
            with_session(key, |s| s.counter = state.value);
            STATUS_OK_EMPTY
        }
        CB_INCREASE_BY => {
            let cctx = abi_pb::CommandContextAux::decode(aux).expect("cctx aux");
            with_session(key, |s| {
                s.observed_cctx
                    .push((cctx.next_sequence, cctx.had_prior_events));
                s.observed_covers.push(cctx.cover.clone());
            });
            let cmd = IncreaseBy::decode(payload).expect("IncreaseBy");
            if cmd.n == 0 {
                host_fill(
                    out,
                    &rejection_status(9, "VALUE_NOT_POSITIVE", "value must be positive"),
                );
                return -9;
            }
            host_fill(out, &increased_book(cmd.n).encode_to_vec());
            STATUS_OK
        }
        CB_PROJ_FOLD => {
            // Host folds one Increased event into its projection instance.
            if Increased::decode(payload).is_err() {
                return -3;
            }
            let paux = abi_pb::ProjectorEventAux::decode(aux).expect("projector aux");
            with_session(key, |s| {
                s.counter += 1;
                s.observed_pages.push((paux.cover, paux.sequence));
            });
            STATUS_OK_EMPTY
        }
        CB_PROJ_FOLD_REJECTS => {
            host_fill(
                out,
                &rejection_status(9, "PROJECTION_STALE", "projection is stale"),
            );
            -9
        }
        CB_PROJ_FINISH => {
            // Pack the folded count into the wire Projection. The core hands
            // the EventBook over as the payload so the host can carry cover.
            let book = pb::EventBook::decode(payload).expect("finish EventBook");
            let proj = pb::Projection {
                cover: book.cover,
                projector: "CounterProjector".to_string(),
                sequence: with_session(key, |s| s.counter),
                ..Default::default()
            };
            host_fill(out, &proj.encode_to_vec());
            STATUS_OK
        }
        CB_SAGA_EVENT => {
            // Host translates the source event into one stamped command,
            // stamping from the coordinator-supplied destination sequences.
            let saux = abi_pb::SagaEventAux::decode(aux).expect("saga event aux");
            with_session(key, |s| {
                s.observed_pages.push((saux.source_cover, saux.source_seq));
            });
            let cmd = pb::CommandBook {
                cover: Some(pb::Cover {
                    domain: "inventory".to_string(),
                    ..Default::default()
                }),
                pages: vec![pb::CommandPage {
                    payload: Some(pb::command_page::Payload::Command(Any {
                        type_url: format!("type.googleapis.com/{FQ_RESERVE_STOCK}"),
                        value: Vec::new(),
                    })),
                    ..Default::default()
                }],
            };
            let resp = pb::SagaResponse {
                commands: vec![cmd],
                events: Vec::new(),
            };
            host_fill(out, &resp.encode_to_vec());
            STATUS_OK
        }
        CB_SAGA_COMP => {
            let raux = abi_pb::RejectionAux::decode(aux).expect("rejection aux");
            pb::Notification::decode(raux.notification.as_slice()).expect("notification");
            pb::RejectionNotification::decode(raux.rejection.as_slice()).expect("rejection");
            let resp = pb::SagaResponse {
                commands: Vec::new(),
                events: vec![pb::EventBook {
                    pages: vec![pb::EventPage::default()],
                    ..Default::default()
                }],
            };
            host_fill(out, &resp.encode_to_vec());
            STATUS_OK
        }
        CB_PM_EVENT => {
            // Stateful PM: the host reacts to the newest trigger event and
            // emits one stamped command, returning the full PM response.
            let paux = abi_pb::PmEventAux::decode(aux).expect("pm event aux");
            let cmd = pb::CommandBook {
                cover: Some(pb::Cover {
                    domain: "inventory".to_string(),
                    ..Default::default()
                }),
                pages: vec![pb::CommandPage {
                    payload: Some(pb::command_page::Payload::Command(Any {
                        type_url: format!("type.googleapis.com/{FQ_RESERVE_STOCK}"),
                        value: Vec::new(),
                    })),
                    ..Default::default()
                }],
            };
            // The trigger cover crosses in the aux; echo its root into the
            // command's cover so tests can observe it.
            let mut cmd = cmd;
            cmd.cover.as_mut().unwrap().root = paux.trigger_cover.and_then(|c| c.root);
            let resp = pb::ProcessManagerHandleResponse {
                commands: vec![cmd],
                ..Default::default()
            };
            host_fill(out, &resp.encode_to_vec());
            STATUS_OK
        }
        CB_PM_COMP => {
            let raux = abi_pb::RejectionAux::decode(aux).expect("rejection aux");
            pb::Notification::decode(raux.notification.as_slice()).expect("notification");
            pb::RejectionNotification::decode(raux.rejection.as_slice()).expect("rejection");
            let resp = pb::ProcessManagerHandleResponse {
                process_events: vec![pb::EventBook {
                    pages: vec![pb::EventPage::default()],
                    ..Default::default()
                }],
                notification: Some(pb::Notification {
                    cover: Some(pb::Cover {
                        domain: "escalated".to_string(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            };
            host_fill(out, &resp.encode_to_vec());
            STATUS_OK
        }
        CB_PM2_EVENT => {
            // The second PM reacts with one command to "pm2-target".
            let resp = pb::ProcessManagerHandleResponse {
                commands: vec![pb::CommandBook {
                    cover: Some(pb::Cover {
                        domain: "pm2-target".to_string(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            };
            host_fill(out, &resp.encode_to_vec());
            STATUS_OK
        }
        CB_PM2_COMP => {
            // The second PM compensates with one process event in "pm2".
            let resp = pb::ProcessManagerHandleResponse {
                process_events: vec![pb::EventBook {
                    cover: Some(pb::Cover {
                        domain: "pm2".to_string(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            };
            host_fill(out, &resp.encode_to_vec());
            STATUS_OK
        }
        CB_UNDO_RESERVE => {
            let uaux = abi_pb::UndoAux::decode(aux).expect("undo aux");
            let compensate =
                pb::Compensate::decode(uaux.compensate.as_slice()).expect("compensate");
            pb::Notification::decode(uaux.notification.as_slice()).expect("notification");
            let resp = pb::BusinessResponse {
                result: Some(pb::business_response::Result::Events(pb::EventBook {
                    pages: compensate
                        .sequences
                        .iter()
                        .map(|_| pb::EventPage::default())
                        .collect(),
                    ..Default::default()
                })),
            };
            host_fill(out, &resp.encode_to_vec());
            STATUS_OK
        }
        CB_FACT_ANNOTATE => {
            // Annotates a fact with the folded counter (the state the fact
            // sees) and flags it with an Increased, which folds in turn.
            let counter = with_session(key, |s| s.counter);
            let record = abi_pb::FactRecord {
                fact: Some(Any {
                    type_url: "type.googleapis.com/test.counter.CounterState".to_string(),
                    value: CounterState { value: counter }.encode_to_vec(),
                }),
                flags: vec![Any {
                    type_url: format!("type.googleapis.com/{FQ_INCREASED}"),
                    value: Increased {}.encode_to_vec(),
                }],
            };
            host_fill(out, &record.encode_to_vec());
            STATUS_OK
        }
        CB_FACT_KEEP => {
            // A record with no fact: the fact is recorded as received.
            host_fill(out, &abi_pb::FactRecord::default().encode_to_vec());
            STATUS_OK
        }
        CB_FACT_GARBAGE => {
            host_fill(out, &[0xFF, 0xFF, 0xFF]);
            STATUS_OK
        }
        CB_PACK_STATE => {
            let counter = with_session(key, |s| s.counter);
            let packed = Any {
                type_url: "type.googleapis.com/test.counter.CounterState".to_string(),
                value: CounterState { value: counter }.encode_to_vec(),
            };
            host_fill(out, &packed.encode_to_vec());
            STATUS_OK
        }
        CB_PM_COMP_COMMANDS => {
            let resp = pb::ProcessManagerHandleResponse {
                commands: vec![pb::CommandBook {
                    cover: Some(pb::Cover {
                        domain: "inventory".to_string(),
                        ..Default::default()
                    }),
                    pages: vec![pb::CommandPage::default()],
                }],
                ..Default::default()
            };
            host_fill(out, &resp.encode_to_vec());
            STATUS_OK
        }
        CB_OK_ZERO => STATUS_OK,
        CB_OK_EMPTY => STATUS_OK_EMPTY,
        CB_FAILS => -13,
        CB_FAIL_HARD => -13, // plain failure, no status payload
        CB_RETURN_NOTHING => STATUS_OK_EMPTY,
        CB_COMP_A | CB_COMP_B => {
            let raux = abi_pb::RejectionAux::decode(aux).expect("rejection aux");
            // The aux must round-trip the framework shapes.
            pb::Notification::decode(raux.notification.as_slice()).expect("notification");
            pb::RejectionNotification::decode(raux.rejection.as_slice()).expect("rejection");
            with_session(key, |s| {
                s.markers.push(if callback_id == CB_COMP_A {
                    "comp-a"
                } else {
                    "comp-b"
                })
            });
            let resp = pb::BusinessResponse {
                result: Some(pb::business_response::Result::Events(pb::EventBook {
                    pages: vec![pb::EventPage::default()],
                    ..Default::default()
                })),
            };
            host_fill(out, &resp.encode_to_vec());
            STATUS_OK
        }
        _ => -13,
    }
}

// --- driving the extern "C" surface as a binding would

fn descriptor_bytes() -> Vec<u8> {
    abi_pb::AggregateDescriptor {
        name: "Counter".to_string(),
        domain: "counter".to_string(),
        commands: vec![
            abi_pb::CallbackEntry {
                fq_type: FQ_INCREASE_BY.to_string(),
                callback_id: CB_INCREASE_BY,
            },
            abi_pb::CallbackEntry {
                fq_type: FQ_FAIL_HARD.to_string(),
                callback_id: CB_FAIL_HARD,
            },
            abi_pb::CallbackEntry {
                fq_type: FQ_RETURN_NOTHING.to_string(),
                callback_id: CB_RETURN_NOTHING,
            },
        ],
        appliers: vec![abi_pb::CallbackEntry {
            fq_type: FQ_INCREASED.to_string(),
            callback_id: CB_APPLIER,
        }],
        rejections: vec![abi_pb::RejectionEntry {
            compensates: FQ_RESERVE.to_string(),
            callback_ids: vec![CB_COMP_A, CB_COMP_B],
        }],
        snapshot_callback_id: Some(CB_SNAPSHOT),
        ..Default::default()
    }
    .encode_to_vec()
}

struct Router(*mut c_void);

// The ABI contract: dispatches on different host_ctx values may run
// concurrently against one router. The test wrapper asserts that.
unsafe impl Send for Router {}
unsafe impl Sync for Router {}

impl Router {
    fn with_counter() -> Self {
        let r = angzarr_router_new();
        let desc = descriptor_bytes();
        let ret =
            unsafe { angzarr_router_register_aggregate(r, desc.as_ptr(), desc.len(), host_cb) };
        assert_eq!(ret, 0, "registration failed");
        Router(r)
    }

    /// Dispatches and copies out the response, releasing router memory —
    /// the full binding-side buffer discipline.
    fn dispatch(&self, session: usize, req: &pb::ContextualCommand) -> (i32, Vec<u8>) {
        let bytes = req.encode_to_vec();
        let mut out = AngzarrBuf {
            data: std::ptr::null_mut(),
            len: 0,
        };
        let ret = unsafe {
            angzarr_router_dispatch(
                self.0,
                session as *mut c_void,
                bytes.as_ptr(),
                bytes.len(),
                &mut out,
            )
        };
        let response = if out.data.is_null() {
            Vec::new()
        } else {
            let copied = unsafe { std::slice::from_raw_parts(out.data, out.len) }.to_vec();
            unsafe { angzarr_buf_release(out.data, out.len) };
            copied
        };
        (ret, response)
    }
}

impl Drop for Router {
    fn drop(&mut self) {
        unsafe { angzarr_router_free(self.0) };
    }
}

fn next_session() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

fn command_req(fq: &str, payload: Vec<u8>, events: Option<pb::EventBook>) -> pb::ContextualCommand {
    pb::ContextualCommand {
        command: Some(pb::CommandBook {
            cover: Some(pb::Cover {
                domain: "counter".to_string(),
                ..Default::default()
            }),
            pages: vec![pb::CommandPage {
                payload: Some(pb::command_page::Payload::Command(Any {
                    type_url: format!("type.googleapis.com/{fq}"),
                    value: payload,
                })),
                ..Default::default()
            }],
        }),
        events,
    }
}

fn decode_response(bytes: &[u8]) -> pb::BusinessResponse {
    pb::BusinessResponse::decode(bytes).expect("BusinessResponse")
}

fn decode_status(bytes: &[u8]) -> (rpc_pb::Status, String) {
    let status = rpc_pb::Status::decode(bytes).expect("Status");
    let reason = status
        .details
        .first()
        .map(|d| {
            rpc_pb::ErrorInfo::decode(d.value.as_slice())
                .expect("ErrorInfo")
                .reason
        })
        .unwrap_or_default();
    (status, reason)
}

fn increased_history(n: u32, next_sequence: u32) -> pb::EventBook {
    let mut book = increased_book(n);
    book.next_sequence = next_sequence;
    book
}

#[test]
fn abi_version_is_three() {
    assert_eq!(angzarr_abi_version(), 3);
}

#[test]
fn empty_history_command_emits_stamped_events() {
    let router = Router::with_counter();
    let session = next_session();
    let (ret, bytes) = router.dispatch(
        session,
        &command_req(FQ_INCREASE_BY, IncreaseBy { n: 2 }.encode_to_vec(), None),
    );
    assert_eq!(ret, 0);
    let resp = decode_response(&bytes);
    let Some(pb::business_response::Result::Events(book)) = resp.result else {
        panic!("expected events result");
    };
    assert_eq!(book.pages.len(), 2);
    let seqs: Vec<u32> = book
        .pages
        .iter()
        .map(angzarr_router::page_sequence)
        .collect();
    assert_eq!(seqs, vec![0, 1], "fill-only stamping from next_sequence 0");

    let s = session_snapshot(session);
    assert_eq!(
        s.observed_cctx,
        vec![(0, false)],
        "fresh aggregate evidence"
    );
    assert_eq!(s.counter, 0, "no history — appliers must not run");
}

#[test]
fn command_cover_ext_survives_the_round_trip() {
    // Fill-only ext is not an aux channel — it rides in-band inside the
    // ContextualCommand / BusinessResponse book bytes. This pins that the
    // stamped cover.ext survives the FFI seam, not just the native path.
    let router = Router::with_counter();
    let session = next_session();
    let ext = Any {
        type_url: "type.googleapis.com/test.counter.Parent".to_string(),
        value: vec![1, 2, 3],
    };
    let mut req = command_req(FQ_INCREASE_BY, IncreaseBy { n: 1 }.encode_to_vec(), None);
    req.command.as_mut().unwrap().cover.as_mut().unwrap().ext = Some(ext.clone());

    let (ret, bytes) = router.dispatch(session, &req);
    assert_eq!(ret, 0);
    let resp = decode_response(&bytes);
    let Some(pb::business_response::Result::Events(book)) = resp.result else {
        panic!("expected events result");
    };
    assert_eq!(
        book.cover.as_ref().and_then(|c| c.ext.as_ref()),
        Some(&ext),
        "command cover.ext must survive the FFI round-trip onto the emitted book",
    );
}

#[test]
fn prior_events_fold_through_host_appliers() {
    let router = Router::with_counter();
    let session = next_session();
    let (ret, bytes) = router.dispatch(
        session,
        &command_req(
            FQ_INCREASE_BY,
            IncreaseBy { n: 1 }.encode_to_vec(),
            Some(increased_history(2, 2)),
        ),
    );
    assert_eq!(ret, 0);
    let s = session_snapshot(session);
    assert_eq!(s.counter, 2, "both history pages folded host-side");
    assert_eq!(
        s.observed_cctx,
        vec![(2, true)],
        "historical-state evidence"
    );

    let resp = decode_response(&bytes);
    let Some(pb::business_response::Result::Events(book)) = resp.result else {
        panic!("expected events result");
    };
    assert_eq!(
        book.pages
            .iter()
            .map(angzarr_router::page_sequence)
            .collect::<Vec<_>>(),
        vec![2],
        "emitted sequence continues prior history"
    );
}

#[test]
fn snapshot_loads_and_covered_pages_skip() {
    let router = Router::with_counter();
    let session = next_session();

    let mut history = pb::EventBook {
        snapshot: Some(pb::Snapshot {
            sequence: 2,
            state: Some(Any {
                type_url: "type.googleapis.com/test.counter.CounterState".to_string(),
                value: CounterState { value: 10 }.encode_to_vec(),
            }),
            ..Default::default()
        }),
        next_sequence: 4,
        ..Default::default()
    };
    let event = |seq: u32| pb::EventPage {
        header: Some(pb::PageHeader {
            sequence_type: Some(pb::page_header::SequenceType::Sequence(seq)),
            ..Default::default()
        }),
        payload: Some(pb::event_page::Payload::Event(Any {
            type_url: format!("type.googleapis.com/{FQ_INCREASED}"),
            value: Vec::new(),
        })),
        ..Default::default()
    };
    history.pages = vec![event(2), event(3)]; // 2 covered (inclusive), 3 applies

    let (ret, _) = router.dispatch(
        session,
        &command_req(
            FQ_INCREASE_BY,
            IncreaseBy { n: 1 }.encode_to_vec(),
            Some(history),
        ),
    );
    assert_eq!(ret, 0);
    let s = session_snapshot(session);
    assert_eq!(s.counter, 11, "snapshot 10 + one uncovered page");
    assert_eq!(s.observed_cctx, vec![(4, true)]);
}

#[test]
fn rejection_crosses_as_status_with_error_info() {
    let router = Router::with_counter();
    let (ret, bytes) = router.dispatch(
        next_session(),
        &command_req(FQ_INCREASE_BY, IncreaseBy { n: 0 }.encode_to_vec(), None),
    );
    assert_eq!(ret, -9, "FAILED_PRECONDITION, negated");
    let (status, reason) = decode_status(&bytes);
    assert_eq!(status.code, 9);
    assert_eq!(reason, "VALUE_NOT_POSITIVE");
    assert_eq!(status.message, "value must be positive");
}

#[test]
fn plain_handler_failure_is_internal() {
    let router = Router::with_counter();
    let (ret, _) = router.dispatch(next_session(), &command_req(FQ_FAIL_HARD, Vec::new(), None));
    assert_eq!(ret, -13, "unclassified host failure surfaces as INTERNAL");
}

#[test]
fn unknown_command_is_unimplemented() {
    let router = Router::with_counter();
    let (ret, bytes) = router.dispatch(
        next_session(),
        &command_req("test.counter.Undeclared", Vec::new(), None),
    );
    assert_eq!(ret, -12);
    let (_, reason) = decode_status(&bytes);
    assert_eq!(reason, "NO_HANDLER_REGISTERED");
}

#[test]
fn corrupt_persisted_event_is_data_loss() {
    let router = Router::with_counter();
    let mut history = pb::EventBook {
        next_sequence: 1,
        ..Default::default()
    };
    history.pages = vec![pb::EventPage {
        payload: Some(pb::event_page::Payload::Event(Any {
            type_url: format!("type.googleapis.com/{FQ_INCREASED}"),
            value: vec![0xFF, 0xFF, 0xFF],
        })),
        ..Default::default()
    }];
    let (ret, bytes) = router.dispatch(
        next_session(),
        &command_req(
            FQ_INCREASE_BY,
            IncreaseBy { n: 1 }.encode_to_vec(),
            Some(history),
        ),
    );
    assert_eq!(ret, -15);
    let (_, reason) = decode_status(&bytes);
    assert_eq!(reason, "PERSISTED_EVENT_CORRUPT");
}

fn notification_command(fq_command: &str) -> Any {
    let rejection = pb::RejectionNotification {
        rejected_command: Some(pb::CommandBook {
            cover: Some(pb::Cover {
                domain: "counter".to_string(),
                ..Default::default()
            }),
            pages: vec![pb::CommandPage {
                payload: Some(pb::command_page::Payload::Command(Any {
                    type_url: format!("type.googleapis.com/{fq_command}"),
                    value: Vec::new(),
                })),
                ..Default::default()
            }],
        }),
        ..Default::default()
    };
    let notification = pb::Notification {
        payload: Some(Any {
            type_url: "type.googleapis.com/io.angzarr.v1.RejectionNotification".to_string(),
            value: rejection.encode_to_vec(),
        }),
        ..Default::default()
    };
    Any {
        type_url: angzarr_router::NOTIFICATION_TYPE_URL.to_string(),
        value: notification.encode_to_vec(),
    }
}

#[test]
fn rejection_fan_out_runs_in_order_and_merges() {
    let router = Router::with_counter();
    let session = next_session();
    let req = pb::ContextualCommand {
        command: Some(pb::CommandBook {
            cover: Some(pb::Cover {
                domain: "counter".to_string(),
                ..Default::default()
            }),
            pages: vec![pb::CommandPage {
                payload: Some(pb::command_page::Payload::Command(notification_command(
                    FQ_RESERVE,
                ))),
                ..Default::default()
            }],
        }),
        ..Default::default()
    };
    let (ret, bytes) = router.dispatch(session, &req);
    assert_eq!(ret, 0);
    let s = session_snapshot(session);
    assert_eq!(s.markers, vec!["comp-a", "comp-b"], "ordered fan-out");
    let resp = decode_response(&bytes);
    let Some(pb::business_response::Result::Events(book)) = resp.result else {
        panic!("expected merged events");
    };
    assert_eq!(book.pages.len(), 2, "compensation events merged");
}

#[test]
fn undeclared_rejection_yields_empty_response() {
    let router = Router::with_counter();
    let req = pb::ContextualCommand {
        command: Some(pb::CommandBook {
            cover: Some(pb::Cover {
                domain: "counter".to_string(),
                ..Default::default()
            }),
            pages: vec![pb::CommandPage {
                payload: Some(pb::command_page::Payload::Command(notification_command(
                    "test.counter.Undeclared",
                ))),
                ..Default::default()
            }],
        }),
        ..Default::default()
    };
    let (ret, bytes) = router.dispatch(next_session(), &req);
    assert_eq!(ret, 0);
    assert!(
        decode_response(&bytes).result.is_none(),
        "DelegateToFramework is an empty response"
    );
}

#[test]
fn handler_emitting_nothing_returns_empty_events() {
    let router = Router::with_counter();
    let (ret, bytes) = router.dispatch(
        next_session(),
        &command_req(FQ_RETURN_NOTHING, Vec::new(), None),
    );
    assert_eq!(ret, 0);
    let resp = decode_response(&bytes);
    let Some(pb::business_response::Result::Events(book)) = resp.result else {
        panic!("expected events result");
    };
    assert!(book.pages.is_empty());
}

#[test]
fn concurrent_dispatches_isolate_sessions() {
    let router = std::sync::Arc::new(Router::with_counter());
    let sessions: Vec<usize> = (0..4).map(|_| next_session()).collect();
    std::thread::scope(|scope| {
        for &session in &sessions {
            let router = router.clone();
            scope.spawn(move || {
                let (ret, _) = router.dispatch(
                    session,
                    &command_req(
                        FQ_INCREASE_BY,
                        IncreaseBy { n: 1 }.encode_to_vec(),
                        Some(increased_history(3, 3)),
                    ),
                );
                assert_eq!(ret, 0);
            });
        }
    });
    for session in sessions {
        let s = session_snapshot(session);
        assert_eq!(s.counter, 3, "each session folded only its own history");
        assert_eq!(s.observed_cctx, vec![(3, true)]);
    }
}

#[test]
fn panic_inside_an_entry_point_becomes_coded_unhandled() {
    let result = flatten_panic::<()>(std::panic::catch_unwind(|| panic!("boom")));
    let err = result.expect_err("panic must surface as a coded error");
    assert_eq!(err.code, "UNHANDLED_HANDLER_ERROR");
    assert_eq!(err.message, "boom");
    assert_eq!(err.grpc as i32, 13);
}

#[test]
fn null_router_pointer_is_a_coded_failure_not_a_crash() {
    let desc = descriptor_bytes();
    let ret = unsafe {
        angzarr_router_register_aggregate(std::ptr::null_mut(), desc.as_ptr(), desc.len(), host_cb)
    };
    assert_eq!(ret, -13);

    let mut out = AngzarrBuf {
        data: std::ptr::null_mut(),
        len: 0,
    };
    let ret = unsafe {
        angzarr_router_dispatch(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
            0,
            &mut out,
        )
    };
    assert_eq!(ret, -13);
    if !out.data.is_null() {
        unsafe { angzarr_buf_release(out.data, out.len) };
    }
}

#[test]
fn garbage_request_bytes_are_coded_not_fatal() {
    let router = Router::with_counter();
    let garbage = [0xFFu8, 0xFF, 0xFF, 0xFF];
    let mut out = AngzarrBuf {
        data: std::ptr::null_mut(),
        len: 0,
    };
    let ret = unsafe {
        angzarr_router_dispatch(
            router.0,
            std::ptr::null_mut(),
            garbage.as_ptr(),
            garbage.len(),
            &mut out,
        )
    };
    assert_eq!(ret, -3);
    let copied = unsafe { std::slice::from_raw_parts(out.data, out.len) }.to_vec();
    unsafe { angzarr_buf_release(out.data, out.len) };
    let (_, reason) = decode_status(&copied);
    assert_eq!(reason, "ANY_DECODE_FAILED");
}

// --- projector ABI surface (per-kind entry points)

fn projector_descriptor_bytes() -> Vec<u8> {
    abi_pb::ProjectorDescriptor {
        name: "CounterProjector".to_string(),
        domains: vec!["counter".to_string()],
        events: vec![abi_pb::CallbackEntry {
            fq_type: FQ_INCREASED.to_string(),
            callback_id: CB_PROJ_FOLD,
        }],
        finish_callback_id: Some(CB_PROJ_FINISH),
        unknown_callback_id: None,
    }
    .encode_to_vec()
}

impl Router {
    fn with_projector() -> Self {
        let r = angzarr_router_new();
        let desc = projector_descriptor_bytes();
        let ret =
            unsafe { angzarr_router_register_projector(r, desc.as_ptr(), desc.len(), host_cb) };
        assert_eq!(ret, 0, "projector registration failed");
        Router(r)
    }

    fn dispatch_projector(&self, session: usize, book: &pb::EventBook) -> (i32, Vec<u8>) {
        let bytes = book.encode_to_vec();
        let mut out = AngzarrBuf {
            data: std::ptr::null_mut(),
            len: 0,
        };
        let ret = unsafe {
            angzarr_router_dispatch_projector(
                self.0,
                session as *mut c_void,
                bytes.as_ptr(),
                bytes.len(),
                &mut out,
            )
        };
        let response = if out.data.is_null() {
            Vec::new()
        } else {
            let copied = unsafe { std::slice::from_raw_parts(out.data, out.len) }.to_vec();
            unsafe { angzarr_buf_release(out.data, out.len) };
            copied
        };
        (ret, response)
    }
}

fn book_in_domain(domain: &str, n: u32) -> pb::EventBook {
    let mut book = increased_book(n);
    book.cover = Some(pb::Cover {
        domain: domain.to_string(),
        ..Default::default()
    });
    book
}

#[test]
fn projector_folds_every_page_through_the_abi() {
    // The whole projector path across raw pointers: register a projector,
    // dispatch an EventBook, and confirm the host folded every page into one
    // projection whose finisher reports the count.
    let router = Router::with_projector();
    let session = next_session();
    let (ret, bytes) = router.dispatch_projector(session, &book_in_domain("counter", 3));
    assert_eq!(ret, 0);
    let proj = pb::Projection::decode(bytes.as_slice()).expect("Projection");
    assert_eq!(proj.projector, "CounterProjector");
    assert_eq!(proj.sequence, 3, "all three pages folded into one instance");
    assert_eq!(session_snapshot(session).counter, 3);
}

#[test]
fn projector_undeclared_domain_folds_nothing_through_the_abi() {
    // ForDomains("counter") with a book in another domain: no fold callback
    // fires, but finish still runs and reports zero.
    let router = Router::with_projector();
    let session = next_session();
    let (ret, bytes) = router.dispatch_projector(session, &book_in_domain("inventory", 3));
    assert_eq!(ret, 0);
    let proj = pb::Projection::decode(bytes.as_slice()).expect("Projection");
    assert_eq!(proj.sequence, 0, "undeclared domain folds nothing");
    assert_eq!(session_snapshot(session).counter, 0);
}

// --- saga ABI surface (per-kind entry points)

fn saga_descriptor_bytes() -> Vec<u8> {
    abi_pb::SagaDescriptor {
        name: "OrderFulfillment".to_string(),
        input_domain: "order".to_string(),
        target_domains: vec!["inventory".to_string()],
        events: vec![abi_pb::CallbackEntry {
            fq_type: FQ_ORDER_CREATED.to_string(),
            callback_id: CB_SAGA_EVENT,
        }],
        rejections: Vec::new(),
    }
    .encode_to_vec()
}

impl Router {
    fn with_saga() -> Self {
        let r = angzarr_router_new();
        let desc = saga_descriptor_bytes();
        let ret = unsafe { angzarr_router_register_saga(r, desc.as_ptr(), desc.len(), host_cb) };
        assert_eq!(ret, 0, "saga registration failed");
        Router(r)
    }

    fn dispatch_saga(&self, session: usize, req: &pb::SagaHandleRequest) -> (i32, Vec<u8>) {
        let bytes = req.encode_to_vec();
        let mut out = AngzarrBuf {
            data: std::ptr::null_mut(),
            len: 0,
        };
        let ret = unsafe {
            angzarr_router_dispatch_saga(
                self.0,
                session as *mut c_void,
                bytes.as_ptr(),
                bytes.len(),
                &mut out,
            )
        };
        let response = if out.data.is_null() {
            Vec::new()
        } else {
            let copied = unsafe { std::slice::from_raw_parts(out.data, out.len) }.to_vec();
            unsafe { angzarr_buf_release(out.data, out.len) };
            copied
        };
        (ret, response)
    }
}

/// A SagaHandleRequest over a source book in `domain` carrying the given
/// event pages.
fn saga_request(domain: &str, pages: Vec<pb::EventPage>) -> pb::SagaHandleRequest {
    pb::SagaHandleRequest {
        source: Some(pb::EventBook {
            cover: Some(pb::Cover {
                domain: domain.to_string(),
                ..Default::default()
            }),
            pages,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn event_page_of(fq: &str) -> pb::EventPage {
    pb::EventPage {
        payload: Some(pb::event_page::Payload::Event(Any {
            type_url: format!("type.googleapis.com/{fq}"),
            value: Vec::new(),
        })),
        ..Default::default()
    }
}

#[test]
fn saga_emits_deferred_command_through_the_abi() {
    // The whole saga path across raw pointers: register a saga, dispatch a
    // source book, and confirm the host's command came back deferred from the
    // triggering page.
    let router = Router::with_saga();
    let mut page = event_page_of(FQ_ORDER_CREATED);
    page.header = Some(pb::PageHeader {
        sequence_type: Some(pb::page_header::SequenceType::Sequence(7)),
        ..Default::default()
    });
    let req = saga_request("order", vec![page]);
    let (ret, bytes) = router.dispatch_saga(next_session(), &req);
    assert_eq!(ret, 0);
    let resp = pb::SagaResponse::decode(bytes.as_slice()).expect("SagaResponse");
    assert_eq!(resp.commands.len(), 1, "one command emitted");
    let cmd = &resp.commands[0];
    assert_eq!(cmd.cover.as_ref().unwrap().domain, "inventory");
    let Some(pb::page_header::SequenceType::AngzarrDeferred(d)) = cmd.pages[0]
        .header
        .as_ref()
        .and_then(|h| h.sequence_type.clone())
    else {
        panic!("command page is not deferred");
    };
    assert_eq!(d.source_seq, 7);
    assert_eq!(d.source.map(|c| c.domain), Some("order".to_string()));
}

#[test]
fn a_saga_handler_receives_the_source_cover_and_sequence() {
    let router = Router::with_saga();
    let session = next_session();
    let mut first = event_page_of(FQ_ORDER_CREATED);
    first.header = Some(pb::PageHeader {
        sequence_type: Some(pb::page_header::SequenceType::Sequence(3)),
        ..Default::default()
    });
    let req = saga_request("order", vec![first, event_page_of(FQ_ORDER_CREATED)]);
    let (ret, _) = router.dispatch_saga(session, &req);
    assert_eq!(ret, 0);
    let observed: Vec<(String, u32)> = session_snapshot(session)
        .observed_pages
        .into_iter()
        .map(|(cover, seq)| (cover.map(|c| c.domain).unwrap_or_default(), seq))
        .collect();
    assert_eq!(
        observed,
        vec![("order".to_string(), 3), ("order".to_string(), 0)],
        "each trigger's cover and sequence (0 when unsequenced)"
    );
}

#[test]
fn saga_missing_source_through_the_abi_is_missing_saga_source() {
    // Source-shape validation must precede domain routing: a request with no
    // source has no cover domain, matches no saga, and must report
    // MISSING_SAGA_SOURCE — not the routing layer's NO_HANDLER_REGISTERED.
    let router = Router::with_saga();
    let session = next_session();
    let (ret, bytes) = router.dispatch_saga(session, &pb::SagaHandleRequest::default());
    assert_eq!(ret, -3, "invalid argument, negated");
    let (_, reason) = decode_status(&bytes);
    assert_eq!(reason, angzarr_router::error::codes::MISSING_SAGA_SOURCE);
}

#[test]
fn saga_empty_source_through_the_abi_is_empty_saga_source() {
    // A source book with no pages likewise validates before routing as
    // EMPTY_SAGA_SOURCE, regardless of which saga (if any) consumes its domain.
    let router = Router::with_saga();
    let session = next_session();
    let req = pb::SagaHandleRequest {
        source: Some(pb::EventBook::default()),
        ..Default::default()
    };
    let (ret, bytes) = router.dispatch_saga(session, &req);
    assert_eq!(ret, -3, "invalid argument, negated");
    let (_, reason) = decode_status(&bytes);
    assert_eq!(reason, angzarr_router::error::codes::EMPTY_SAGA_SOURCE);
}

// --- process-manager ABI surface (per-kind entry points)

fn pm_descriptor_bytes() -> Vec<u8> {
    abi_pb::ProcessManagerDescriptor {
        name: "OrderPM".to_string(),
        pm_domain: "order-pm".to_string(),
        appliers: Vec::new(),
        snapshot_callback_id: None,
        events: vec![abi_pb::PmEventEntry {
            input_domain: "orders".to_string(),
            fq_type: FQ_ORDER_SHIPPED.to_string(),
            callback_id: CB_PM_EVENT,
        }],
        rejections: vec![abi_pb::RejectionEntry {
            compensates: FQ_RESERVE_STOCK.to_string(),
            callback_ids: vec![CB_PM_COMP],
        }],
        target_domains: vec!["inventory".to_string()],
        state_callback_id: None,
    }
    .encode_to_vec()
}

impl Router {
    fn with_process_manager() -> Self {
        let r = angzarr_router_new();
        let desc = pm_descriptor_bytes();
        let ret = unsafe {
            angzarr_router_register_process_manager(r, desc.as_ptr(), desc.len(), host_cb)
        };
        assert_eq!(ret, 0, "process-manager registration failed");
        Router(r)
    }

    fn dispatch_process_manager(
        &self,
        session: usize,
        req: &pb::ProcessManagerHandleRequest,
    ) -> (i32, Vec<u8>) {
        let bytes = req.encode_to_vec();
        let mut out = AngzarrBuf {
            data: std::ptr::null_mut(),
            len: 0,
        };
        let ret = unsafe {
            angzarr_router_dispatch_process_manager(
                self.0,
                session as *mut c_void,
                bytes.as_ptr(),
                bytes.len(),
                &mut out,
            )
        };
        let response = if out.data.is_null() {
            Vec::new()
        } else {
            let copied = unsafe { std::slice::from_raw_parts(out.data, out.len) }.to_vec();
            unsafe { angzarr_buf_release(out.data, out.len) };
            copied
        };
        (ret, response)
    }
}

/// A PM request over a trigger book in `domain` carrying the given pages.
fn pm_request(domain: &str, pages: Vec<pb::EventPage>) -> pb::ProcessManagerHandleRequest {
    pb::ProcessManagerHandleRequest {
        trigger: Some(pb::EventBook {
            cover: Some(pb::Cover {
                domain: domain.to_string(),
                ..Default::default()
            }),
            pages,
            ..Default::default()
        }),
        process_state: None,
    }
}

#[test]
fn pm_emits_deferred_command_through_the_abi() {
    // The whole PM path across raw pointers: the host sees the trigger cover
    // in its aux (it echoes the root into its command), and the command comes
    // back deferred from the newest trigger page.
    let router = Router::with_process_manager();
    let mut page = event_page_of(FQ_ORDER_SHIPPED);
    page.header = Some(pb::PageHeader {
        sequence_type: Some(pb::page_header::SequenceType::Sequence(4)),
        ..Default::default()
    });
    let mut req = pm_request("orders", vec![page]);
    req.trigger.as_mut().unwrap().cover.as_mut().unwrap().root = Some(pb::Uuid { value: vec![8] });
    let (ret, bytes) = router.dispatch_process_manager(next_session(), &req);
    assert_eq!(ret, 0);
    let resp = pb::ProcessManagerHandleResponse::decode(bytes.as_slice()).expect("PMResponse");
    assert_eq!(resp.commands.len(), 1);
    let cmd = &resp.commands[0];
    assert_eq!(
        cmd.cover.as_ref().unwrap().root,
        Some(pb::Uuid { value: vec![8] }),
        "the trigger cover crossed to the host"
    );
    let Some(pb::page_header::SequenceType::AngzarrDeferred(d)) = cmd.pages[0]
        .header
        .as_ref()
        .and_then(|h| h.sequence_type.clone())
    else {
        panic!("command page is not deferred");
    };
    assert_eq!(d.source_seq, 4);
}

#[test]
fn pm_compensator_runs_through_the_abi() {
    // A Notification trigger routes to the compensator, whose process event +
    // escalation cross back as a ProcessManagerHandleResponse.
    let router = Router::with_process_manager();
    let session = next_session();
    let notification_page = pb::EventPage {
        payload: Some(pb::event_page::Payload::Event(notification_command(
            FQ_RESERVE_STOCK,
        ))),
        ..Default::default()
    };
    let req = pm_request("orders", vec![notification_page]);
    let (ret, bytes) = router.dispatch_process_manager(session, &req);
    assert_eq!(ret, 0);
    let resp = pb::ProcessManagerHandleResponse::decode(bytes.as_slice()).expect("PMResponse");
    assert_eq!(
        resp.process_events.len(),
        1,
        "compensator emitted one process event"
    );
    assert_eq!(
        resp.notification.expect("escalation").cover.unwrap().domain,
        "escalated"
    );
}

#[test]
fn two_process_managers_share_a_source_domain_route_by_type() {
    // Two PMs subscribe to the SAME source domain (`orders`) for different event
    // types. A trigger routes to the PM that declares its type; the other no-ops.
    // Before multi-PM routing, registering two PMs returned NO_HANDLER_REGISTERED.
    let r = angzarr_router_new();
    let pm1 = pm_descriptor_bytes(); // OrderPM: orders/OrderShipped -> CB_PM_EVENT
    let pm2 = abi_pb::ProcessManagerDescriptor {
        name: "OtherPM".to_string(),
        pm_domain: "other-pm".to_string(),
        appliers: Vec::new(),
        snapshot_callback_id: None,
        events: vec![abi_pb::PmEventEntry {
            input_domain: "orders".to_string(),
            fq_type: FQ_ORDER_CREATED.to_string(),
            callback_id: CB_PM_EVENT,
        }],
        rejections: Vec::new(),
        target_domains: vec!["inventory".to_string()],
        state_callback_id: None,
    }
    .encode_to_vec();
    assert_eq!(
        unsafe { angzarr_router_register_process_manager(r, pm1.as_ptr(), pm1.len(), host_cb) },
        0,
    );
    assert_eq!(
        unsafe { angzarr_router_register_process_manager(r, pm2.as_ptr(), pm2.len(), host_cb) },
        0,
    );
    let router = Router(r);
    let session = next_session();
    let req = pm_request("orders", vec![event_page_of(FQ_ORDER_SHIPPED)]);
    let (ret, bytes) = router.dispatch_process_manager(session, &req);
    assert_eq!(ret, 0, "multi-PM routing should not report NO_HANDLER");
    let resp = pb::ProcessManagerHandleResponse::decode(bytes.as_slice()).expect("PMResponse");
    assert_eq!(
        resp.commands.len(),
        1,
        "routed to the PM declaring OrderShipped; the OrderCreated PM no-opped",
    );
}

#[test]
fn pm_missing_trigger_through_the_abi_is_missing_pm_trigger() {
    // Trigger-shape validation precedes routing (mirrors the saga path): a nil
    // trigger reports MISSING_PM_TRIGGER, not NO_HANDLER from an empty domain.
    let router = Router::with_process_manager();
    let session = next_session();
    let (ret, bytes) =
        router.dispatch_process_manager(session, &pb::ProcessManagerHandleRequest::default());
    assert_eq!(ret, -3, "invalid argument, negated");
    let (_, reason) = decode_status(&bytes);
    assert_eq!(reason, angzarr_router::error::codes::MISSING_PM_TRIGGER);
}

#[test]
fn pm_empty_trigger_through_the_abi_is_empty_pm_trigger() {
    // A trigger book with no pages validates before routing as EMPTY_PM_TRIGGER.
    let router = Router::with_process_manager();
    let session = next_session();
    let req = pb::ProcessManagerHandleRequest {
        trigger: Some(pb::EventBook::default()),
        ..Default::default()
    };
    let (ret, bytes) = router.dispatch_process_manager(session, &req);
    assert_eq!(ret, -3, "invalid argument, negated");
    let (_, reason) = decode_status(&bytes);
    assert_eq!(reason, angzarr_router::error::codes::EMPTY_PM_TRIGGER);
}

/// A second PM ("pm2", domain "other-pm") consuming the same `orders`
/// OrderShipped trigger and compensating the same rejected command.
fn pm2_descriptor_bytes() -> Vec<u8> {
    abi_pb::ProcessManagerDescriptor {
        name: "Pm2".to_string(),
        pm_domain: "other-pm".to_string(),
        appliers: Vec::new(),
        snapshot_callback_id: None,
        events: vec![abi_pb::PmEventEntry {
            input_domain: "orders".to_string(),
            fq_type: FQ_ORDER_SHIPPED.to_string(),
            callback_id: CB_PM2_EVENT,
        }],
        rejections: vec![abi_pb::RejectionEntry {
            compensates: FQ_RESERVE_STOCK.to_string(),
            callback_ids: vec![CB_PM2_COMP],
        }],
        target_domains: vec!["inventory".to_string()],
        state_callback_id: None,
    }
    .encode_to_vec()
}

impl Router {
    fn with_two_process_managers() -> Self {
        let r = angzarr_router_new();
        for desc in [pm_descriptor_bytes(), pm2_descriptor_bytes()] {
            let ret = unsafe {
                angzarr_router_register_process_manager(r, desc.as_ptr(), desc.len(), host_cb)
            };
            assert_eq!(ret, 0, "process-manager registration failed");
        }
        Router(r)
    }
}

fn notification_page(fq_command: &str) -> pb::EventPage {
    pb::EventPage {
        payload: Some(pb::event_page::Payload::Event(notification_command(
            fq_command,
        ))),
        ..Default::default()
    }
}

fn command_domains(resp: &pb::ProcessManagerHandleResponse) -> Vec<String> {
    resp.commands
        .iter()
        .map(|c| {
            c.cover
                .as_ref()
                .map(|c| c.domain.clone())
                .unwrap_or_default()
        })
        .collect()
}

#[test]
fn pm_rejection_addressed_to_its_own_domain_reaches_its_compensator() {
    // The coordinator delivers a PM-issued command's rejection to the PM's own
    // domain, which is not one of its input domains.
    let router = Router::with_process_manager();
    let req = pm_request("order-pm", vec![notification_page(FQ_RESERVE_STOCK)]);
    let (ret, bytes) = router.dispatch_process_manager(next_session(), &req);
    assert_eq!(ret, 0);
    let resp = pb::ProcessManagerHandleResponse::decode(bytes.as_slice()).expect("PMResponse");
    assert_eq!(resp.process_events.len(), 1, "the PM's compensator ran");
    assert!(resp.notification.is_some(), "its escalation crossed back");
}

#[test]
fn co_resident_rejection_reaches_only_the_addressed_pm() {
    let router = Router::with_two_process_managers();
    let req = pm_request("other-pm", vec![notification_page(FQ_RESERVE_STOCK)]);
    let (ret, bytes) = router.dispatch_process_manager(next_session(), &req);
    assert_eq!(ret, 0);
    let resp = pb::ProcessManagerHandleResponse::decode(bytes.as_slice()).expect("PMResponse");
    let domains: Vec<_> = resp
        .process_events
        .iter()
        .map(|b| {
            b.cover
                .as_ref()
                .map(|c| c.domain.clone())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(domains, vec!["pm2".to_string()], "only pm2 compensated");
    assert!(
        resp.notification.is_none(),
        "the order PM's escalation did not run"
    );
}

#[test]
fn co_resident_rejection_on_a_shared_input_domain_runs_no_compensator() {
    let router = Router::with_two_process_managers();
    let req = pm_request("orders", vec![notification_page(FQ_RESERVE_STOCK)]);
    let (ret, bytes) = router.dispatch_process_manager(next_session(), &req);
    assert_eq!(ret, 0);
    let resp = pb::ProcessManagerHandleResponse::decode(bytes.as_slice()).expect("PMResponse");
    assert_eq!(resp, pb::ProcessManagerHandleResponse::default());
}

#[test]
fn process_state_cover_routes_the_trigger_to_its_own_pm() {
    let router = Router::with_two_process_managers();
    let mut req = pm_request("orders", vec![event_page_of(FQ_ORDER_SHIPPED)]);
    req.process_state = Some(pb::EventBook {
        cover: Some(pb::Cover {
            domain: "other-pm".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    });
    let (ret, bytes) = router.dispatch_process_manager(next_session(), &req);
    assert_eq!(ret, 0);
    let resp = pb::ProcessManagerHandleResponse::decode(bytes.as_slice()).expect("PMResponse");
    assert_eq!(command_domains(&resp), vec!["pm2-target".to_string()]);
}

#[test]
fn uncovered_process_state_fans_the_trigger_out_in_registration_order() {
    let router = Router::with_two_process_managers();
    let req = pm_request("orders", vec![event_page_of(FQ_ORDER_SHIPPED)]);
    let (ret, bytes) = router.dispatch_process_manager(next_session(), &req);
    assert_eq!(ret, 0);
    let resp = pb::ProcessManagerHandleResponse::decode(bytes.as_slice()).expect("PMResponse");
    assert_eq!(
        command_domains(&resp),
        vec!["inventory".to_string(), "pm2-target".to_string()]
    );
}

// --- registration-time claims + host fold status

#[test]
fn a_second_claim_of_a_domain_command_is_refused_at_registration() {
    let router = Router::with_counter();
    let mut desc = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
    desc.name = "Rival".to_string();
    desc.commands.truncate(1);
    desc.rejections.clear();
    assert_eq!(
        register_aggregate_desc(router.0, desc),
        -3,
        "a duplicate (domain, command) claim is an invalid argument"
    );
    // The first registration still serves the domain.
    let (ret, _) = router.dispatch(
        next_session(),
        &command_req(FQ_INCREASE_BY, IncreaseBy { n: 1 }.encode_to_vec(), None),
    );
    assert_eq!(ret, 0);
}

#[test]
fn aggregates_for_distinct_domains_both_register() {
    let router = Router::with_counter();
    let mut other = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
    other.domain = "other".to_string();
    let desc = other.encode_to_vec();
    let ret =
        unsafe { angzarr_router_register_aggregate(router.0, desc.as_ptr(), desc.len(), host_cb) };
    assert_eq!(ret, 0);
}

#[test]
fn second_projector_is_refused_at_registration() {
    let router = Router::with_projector();
    let desc = projector_descriptor_bytes();
    let ret =
        unsafe { angzarr_router_register_projector(router.0, desc.as_ptr(), desc.len(), host_cb) };
    assert_eq!(ret, -3, "one projector per router");
    let (ret, _) = router.dispatch_projector(next_session(), &book_in_domain("counter", 2));
    assert_eq!(ret, 0, "the registered projector still claims every book");
}

#[test]
fn projector_fold_failure_keeps_the_host_status() {
    let r = angzarr_router_new();
    let desc = abi_pb::ProjectorDescriptor {
        name: "Rejecting".to_string(),
        domains: vec!["counter".to_string()],
        events: vec![abi_pb::CallbackEntry {
            fq_type: FQ_INCREASED.to_string(),
            callback_id: CB_PROJ_FOLD_REJECTS,
        }],
        finish_callback_id: Some(CB_PROJ_FINISH),
        unknown_callback_id: None,
    }
    .encode_to_vec();
    assert_eq!(
        unsafe { angzarr_router_register_projector(r, desc.as_ptr(), desc.len(), host_cb) },
        0
    );
    let router = Router(r);
    let (ret, bytes) = router.dispatch_projector(next_session(), &book_in_domain("counter", 1));
    assert_eq!(ret, -9, "the host's gRPC code survives");
    let (_, reason) = decode_status(&bytes);
    assert_eq!(reason, "PROJECTION_STALE");
}

// --- host return-code boundaries across every callback kind

fn register_aggregate_desc(r: *mut c_void, desc: abi_pb::AggregateDescriptor) -> i32 {
    let bytes = desc.encode_to_vec();
    unsafe { angzarr_router_register_aggregate(r, bytes.as_ptr(), bytes.len(), host_cb) }
}

/// The counter aggregate with its applier and snapshot loader replaced.
fn counter_with(applier: u64, snapshot: u64) -> Router {
    let mut desc = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
    desc.appliers[0].callback_id = applier;
    desc.snapshot_callback_id = Some(snapshot);
    let r = angzarr_router_new();
    assert_eq!(register_aggregate_desc(r, desc), 0);
    Router(r)
}

fn snapshot_history_book() -> pb::EventBook {
    let mut book = increased_history(1, 2);
    book.snapshot = Some(pb::Snapshot {
        sequence: 0,
        state: Some(Any {
            type_url: "type.googleapis.com/test.counter.CounterState".to_string(),
            value: CounterState { value: 1 }.encode_to_vec(),
        }),
        ..Default::default()
    });
    book
}

fn increase_one(events: Option<pb::EventBook>) -> pb::ContextualCommand {
    command_req(FQ_INCREASE_BY, IncreaseBy { n: 1 }.encode_to_vec(), events)
}

#[test]
fn applier_returning_status_ok_is_success() {
    let router = counter_with(CB_OK_ZERO, CB_OK_ZERO);
    let (ret, _) = router.dispatch(next_session(), &increase_one(Some(increased_history(2, 2))));
    assert_eq!(ret, 0);
}

#[test]
fn snapshot_loader_returning_status_ok_is_success() {
    let router = counter_with(CB_OK_ZERO, CB_OK_ZERO);
    let (ret, _) = router.dispatch(next_session(), &increase_one(Some(snapshot_history_book())));
    assert_eq!(ret, 0);
}

#[test]
fn failing_snapshot_loader_is_data_loss() {
    let router = counter_with(CB_OK_ZERO, CB_FAILS);
    let (ret, bytes) =
        router.dispatch(next_session(), &increase_one(Some(snapshot_history_book())));
    assert_eq!(ret, -15);
    let (_, reason) = decode_status(&bytes);
    assert_eq!(
        reason,
        angzarr_router::error::codes::PERSISTED_EVENT_CORRUPT
    );
}

#[test]
fn compensator_emitting_nothing_is_an_empty_response() {
    let mut desc = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
    desc.rejections[0].callback_ids = vec![CB_OK_EMPTY];
    let r = angzarr_router_new();
    assert_eq!(register_aggregate_desc(r, desc), 0);
    let router = Router(r);
    let mut req = command_req(FQ_RESERVE, Vec::new(), None);
    req.command.as_mut().unwrap().pages[0].payload = Some(pb::command_page::Payload::Command(
        notification_command(FQ_RESERVE),
    ));
    let (ret, bytes) = router.dispatch(next_session(), &req);
    assert_eq!(ret, 0);
    assert_eq!(decode_response(&bytes).result, None);
}

#[test]
fn projector_fold_returning_status_ok_is_success() {
    let mut desc =
        abi_pb::ProjectorDescriptor::decode(projector_descriptor_bytes().as_slice()).unwrap();
    desc.events[0].callback_id = CB_OK_ZERO;
    let bytes = desc.encode_to_vec();
    let r = angzarr_router_new();
    assert_eq!(
        unsafe { angzarr_router_register_projector(r, bytes.as_ptr(), bytes.len(), host_cb) },
        0
    );
    let router = Router(r);
    let (ret, _) = router.dispatch_projector(next_session(), &book_in_domain("counter", 2));
    assert_eq!(ret, 0);
}

fn saga_with(event: u64) -> Router {
    let mut desc = abi_pb::SagaDescriptor::decode(saga_descriptor_bytes().as_slice()).unwrap();
    desc.events[0].callback_id = event;
    let bytes = desc.encode_to_vec();
    let r = angzarr_router_new();
    assert_eq!(
        unsafe { angzarr_router_register_saga(r, bytes.as_ptr(), bytes.len(), host_cb) },
        0
    );
    Router(r)
}

#[test]
fn saga_handler_emitting_nothing_is_an_empty_response() {
    let router = saga_with(CB_OK_EMPTY);
    let req = saga_request("order", vec![event_page_of(FQ_ORDER_CREATED)]);
    let (ret, bytes) = router.dispatch_saga(next_session(), &req);
    assert_eq!(ret, 0);
    assert_eq!(
        pb::SagaResponse::decode(bytes.as_slice()).unwrap(),
        pb::SagaResponse::default()
    );
}

#[test]
fn saga_ignores_a_notification_page() {
    let router = saga_with(CB_OK_EMPTY);
    let req = saga_request("order", vec![notification_page(FQ_RESERVE_STOCK)]);
    let (ret, bytes) = router.dispatch_saga(next_session(), &req);
    assert_eq!(ret, 0);
    assert_eq!(
        pb::SagaResponse::decode(bytes.as_slice()).unwrap(),
        pb::SagaResponse::default()
    );
}

/// The order PM with an applier and snapshot loader for Increased, and its
/// event handler / compensator replaced.
fn pm_with(applier: u64, snapshot: u64, event: u64, comp: u64) -> Router {
    let mut desc =
        abi_pb::ProcessManagerDescriptor::decode(pm_descriptor_bytes().as_slice()).unwrap();
    desc.appliers = vec![abi_pb::CallbackEntry {
        fq_type: FQ_INCREASED.to_string(),
        callback_id: applier,
    }];
    desc.snapshot_callback_id = Some(snapshot);
    desc.events[0].callback_id = event;
    desc.rejections[0].callback_ids = vec![comp];
    let bytes = desc.encode_to_vec();
    let r = angzarr_router_new();
    assert_eq!(
        unsafe { angzarr_router_register_process_manager(r, bytes.as_ptr(), bytes.len(), host_cb) },
        0
    );
    Router(r)
}

fn pm_over_state(state: pb::EventBook) -> pb::ProcessManagerHandleRequest {
    let mut req = pm_request("orders", vec![event_page_of(FQ_ORDER_SHIPPED)]);
    req.process_state = Some(state);
    req
}

#[test]
fn pm_applier_and_loader_succeed_on_either_ok_status() {
    for ok in [CB_OK_ZERO, CB_OK_EMPTY] {
        let router = pm_with(ok, ok, CB_OK_EMPTY, CB_OK_EMPTY);
        let (ret, bytes) = router
            .dispatch_process_manager(next_session(), &pm_over_state(snapshot_history_book()));
        assert_eq!(ret, 0, "callback status {ok} is success");
        assert_eq!(
            pb::ProcessManagerHandleResponse::decode(bytes.as_slice()).unwrap(),
            pb::ProcessManagerHandleResponse::default(),
            "a handler emitting nothing is an empty response"
        );
    }
}

#[test]
fn failing_pm_applier_is_data_loss() {
    let router = pm_with(CB_FAILS, CB_OK_EMPTY, CB_OK_EMPTY, CB_OK_EMPTY);
    let (ret, bytes) =
        router.dispatch_process_manager(next_session(), &pm_over_state(increased_book(1)));
    assert_eq!(ret, -15);
    assert_eq!(
        decode_status(&bytes).1,
        angzarr_router::error::codes::PERSISTED_EVENT_CORRUPT
    );
}

#[test]
fn failing_pm_snapshot_loader_is_data_loss() {
    let router = pm_with(CB_OK_EMPTY, CB_FAILS, CB_OK_EMPTY, CB_OK_EMPTY);
    let (ret, _) =
        router.dispatch_process_manager(next_session(), &pm_over_state(snapshot_history_book()));
    assert_eq!(ret, -15);
}

#[test]
fn pm_compensator_emitting_nothing_is_an_empty_response() {
    let router = pm_with(CB_OK_EMPTY, CB_OK_EMPTY, CB_OK_EMPTY, CB_OK_EMPTY);
    let req = pm_request("order-pm", vec![notification_page(FQ_RESERVE_STOCK)]);
    let (ret, bytes) = router.dispatch_process_manager(next_session(), &req);
    assert_eq!(ret, 0);
    assert_eq!(
        pb::ProcessManagerHandleResponse::decode(bytes.as_slice()).unwrap(),
        pb::ProcessManagerHandleResponse::default()
    );
}

// --- aggregate routing by command domain

/// Two aggregates: "counter" (IncreaseBy emits events) and "other", whose
/// IncreaseBy handler emits nothing.
fn two_aggregates() -> Router {
    let r = angzarr_router_new();
    assert_eq!(
        register_aggregate_desc(
            r,
            abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap()
        ),
        0
    );
    let mut other = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
    other.domain = "other".to_string();
    other.commands[0].callback_id = CB_OK_EMPTY;
    assert_eq!(register_aggregate_desc(r, other), 0);
    Router(r)
}

fn increase_in(domain: &str) -> pb::ContextualCommand {
    let mut req = increase_one(None);
    req.command.as_mut().unwrap().cover.as_mut().unwrap().domain = domain.to_string();
    req
}

fn emitted_pages(bytes: &[u8]) -> usize {
    match decode_response(bytes).result {
        Some(pb::business_response::Result::Events(book)) => book.pages.len(),
        other => panic!("expected events, got {other:?}"),
    }
}

#[test]
fn commands_route_to_the_aggregate_owning_their_domain() {
    let router = two_aggregates();
    let (ret, bytes) = router.dispatch(next_session(), &increase_in("counter"));
    assert_eq!(ret, 0);
    assert_eq!(emitted_pages(&bytes), 1, "the counter aggregate handled it");
    let (ret, bytes) = router.dispatch(next_session(), &increase_in("other"));
    assert_eq!(ret, 0);
    assert_eq!(emitted_pages(&bytes), 0, "the other aggregate handled it");
}

#[test]
fn unclaimed_domain_with_several_aggregates_is_no_handler() {
    let router = two_aggregates();
    let (ret, bytes) = router.dispatch(next_session(), &increase_in("nobody"));
    assert_eq!(ret, -12, "NO_HANDLER_REGISTERED is UNIMPLEMENTED");
    assert_eq!(
        decode_status(&bytes).1,
        angzarr_router::error::codes::NO_HANDLER_REGISTERED
    );
}

#[test]
fn a_sole_aggregate_claims_any_domain() {
    let router = Router::with_counter();
    let (ret, bytes) = router.dispatch(next_session(), &increase_in("nobody"));
    assert_eq!(ret, 0);
    assert_eq!(emitted_pages(&bytes), 1);
}

// --- ABI v2 surface: undo, facts, replay, context aux

impl Router {
    fn call(
        &self,
        f: unsafe extern "C" fn(*mut c_void, *mut c_void, *const u8, usize, *mut AngzarrBuf) -> i32,
        session: usize,
        bytes: &[u8],
    ) -> (i32, Vec<u8>) {
        let mut out = AngzarrBuf {
            data: std::ptr::null_mut(),
            len: 0,
        };
        let ret = unsafe {
            f(
                self.0,
                session as *mut c_void,
                bytes.as_ptr(),
                bytes.len(),
                &mut out,
            )
        };
        let response = if out.data.is_null() {
            Vec::new()
        } else {
            let copied = unsafe { std::slice::from_raw_parts(out.data, out.len) }.to_vec();
            unsafe { angzarr_buf_release(out.data, out.len) };
            copied
        };
        (ret, response)
    }
}

/// The counter aggregate plus an undo for Reserve, a fact handler for
/// Increased, and the state packer.
fn full_counter() -> Router {
    let mut desc = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
    desc.undoes = vec![abi_pb::CallbackEntry {
        fq_type: FQ_RESERVE.to_string(),
        callback_id: CB_UNDO_RESERVE,
    }];
    desc.facts = vec![abi_pb::CallbackEntry {
        fq_type: FQ_INCREASED.to_string(),
        callback_id: CB_FACT_ANNOTATE,
    }];
    desc.state_callback_id = Some(CB_PACK_STATE);
    let r = angzarr_router_new();
    assert_eq!(register_aggregate_desc(r, desc), 0);
    Router(r)
}

fn compensate_command(command_type: &str, sequences: Vec<u32>) -> pb::ContextualCommand {
    let notification = pb::Notification {
        payload: Some(Any {
            type_url: "type.googleapis.com/io.angzarr.v1.Compensate".to_string(),
            value: pb::Compensate {
                command_type: command_type.to_string(),
                sequences,
                reason: "aborted".to_string(),
            }
            .encode_to_vec(),
        }),
        ..Default::default()
    };
    let mut req = command_req(FQ_RESERVE, Vec::new(), Some(increased_history(1, 3)));
    req.command.as_mut().unwrap().pages[0].payload =
        Some(pb::command_page::Payload::Command(Any {
            type_url: "type.googleapis.com/io.angzarr.v1.Notification".to_string(),
            value: notification.encode_to_vec(),
        }));
    req
}

#[test]
fn compensate_runs_the_undo_handler_through_the_abi() {
    let router = full_counter();
    let (ret, bytes) = router.dispatch(next_session(), &compensate_command(FQ_RESERVE, vec![1, 2]));
    assert_eq!(ret, 0);
    let book = match decode_response(&bytes).result {
        Some(pb::business_response::Result::Events(book)) => book,
        other => panic!("expected events, got {other:?}"),
    };
    let seqs: Vec<_> = book
        .pages
        .iter()
        .map(
            |p| match p.header.as_ref().and_then(|h| h.sequence_type.as_ref()) {
                Some(pb::page_header::SequenceType::Sequence(s)) => *s,
                _ => panic!("undo events are stamped"),
            },
        )
        .collect();
    assert_eq!(
        seqs,
        vec![3, 4],
        "one event per undone sequence, after history"
    );
}

#[test]
fn compensate_with_no_undo_handler_is_unimplemented_through_the_abi() {
    let router = full_counter();
    let (ret, bytes) = router.dispatch(
        next_session(),
        &compensate_command("test.counter.CountStock", vec![]),
    );
    assert_eq!(ret, -12);
    assert_eq!(
        decode_status(&bytes).1,
        angzarr_router::error::codes::NO_UNDO_HANDLER
    );
}

#[test]
fn command_handler_sees_its_cover_through_the_abi() {
    let router = Router::with_counter();
    let session = next_session();
    let mut req = command_req(FQ_INCREASE_BY, IncreaseBy { n: 1 }.encode_to_vec(), None);
    req.command.as_mut().unwrap().cover.as_mut().unwrap().root = Some(pb::Uuid { value: vec![4] });
    let (ret, _) = router.dispatch(session, &req);
    assert_eq!(ret, 0);
    assert_eq!(
        session_snapshot(session).observed_covers,
        vec![req.command.unwrap().cover]
    );
}

#[test]
fn projector_fold_sees_its_page_place_through_the_abi() {
    let router = Router::with_projector();
    let session = next_session();
    let mut book = book_in_domain("counter", 2);
    book.cover.as_mut().unwrap().root = Some(pb::Uuid { value: vec![6] });
    book.pages[1].header = Some(pb::PageHeader {
        sequence_type: Some(pb::page_header::SequenceType::Sequence(9)),
        ..Default::default()
    });
    let (ret, _) = router.dispatch_projector(session, &book);
    assert_eq!(ret, 0);
    assert_eq!(
        session_snapshot(session).observed_pages,
        vec![(book.cover.clone(), 0), (book.cover.clone(), 9)]
    );
}

#[test]
fn facts_are_annotated_against_folded_state_through_the_abi() {
    let router = full_counter();
    let mut facts = book_in_domain("counter", 2);
    facts.pages[0].header = Some(pb::PageHeader {
        sequence_type: Some(pb::page_header::SequenceType::ExternalDeferred(
            pb::ExternalDeferredSequence {
                external_id: "ext-1".to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    });
    let req = pb::FactRequest {
        facts: Some(facts.clone()),
        prior_events: Some(increased_history(3, 3)),
    };
    let (ret, bytes) = router.call(
        angzarr_router_dispatch_fact,
        next_session(),
        &req.encode_to_vec(),
    );
    assert_eq!(ret, 0);
    let out = pb::EventBook::decode(bytes.as_slice()).expect("EventBook");
    assert_eq!(out.cover, facts.cover, "the facts' cover is kept");
    assert_eq!(
        out.pages[0].header, facts.pages[0].header,
        "headers are kept"
    );
    let recorded: Vec<(String, Option<u32>)> = out
        .pages
        .iter()
        .map(|p| match p.payload.as_ref() {
            Some(pb::event_page::Payload::Event(any)) => {
                let name = angzarr_router::type_name_from_url(&any.type_url).to_string();
                let counter = name
                    .ends_with("CounterState")
                    .then(|| CounterState::decode(any.value.as_slice()).unwrap().value);
                (name, counter)
            }
            _ => panic!("a recorded page lost its event"),
        })
        .collect();
    // Each annotation replaces its Increased fact (no fold); each flag is an
    // Increased that folds, so the second fact sees one more event.
    assert_eq!(
        recorded,
        vec![
            ("test.counter.CounterState".to_string(), Some(3)),
            (FQ_INCREASED.to_string(), None),
            ("test.counter.CounterState".to_string(), Some(4)),
            (FQ_INCREASED.to_string(), None),
        ]
    );
    assert_eq!(out.pages[1].header, None, "a flag carries no header");
}

#[test]
fn facts_route_by_their_cover_domain_through_the_abi() {
    let r = angzarr_router_new();
    assert_eq!(
        register_aggregate_desc(
            r,
            abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap()
        ),
        0
    );
    let mut other = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
    other.domain = "other".to_string();
    other.facts = vec![abi_pb::CallbackEntry {
        fq_type: FQ_INCREASED.to_string(),
        callback_id: CB_FACT_ANNOTATE,
    }];
    assert_eq!(register_aggregate_desc(r, other), 0);
    let router = Router(r);
    let annotated = |domain: &str| {
        let req = pb::FactRequest {
            facts: Some(book_in_domain(domain, 1)),
            prior_events: None,
        };
        let (ret, bytes) = router.call(
            angzarr_router_dispatch_fact,
            next_session(),
            &req.encode_to_vec(),
        );
        assert_eq!(ret, 0);
        let out = pb::EventBook::decode(bytes.as_slice()).unwrap();
        match out.pages[0].payload.as_ref() {
            Some(pb::event_page::Payload::Event(any)) => any.type_url.ends_with("CounterState"),
            _ => false,
        }
    };
    assert!(annotated("other"), "other's fact handler ran");
    let req = pb::FactRequest {
        facts: Some(book_in_domain("counter", 1)),
        prior_events: None,
    };
    let (ret, bytes) = router.call(
        angzarr_router_dispatch_fact,
        next_session(),
        &req.encode_to_vec(),
    );
    assert_eq!(ret, -3, "counter declares no fact: INVALID_ARGUMENT");
    assert_eq!(
        decode_status(&bytes).1,
        angzarr_router::error::codes::NO_FACT_HANDLER
    );
}

#[test]
fn replay_returns_the_packed_state_through_the_abi() {
    let router = full_counter();
    let call = abi_pb::ReplayCall {
        domain: "counter".to_string(),
        request: Some(pb::ReplayRequest {
            base_snapshot: Some(pb::Snapshot {
                sequence: 1,
                state: Some(Any {
                    type_url: "type.googleapis.com/test.counter.CounterState".to_string(),
                    value: CounterState { value: 10 }.encode_to_vec(),
                }),
                ..Default::default()
            }),
            events: increased_book(2).pages,
        }),
    };
    let (ret, bytes) = router.call(
        angzarr_router_dispatch_replay,
        next_session(),
        &call.encode_to_vec(),
    );
    assert_eq!(ret, 0);
    let resp = pb::ReplayResponse::decode(bytes.as_slice()).expect("ReplayResponse");
    let state = CounterState::decode(resp.state.unwrap().value.as_slice()).unwrap();
    assert_eq!(state.value, 12, "snapshot 10 + two unsequenced events");
}

#[test]
fn replay_without_a_state_packer_is_unimplemented_through_the_abi() {
    let router = Router::with_counter();
    let call = abi_pb::ReplayCall {
        domain: "counter".to_string(),
        request: Some(pb::ReplayRequest::default()),
    };
    let (ret, bytes) = router.call(
        angzarr_router_dispatch_replay,
        next_session(),
        &call.encode_to_vec(),
    );
    assert_eq!(ret, -12);
    assert_eq!(
        decode_status(&bytes).1,
        angzarr_router::error::codes::NO_HANDLER_REGISTERED
    );
}

#[test]
fn pm_compensator_commands_cross_back_deferred() {
    let router = pm_with(CB_OK_EMPTY, CB_OK_EMPTY, CB_OK_EMPTY, CB_PM_COMP_COMMANDS);
    let req = pm_request("order-pm", vec![notification_page(FQ_RESERVE_STOCK)]);
    let (ret, bytes) = router.dispatch_process_manager(next_session(), &req);
    assert_eq!(ret, 0);
    let resp = pb::ProcessManagerHandleResponse::decode(bytes.as_slice()).unwrap();
    assert_eq!(resp.commands.len(), 1);
    assert!(matches!(
        resp.commands[0].pages[0]
            .header
            .as_ref()
            .and_then(|h| h.sequence_type.as_ref()),
        Some(pb::page_header::SequenceType::AngzarrDeferred(_))
    ));
}

#[test]
fn ambiguous_compensates_entries_are_refused_at_registration() {
    let mut desc = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
    desc.rejections.push(abi_pb::RejectionEntry {
        compensates: format!("inventory:{FQ_RESERVE}"),
        callback_ids: vec![CB_COMP_A],
    });
    let r = angzarr_router_new();
    assert_eq!(register_aggregate_desc(r, desc), -3);
    let mut pm =
        abi_pb::ProcessManagerDescriptor::decode(pm_descriptor_bytes().as_slice()).unwrap();
    pm.rejections.push(abi_pb::RejectionEntry {
        compensates: format!("inventory:{FQ_RESERVE_STOCK}"),
        callback_ids: vec![CB_PM_COMP],
    });
    let bytes = pm.encode_to_vec();
    assert_eq!(
        unsafe { angzarr_router_register_process_manager(r, bytes.as_ptr(), bytes.len(), host_cb) },
        -3
    );
    unsafe { angzarr_router_free(r) };
}

#[test]
fn undo_and_fact_handlers_returning_nothing_cross_cleanly() {
    let mut desc = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
    desc.undoes = vec![abi_pb::CallbackEntry {
        fq_type: FQ_RESERVE.to_string(),
        callback_id: CB_OK_EMPTY,
    }];
    desc.facts = vec![abi_pb::CallbackEntry {
        fq_type: FQ_INCREASED.to_string(),
        callback_id: CB_OK_EMPTY,
    }];
    let r = angzarr_router_new();
    assert_eq!(register_aggregate_desc(r, desc), 0);
    let router = Router(r);

    let (ret, bytes) = router.dispatch(next_session(), &compensate_command(FQ_RESERVE, vec![1]));
    assert_eq!(ret, 0, "an undo handler returning nothing succeeds");
    assert_eq!(decode_response(&bytes).result, None);

    let facts = book_in_domain("counter", 1);
    let req = pb::FactRequest {
        facts: Some(facts.clone()),
        prior_events: None,
    };
    let (ret, bytes) = router.call(
        angzarr_router_dispatch_fact,
        next_session(),
        &req.encode_to_vec(),
    );
    assert_eq!(
        ret, 0,
        "a fact handler returning nothing records the fact unchanged"
    );
    assert_eq!(
        pb::EventBook::decode(bytes.as_slice()).unwrap().pages,
        facts.pages
    );
}

#[test]
fn a_failing_state_packer_fails_replay_with_its_status() {
    let mut desc = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
    desc.state_callback_id = Some(CB_PROJ_FOLD_REJECTS);
    let r = angzarr_router_new();
    assert_eq!(register_aggregate_desc(r, desc), 0);
    let router = Router(r);
    let call = abi_pb::ReplayCall {
        domain: "counter".to_string(),
        request: Some(pb::ReplayRequest::default()),
    };
    let (ret, bytes) = router.call(
        angzarr_router_dispatch_replay,
        next_session(),
        &call.encode_to_vec(),
    );
    assert_eq!(ret, -9);
    assert_eq!(decode_status(&bytes).1, "PROJECTION_STALE");
}

// --- applier page context + PM replay

#[test]
fn appliers_see_their_page_place_through_the_abi() {
    let router = Router::with_counter();
    let session = next_session();
    let mut history = increased_history(2, 2);
    history.cover = Some(pb::Cover {
        domain: "counter".to_string(),
        root: Some(pb::Uuid { value: vec![5] }),
        ..Default::default()
    });
    history.pages[0].header = Some(pb::PageHeader {
        sequence_type: Some(pb::page_header::SequenceType::Sequence(0)),
        ..Default::default()
    });
    history.pages[1].header = Some(pb::PageHeader {
        sequence_type: Some(pb::page_header::SequenceType::Sequence(1)),
        ..Default::default()
    });
    let (ret, _) = router.dispatch(
        session,
        &command_req(
            FQ_INCREASE_BY,
            IncreaseBy { n: 1 }.encode_to_vec(),
            Some(history.clone()),
        ),
    );
    assert_eq!(ret, 0);
    assert_eq!(
        session_snapshot(session).applied_pages,
        vec![(history.cover.clone(), 0), (history.cover.clone(), 1)]
    );
}

/// The order PM with an Increased applier and the state packer.
fn replayable_pm() -> Router {
    let mut desc =
        abi_pb::ProcessManagerDescriptor::decode(pm_descriptor_bytes().as_slice()).unwrap();
    desc.appliers = vec![abi_pb::CallbackEntry {
        fq_type: FQ_INCREASED.to_string(),
        callback_id: CB_APPLIER,
    }];
    desc.state_callback_id = Some(CB_PACK_STATE);
    let bytes = desc.encode_to_vec();
    let r = angzarr_router_new();
    assert_eq!(
        unsafe { angzarr_router_register_process_manager(r, bytes.as_ptr(), bytes.len(), host_cb) },
        0
    );
    Router(r)
}

#[test]
fn process_manager_replay_packs_its_state_through_the_abi() {
    let router = replayable_pm();
    let call = abi_pb::ReplayCall {
        domain: "order-pm".to_string(),
        request: Some(pb::ReplayRequest {
            base_snapshot: None,
            events: increased_book(3).pages,
        }),
    };
    let (ret, bytes) = router.call(
        angzarr_router_dispatch_replay,
        next_session(),
        &call.encode_to_vec(),
    );
    assert_eq!(ret, 0);
    let resp = pb::ReplayResponse::decode(bytes.as_slice()).expect("ReplayResponse");
    assert_eq!(
        CounterState::decode(resp.state.unwrap().value.as_slice())
            .unwrap()
            .value,
        3
    );
}

#[test]
fn replay_for_an_unknown_domain_is_no_handler() {
    let router = replayable_pm();
    let call = abi_pb::ReplayCall {
        domain: "nobody".to_string(),
        request: Some(pb::ReplayRequest::default()),
    };
    let (ret, bytes) = router.call(
        angzarr_router_dispatch_replay,
        next_session(),
        &call.encode_to_vec(),
    );
    assert_eq!(ret, -12);
    assert_eq!(
        decode_status(&bytes).1,
        angzarr_router::error::codes::NO_HANDLER_REGISTERED
    );
}

#[test]
fn process_manager_without_a_state_packer_refuses_replay() {
    let router = Router::with_process_manager();
    let call = abi_pb::ReplayCall {
        domain: "order-pm".to_string(),
        request: Some(pb::ReplayRequest::default()),
    };
    let (ret, _) = router.call(
        angzarr_router_dispatch_replay,
        next_session(),
        &call.encode_to_vec(),
    );
    assert_eq!(ret, -12);
}

// --- several aggregates in one domain (C-0010, C-0042) ----------------------

const FQ_AUDIT: &str = "test.counter.Audit";

/// The counter aggregate plus a second "counter" aggregate handling Audit
/// (empty reply) and compensating Reserve with comp-b.
fn counter_and_auditor() -> Router {
    let router = Router::with_counter();
    let auditor = abi_pb::AggregateDescriptor {
        name: "Auditor".to_string(),
        domain: "counter".to_string(),
        commands: vec![abi_pb::CallbackEntry {
            fq_type: FQ_AUDIT.to_string(),
            callback_id: CB_OK_EMPTY,
        }],
        rejections: vec![abi_pb::RejectionEntry {
            compensates: FQ_RESERVE.to_string(),
            callback_ids: vec![CB_COMP_B],
        }],
        ..Default::default()
    };
    assert_eq!(register_aggregate_desc(router.0, auditor), 0);
    router
}

fn reserve_rejection_delivery() -> pb::ContextualCommand {
    pb::ContextualCommand {
        command: Some(pb::CommandBook {
            cover: Some(pb::Cover {
                domain: "counter".to_string(),
                ..Default::default()
            }),
            pages: vec![pb::CommandPage {
                payload: Some(pb::command_page::Payload::Command(notification_command(
                    FQ_RESERVE,
                ))),
                ..Default::default()
            }],
        }),
        events: Some(pb::EventBook {
            next_sequence: 5,
            ..Default::default()
        }),
    }
}

#[test]
fn aggregates_sharing_a_domain_route_commands_by_type() {
    let router = counter_and_auditor();
    let (ret, bytes) = router.dispatch(next_session(), &increase_in("counter"));
    assert_eq!(ret, 0);
    assert_eq!(
        emitted_pages(&bytes),
        1,
        "the counter aggregate handled IncreaseBy"
    );
    let (ret, bytes) = router.dispatch(next_session(), &command_req(FQ_AUDIT, Vec::new(), None));
    assert_eq!(ret, 0);
    assert_eq!(emitted_pages(&bytes), 0, "the auditor handled Audit");
}

#[test]
fn every_aggregate_of_a_domain_compensating_a_rejection_runs_in_order() {
    let router = counter_and_auditor();
    let session = next_session();
    let (ret, bytes) = router.dispatch(session, &reserve_rejection_delivery());
    assert_eq!(ret, 0);
    assert_eq!(
        session_snapshot(session).markers,
        vec!["comp-a", "comp-b", "comp-b"],
        "the counter's compensators, then the auditor's"
    );
    let Some(pb::business_response::Result::Events(book)) = decode_response(&bytes).result else {
        panic!("expected merged events");
    };
    let sequences: Vec<u32> = book
        .pages
        .iter()
        .map(
            |p| match p.header.as_ref().and_then(|h| h.sequence_type.as_ref()) {
                Some(pb::page_header::SequenceType::Sequence(s)) => *s,
                other => panic!("no sequence: {other:?}"),
            },
        )
        .collect();
    assert_eq!(
        sequences,
        vec![5, 6, 7],
        "sequences continue across aggregates"
    );
}

#[test]
fn replay_of_a_shared_domain_goes_to_its_first_aggregate() {
    let r = angzarr_router_new();
    let mut counter = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
    counter.state_callback_id = Some(CB_PACK_STATE);
    assert_eq!(register_aggregate_desc(r, counter), 0);
    let auditor = abi_pb::AggregateDescriptor {
        name: "Auditor".to_string(),
        domain: "counter".to_string(),
        commands: vec![abi_pb::CallbackEntry {
            fq_type: FQ_AUDIT.to_string(),
            callback_id: CB_OK_EMPTY,
        }],
        ..Default::default()
    };
    assert_eq!(register_aggregate_desc(r, auditor), 0);
    let router = Router(r);
    let call = abi_pb::ReplayCall {
        domain: "counter".to_string(),
        request: Some(pb::ReplayRequest {
            events: increased_history(2, 2).pages,
            ..Default::default()
        }),
    };
    let (ret, bytes) = router.call(
        angzarr_router_dispatch_replay,
        next_session(),
        &call.encode_to_vec(),
    );
    assert_eq!(ret, 0, "the counter (registered first) packs its state");
    let resp = pb::ReplayResponse::decode(bytes.as_slice()).expect("ReplayResponse");
    let state = CounterState::decode(resp.state.expect("state").value.as_slice()).unwrap();
    assert_eq!(state.value, 2, "the counter's appliers folded both events");
}

#[test]
fn a_compensate_reaches_only_the_aggregate_of_its_domain_undoing_it() {
    let router = Router::with_counter();
    let undoer = abi_pb::AggregateDescriptor {
        name: "Undoer".to_string(),
        domain: "counter".to_string(),
        appliers: vec![abi_pb::CallbackEntry {
            fq_type: FQ_INCREASED.to_string(),
            callback_id: CB_APPLIER,
        }],
        undoes: vec![abi_pb::CallbackEntry {
            fq_type: FQ_RESERVE.to_string(),
            callback_id: CB_UNDO_RESERVE,
        }],
        ..Default::default()
    };
    assert_eq!(register_aggregate_desc(router.0, undoer), 0);
    let (ret, bytes) = router.dispatch(next_session(), &compensate_command(FQ_RESERVE, vec![1]));
    assert_eq!(ret, 0, "the counter (no undo for Reserve) never runs");
    assert_eq!(emitted_pages(&bytes), 1);
}

// --- fact records (ComponentOptions.facts) -----------------------------------

fn counter_with_fact_handler(callback_id: u64) -> Router {
    let mut desc = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
    desc.facts = vec![abi_pb::CallbackEntry {
        fq_type: FQ_INCREASED.to_string(),
        callback_id,
    }];
    let r = angzarr_router_new();
    assert_eq!(register_aggregate_desc(r, desc), 0);
    Router(r)
}

fn handle_facts(router: &Router, facts: pb::EventBook) -> (i32, Vec<u8>) {
    let req = pb::FactRequest {
        facts: Some(facts),
        prior_events: None,
    };
    router.call(
        angzarr_router_dispatch_fact,
        next_session(),
        &req.encode_to_vec(),
    )
}

#[test]
fn a_fact_record_without_a_fact_records_the_fact_as_received() {
    let router = counter_with_fact_handler(CB_FACT_KEEP);
    let facts = book_in_domain("counter", 1);
    let (ret, bytes) = handle_facts(&router, facts.clone());
    assert_eq!(ret, 0);
    assert_eq!(
        pb::EventBook::decode(bytes.as_slice()).unwrap().pages,
        facts.pages
    );
}

#[test]
fn an_undecodable_fact_record_is_an_unhandled_error() {
    let router = counter_with_fact_handler(CB_FACT_GARBAGE);
    let (ret, bytes) = handle_facts(&router, book_in_domain("counter", 1));
    assert_eq!(ret, -13, "INTERNAL");
    assert_eq!(
        decode_status(&bytes).1,
        angzarr_router::error::codes::UNHANDLED_HANDLER_ERROR
    );
}

#[test]
fn an_undeclared_fact_never_reaches_the_host() {
    let router = counter_with_fact_handler(CB_FACT_ANNOTATE);
    let session = next_session();
    let mut facts = book_in_domain("counter", 1);
    facts.pages.push(event_page_of(FQ_RESERVE));
    let req = pb::FactRequest {
        facts: Some(facts),
        prior_events: Some(increased_history(2, 2)),
    };
    let (ret, bytes) = router.call(angzarr_router_dispatch_fact, session, &req.encode_to_vec());
    assert_eq!(ret, -3, "INVALID_ARGUMENT");
    assert_eq!(
        decode_status(&bytes).1,
        angzarr_router::error::codes::NO_FACT_HANDLER
    );
    assert_eq!(
        session_snapshot(session).counter,
        0,
        "refused before any applier or fact handler ran"
    );
}

#[test]
fn a_saga_declaring_a_rejection_handler_is_refused_at_registration() {
    let r = angzarr_router_new();
    let mut desc = abi_pb::SagaDescriptor::decode(saga_descriptor_bytes().as_slice()).unwrap();
    desc.rejections = vec![abi_pb::RejectionEntry {
        compensates: FQ_RESERVE_STOCK.to_string(),
        callback_ids: vec![CB_SAGA_COMP],
    }];
    let bytes = desc.encode_to_vec();
    let ret = unsafe { angzarr_router_register_saga(r, bytes.as_ptr(), bytes.len(), host_cb) };
    assert_eq!(ret, -3, "INVALID_ARGUMENT");
    let router = Router(r);
    let (ret, _) = router.dispatch_saga(
        next_session(),
        &saga_request("order", vec![event_page_of(FQ_ORDER_CREATED)]),
    );
    assert_eq!(ret, -12, "no saga was registered");
}

#[test]
fn a_saga_refusal_names_the_code_and_the_saga() {
    let mut router = registry::FfiRouter::new();
    let mut desc = abi_pb::SagaDescriptor::decode(saga_descriptor_bytes().as_slice()).unwrap();
    desc.rejections = vec![abi_pb::RejectionEntry {
        compensates: FQ_RESERVE_STOCK.to_string(),
        callback_ids: vec![CB_SAGA_COMP],
    }];
    let err = router
        .register_saga(&desc.encode_to_vec(), host_cb)
        .unwrap_err();
    assert_eq!(err.code, angzarr_router::error::codes::SAGA_COMPENSATES);
    assert_eq!(
        err.extras.get("saga").map(String::as_str),
        Some("OrderFulfillment")
    );
}
