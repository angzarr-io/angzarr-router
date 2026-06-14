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
use crate::proto::io::angzarr::router::ffi::v1 as abi_pb;
use crate::proto::google::rpc as rpc_pb;

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

// --- host-side session registry (state never crosses the boundary)

#[derive(Default, Clone)]
struct Session {
    counter: u32,
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
            with_session(key, |s| s.counter += 1);
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
                s.observed_cctx.push((cctx.next_sequence, cctx.had_prior_events))
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
        CB_FAIL_HARD => -13, // plain failure, no status payload
        CB_RETURN_NOTHING => STATUS_OK_EMPTY,
        CB_COMP_A | CB_COMP_B => {
            let raux = abi_pb::RejectionAux::decode(aux).expect("rejection aux");
            // The aux must round-trip the framework shapes.
            pb::Notification::decode(raux.notification.as_slice()).expect("notification");
            pb::RejectionNotification::decode(raux.rejection.as_slice()).expect("rejection");
            with_session(key, |s| {
                s.markers
                    .push(if callback_id == CB_COMP_A { "comp-a" } else { "comp-b" })
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
            fq_command_type: FQ_RESERVE.to_string(),
            callback_ids: vec![CB_COMP_A, CB_COMP_B],
        }],
        snapshot_callback_id: Some(CB_SNAPSHOT),
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
fn abi_version_is_one() {
    assert_eq!(angzarr_abi_version(), 1);
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
    let seqs: Vec<u32> = book.pages.iter().map(angzarr_router::page_sequence).collect();
    assert_eq!(seqs, vec![0, 1], "fill-only stamping from next_sequence 0");

    let s = session_snapshot(session);
    assert_eq!(s.observed_cctx, vec![(0, false)], "fresh aggregate evidence");
    assert_eq!(s.counter, 0, "no history — appliers must not run");
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
    assert_eq!(s.observed_cctx, vec![(2, true)], "historical-state evidence");

    let resp = decode_response(&bytes);
    let Some(pb::business_response::Result::Events(book)) = resp.result else {
        panic!("expected events result");
    };
    assert_eq!(
        book.pages.iter().map(angzarr_router::page_sequence).collect::<Vec<_>>(),
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
    let (ret, _) = router.dispatch(
        next_session(),
        &command_req(FQ_FAIL_HARD, Vec::new(), None),
    );
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
            type_url: "type.googleapis.com/io.angzarr.v1.RejectionNotification"
                .to_string(),
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
