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

/// The fold over router-owned CounterState: one per Increased page.
const CB_FOLD: u64 = 1;
const CB_INCREASE_BY: u64 = 2;
const CB_FAIL_HARD: u64 = 3;
const CB_RETURN_NOTHING: u64 = 6;
const CB_SAGA_EVENT: u64 = 10;
const CB_PM_EVENT: u64 = 12;
const CB_PM2_EVENT: u64 = 14;
const CB_PROJ_FOLD_REJECTS: u64 = 16;
/// Succeeds with STATUS_OK and no output (an applier/fold that returns 0).
const CB_OK_ZERO: u64 = 17;
/// Succeeds with STATUS_OK_EMPTY and no output (a handler emitting nothing).
const CB_OK_EMPTY: u64 = 18;
/// Fails with -13 and no status payload.
const CB_FAILS: u64 = 19;
const CB_FACT_ANNOTATE: u64 = 21;
const CB_FACT_KEEP: u64 = 24;
const CB_FACT_GARBAGE: u64 = 25;
/// IncreaseBy that never resolves the conflict window.
const CB_INCREASE_IGNORES: u64 = 26;
/// IncreaseBy that resolves with a made-up nonce.
const CB_INCREASE_MADE_UP: u64 = 27;
/// changed(): ["value"] when the counters differ.
const CB_CHANGED_VALUE: u64 = 29;
/// changed(): always empty.
const CB_CHANGED_NONE: u64 = 30;
/// Upcasts IncreaseByV1 to IncreaseBy (payload unchanged).
const CB_UPCAST_V1: u64 = 31;

const FQ_INCREASE_BY_V1: &str = "test.counter.IncreaseByV1";

const FQ_ORDER_CREATED: &str = "test.order.OrderCreated";
const FQ_RESERVE_STOCK: &str = "test.order.ReserveStock";
const FQ_ORDER_SHIPPED: &str = "test.order.OrderShipped";

// --- host-side session registry (state never crosses the boundary)

#[derive(Default, Clone)]
struct Session {
    counter: u32,
    observed_covers: Vec<Option<pb::Cover>>,
    observed_pages: Vec<(Option<pb::Cover>, u64)>,
    applied_pages: Vec<(Option<pb::Cover>, u64)>,
    observed_cctx: Vec<(u64, bool)>,
    /// (expected_sequence, conflict_resolution, window sequences, nonce).
    observed_handling: Vec<(u64, i32, Vec<u64>, Vec<u8>)>,
    /// (before, after) counters each changed() call compared.
    changed_calls: Vec<(u32, u32)>,
    /// The router-owned state value each decide / fact / PM handler saw.
    observed_state: Vec<u32>,
    /// The type URL of each state a decide was handed.
    observed_state_type: Vec<String>,
    /// The page sequences of each fold call.
    fold_calls: Vec<Vec<u64>>,
}

const STATE_TYPE: &str = "test.counter.CounterState";

/// The CounterState value of a router-owned state Any (0 for the empty
/// default).
fn state_value(state: Option<&Any>) -> u32 {
    state.map_or(0, |any| {
        CounterState::decode(any.value.as_slice())
            .expect("CounterState")
            .value
    })
}

fn state_any(value: u32) -> Any {
    Any {
        type_url: format!("/{STATE_TYPE}"),
        value: CounterState { value }.encode_to_vec(),
    }
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

/// `n` Increased events at sequences 0...
fn increased_book(n: u32) -> pb::EventBook {
    pb::EventBook {
        pages: (0..n)
            .map(|seq| pb::EventPage {
                payload: Some(Any {
                    type_url: format!("type.googleapis.com/{FQ_INCREASED}"),
                    value: Increased {}.encode_to_vec(),
                }),
                sequence: u64::from(seq),
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
        CB_FOLD => {
            // Stateless: fold every Increased page into the state handed in.
            let call = abi_pb::FoldCall::decode(payload).expect("FoldCall");
            let mut value = state_value(call.state.as_ref());
            for page in &call.pages {
                let Some(event) = page.payload.as_ref() else {
                    continue;
                };
                if !event.type_url.ends_with(FQ_INCREASED) {
                    continue;
                }
                if Increased::decode(event.value.as_slice()).is_err() {
                    return -3;
                }
                value += 1;
                with_session(key, |s| {
                    s.applied_pages.push((call.cover.clone(), page.sequence))
                });
            }
            with_session(key, |s| {
                s.fold_calls
                    .push(call.pages.iter().map(|p| p.sequence).collect())
            });
            host_fill(out, &state_any(value).encode_to_vec());
            STATUS_OK
        }
        CB_INCREASE_BY | CB_INCREASE_IGNORES | CB_INCREASE_MADE_UP => {
            let cctx = abi_pb::CommandContextAux::decode(aux).expect("cctx aux");
            with_session(key, |s| {
                s.observed_state.push(state_value(cctx.state.as_ref()));
                s.observed_state_type.push(
                    cctx.state
                        .as_ref()
                        .map(|a| a.type_url.clone())
                        .unwrap_or_default(),
                );
                s.observed_cctx
                    .push((cctx.next_sequence, cctx.had_prior_events));
                s.observed_covers.push(cctx.cover.clone());
                s.observed_handling.push((
                    cctx.expected_sequence,
                    cctx.conflict_resolution,
                    cctx.window.iter().map(|p| p.sequence).collect(),
                    cctx.conflict_nonce.clone(),
                ));
            });
            let cmd = IncreaseBy::decode(payload).expect("IncreaseBy");
            if cmd.n == 0 {
                host_fill(
                    out,
                    &rejection_status(9, "VALUE_NOT_POSITIVE", "value must be positive"),
                );
                return -9;
            }
            let resolved_nonce = match callback_id {
                CB_INCREASE_IGNORES => Vec::new(),
                CB_INCREASE_MADE_UP => b"made-up".to_vec(),
                _ => cctx.conflict_nonce.clone(),
            };
            let output = abi_pb::CommandOutput {
                events: Some(increased_book(cmd.n)),
                resolved_nonce,
            };
            host_fill(out, &output.encode_to_vec());
            STATUS_OK
        }
        CB_CHANGED_VALUE | CB_CHANGED_NONE => {
            let call = abi_pb::ChangedCall::decode(payload).expect("ChangedCall");
            let value = |any: Option<Any>| {
                CounterState::decode(any.expect("a state image").value.as_slice())
                    .expect("CounterState")
                    .value
            };
            let (before, after) = (value(call.before), value(call.after));
            with_session(key, |s| s.changed_calls.push((before, after)));
            let units = if callback_id == CB_CHANGED_VALUE && before != after {
                vec!["value".to_string()]
            } else {
                Vec::new()
            };
            host_fill(out, &abi_pb::ChangedUnits { units }.encode_to_vec());
            STATUS_OK
        }
        CB_UPCAST_V1 => {
            let upcast = Any {
                type_url: format!("type.googleapis.com/{FQ_INCREASE_BY}"),
                value: payload.to_vec(),
            };
            host_fill(out, &upcast.encode_to_vec());
            STATUS_OK
        }
        CB_PROJ_FOLD_REJECTS => {
            host_fill(
                out,
                &rejection_status(9, "PROJECTION_STALE", "projection is stale"),
            );
            -9
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
                page: Some(pb::CommandPage {
                    body: Some(pb::command_page::Body::Command(pb::Command {
                        payload: Some(Any {
                            type_url: format!("type.googleapis.com/{FQ_RESERVE_STOCK}"),
                            value: Vec::new(),
                        }),
                    })),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let resp = pb::SagaResponse {
                commands: vec![cmd],
            };
            host_fill(out, &resp.encode_to_vec());
            STATUS_OK
        }
        CB_PM_EVENT => {
            // Stateful PM: the host reacts to the newest trigger event and
            // emits one stamped command, returning the full PM response.
            let paux = abi_pb::PmEventAux::decode(aux).expect("pm event aux");
            with_session(key, |s| {
                s.observed_state.push(state_value(paux.state.as_ref()))
            });
            let cmd = pb::CommandBook {
                cover: Some(pb::Cover {
                    domain: "inventory".to_string(),
                    ..Default::default()
                }),
                page: Some(pb::CommandPage {
                    body: Some(pb::command_page::Body::Command(pb::Command {
                        payload: Some(Any {
                            type_url: format!("type.googleapis.com/{FQ_RESERVE_STOCK}"),
                            value: Vec::new(),
                        }),
                    })),
                    ..Default::default()
                }),
                ..Default::default()
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
        CB_FACT_ANNOTATE => {
            // Annotates a fact with the router-owned state it was handed and
            // flags it with an Increased.
            let cctx = abi_pb::CommandContextAux::decode(aux).expect("fact aux");
            let counter = state_value(cctx.state.as_ref());
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
        CB_OK_ZERO => STATUS_OK,
        CB_OK_EMPTY => STATUS_OK_EMPTY,
        CB_FAILS => -13,
        CB_FAIL_HARD => -13, // plain failure, no status payload
        CB_RETURN_NOTHING => STATUS_OK_EMPTY,
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
        state_type_url: STATE_TYPE.to_string(),
        fold_callback_id: Some(CB_FOLD),
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
            page: Some(pb::CommandPage {
                body: Some(pb::command_page::Body::Command(pb::Command {
                    payload: Some(Any {
                        type_url: format!("type.googleapis.com/{fq}"),
                        value: payload,
                    }),
                })),
                ..Default::default()
            }),
            ..Default::default()
        }),
        events,
        ..Default::default()
    }
}

/// A ContextualCommand delivering an `fq` fact (empty payload) to `domain`.
fn fact_req(domain: &str, fq: &str, events: Option<pb::EventBook>) -> pb::ContextualCommand {
    let mut req = command_req(fq, Vec::new(), events);
    let book = req.command.as_mut().unwrap();
    book.cover.as_mut().unwrap().domain = domain.to_string();
    book.page = Some(pb::CommandPage {
        body: Some(pb::command_page::Body::Fact(pb::Fact {
            payload: Some(Any {
                type_url: format!("type.googleapis.com/{fq}"),
                value: Vec::new(),
            }),
        })),
        ..Default::default()
    });
    req
}

fn decode_response(bytes: &[u8]) -> pb::CommandHandlerResponse {
    pb::CommandHandlerResponse::decode(bytes).expect("CommandHandlerResponse")
}

/// The events of a CommandHandlerResponse's bytes.
fn decode_events(bytes: &[u8]) -> pb::EventBook {
    decode_response(bytes).events.expect("events")
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

fn increased_history(n: u32, next_sequence: u64) -> pb::EventBook {
    let mut book = increased_book(n);
    book.next_sequence = next_sequence;
    book
}

#[test]
fn abi_version_is_four() {
    assert_eq!(angzarr_abi_version(), 4);
}

#[test]
fn empty_history_command_emits_unstamped_events() {
    let router = Router::with_counter();
    let session = next_session();
    let (ret, bytes) = router.dispatch(
        session,
        &command_req(FQ_INCREASE_BY, IncreaseBy { n: 2 }.encode_to_vec(), None),
    );
    assert_eq!(ret, 0);
    let book = decode_events(&bytes);
    assert_eq!(
        book,
        increased_book(2),
        "the host's book crosses back as emitted: the coordinator stamps sequences"
    );

    let s = session_snapshot(session);
    assert_eq!(
        s.observed_cctx,
        vec![(0, false)],
        "fresh aggregate evidence"
    );
    assert!(s.fold_calls.is_empty(), "no history: the fold must not run");
    assert_eq!(s.observed_state, vec![0]);
}

#[test]
fn prior_events_fold_through_one_host_fold_call() {
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
    assert_eq!(
        s.fold_calls,
        vec![vec![0, 1]],
        "one batched call over both pages"
    );
    assert_eq!(
        s.observed_state,
        vec![2],
        "the router handed decide the folded state"
    );
    assert_eq!(
        s.observed_cctx,
        vec![(2, true)],
        "historical-state evidence"
    );

    let book = decode_events(&bytes);
    assert_eq!(
        book,
        increased_book(1),
        "the emitted book crosses back unstamped"
    );
}

#[test]
fn snapshot_loads_and_covered_pages_skip() {
    let router = Router::with_counter();
    let session = next_session();

    let mut history = pb::EventBook {
        snapshot: Some(pb::Snapshot {
            sequence: 2,
            state: Some(state_any(10)),
            ..Default::default()
        }),
        next_sequence: 4,
        ..Default::default()
    };
    let event = |seq: u64| pb::EventPage {
        sequence: seq,
        payload: Some(Any {
            type_url: format!("type.googleapis.com/{FQ_INCREASED}"),
            value: Vec::new(),
        }),
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
    assert_eq!(
        s.fold_calls,
        vec![vec![3]],
        "the covered page never reaches the fold"
    );
    assert_eq!(
        s.observed_state,
        vec![11],
        "snapshot 10 + one uncovered page"
    );
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

// command_handler.proto, CommandHandlerService.Handle step 2: an unknown type
// URL is refused INVALID_ARGUMENT, naming it (decision 12.11, backlog VC-1).
#[test]
fn unknown_command_is_invalid_argument_naming_its_type_url() {
    let router = Router::with_counter();
    let (ret, bytes) = router.dispatch(
        next_session(),
        &command_req("test.counter.Undeclared", Vec::new(), None),
    );
    assert_eq!(ret, -3);
    let (status, reason) = decode_status(&bytes);
    assert_eq!(reason, "NO_HANDLER_REGISTERED");
    assert_eq!(
        status.message,
        "no handler for type URL type.googleapis.com/test.counter.Undeclared"
    );
    // upcaster.proto (VC-1): ErrorInfo metadata upcast_type_url.
    let info = rpc_pb::ErrorInfo::decode(status.details[0].value.as_slice()).expect("ErrorInfo");
    assert_eq!(
        info.metadata.get("upcast_type_url").map(String::as_str),
        Some("type.googleapis.com/test.counter.Undeclared")
    );
}

#[test]
fn corrupt_persisted_event_is_data_loss() {
    let router = Router::with_counter();
    let mut history = pb::EventBook {
        next_sequence: 1,
        ..Default::default()
    };
    history.pages = vec![pb::EventPage {
        payload: Some(Any {
            type_url: format!("type.googleapis.com/{FQ_INCREASED}"),
            value: vec![0xFF, 0xFF, 0xFF],
        }),
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

#[test]
fn handler_emitting_nothing_returns_empty_events() {
    let router = Router::with_counter();
    let (ret, bytes) = router.dispatch(
        next_session(),
        &command_req(FQ_RETURN_NOTHING, Vec::new(), None),
    );
    assert_eq!(ret, 0);
    let book = decode_events(&bytes);
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
        assert_eq!(
            s.observed_state,
            vec![3],
            "each dispatch folded its own history"
        );
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
        projection_type_url: STATE_TYPE.to_string(),
        fold_callback_id: Some(CB_FOLD),
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

// Router-owned projections (docs/design-handling.md): the router folds the
// projection bytes through one stateless host fold and builds the Projection.
#[test]
fn projector_folds_every_page_through_the_abi() {
    let router = Router::with_projector();
    let session = next_session();
    let (ret, bytes) = router.dispatch_projector(session, &book_in_domain("counter", 3));
    assert_eq!(ret, 0);
    let proj = pb::Projection::decode(bytes.as_slice()).expect("Projection");
    assert_eq!(proj.projector, "CounterProjector");
    assert_eq!(
        proj.cover.as_ref().map(|c| c.domain.as_str()),
        Some("counter")
    );
    assert_eq!(
        state_value(proj.projection.as_ref()),
        3,
        "every page folded"
    );
    assert_eq!(
        proj.next_sequence, 3,
        "the last projected page's sequence + 1"
    );
    assert_eq!(
        session_snapshot(session).fold_calls,
        vec![vec![0, 1, 2]],
        "one batched call"
    );
}

#[test]
fn projector_undeclared_domain_folds_nothing_through_the_abi() {
    let router = Router::with_projector();
    let session = next_session();
    let (ret, bytes) = router.dispatch_projector(session, &book_in_domain("inventory", 3));
    assert_eq!(ret, 0);
    let proj = pb::Projection::decode(bytes.as_slice()).expect("Projection");
    assert_eq!(
        proj.projection,
        Some(Any {
            type_url: format!("/{STATE_TYPE}"),
            value: Vec::new(),
        }),
        "the projection type's default"
    );
    assert!(session_snapshot(session).fold_calls.is_empty());
}

#[test]
fn an_empty_delivery_reports_the_books_next_sequence() {
    let router = Router::with_projector();
    let mut book = book_in_domain("counter", 0);
    book.next_sequence = 7;
    let (ret, bytes) = router.dispatch_projector(next_session(), &book);
    assert_eq!(ret, 0);
    assert_eq!(
        pb::Projection::decode(bytes.as_slice())
            .unwrap()
            .next_sequence,
        7
    );
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

    fn dispatch_saga(&self, session: usize, req: &pb::EventBook) -> (i32, Vec<u8>) {
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

/// A saga source book in `domain` carrying the given event pages.
fn saga_request(domain: &str, pages: Vec<pb::EventPage>) -> pb::EventBook {
    pb::EventBook {
        cover: Some(pb::Cover {
            domain: domain.to_string(),
            ..Default::default()
        }),
        pages,
        ..Default::default()
    }
}

fn event_page_of(fq: &str) -> pb::EventPage {
    pb::EventPage {
        payload: Some(Any {
            type_url: format!("type.googleapis.com/{fq}"),
            value: Vec::new(),
        }),
        ..Default::default()
    }
}

#[test]
fn saga_emits_its_command_through_the_abi() {
    // The whole saga path across raw pointers: register a saga, dispatch a
    // source book, and confirm the host's command came back as emitted (the
    // coordinator stamps its provenance).
    let router = Router::with_saga();
    let mut page = event_page_of(FQ_ORDER_CREATED);
    page.sequence = 7;
    let req = saga_request("order", vec![page]);
    let (ret, bytes) = router.dispatch_saga(next_session(), &req);
    assert_eq!(ret, 0);
    let resp = pb::SagaResponse::decode(bytes.as_slice()).expect("SagaResponse");
    assert_eq!(resp.commands.len(), 1, "one command emitted");
    let cmd = &resp.commands[0];
    assert_eq!(cmd.cover.as_ref().unwrap().domain, "inventory");
    assert_eq!(
        cmd.page.as_ref().unwrap().command_id,
        None,
        "the command crosses back unstamped"
    );
}

#[test]
fn a_saga_handler_receives_the_source_cover_and_sequence() {
    let router = Router::with_saga();
    let session = next_session();
    let mut first = event_page_of(FQ_ORDER_CREATED);
    first.sequence = 3;
    let req = saga_request("order", vec![first, event_page_of(FQ_ORDER_CREATED)]);
    let (ret, _) = router.dispatch_saga(session, &req);
    assert_eq!(ret, 0);
    let observed: Vec<(String, u64)> = session_snapshot(session)
        .observed_pages
        .into_iter()
        .map(|(cover, seq)| (cover.map(|c| c.domain).unwrap_or_default(), seq))
        .collect();
    assert_eq!(
        observed,
        vec![("order".to_string(), 3), ("order".to_string(), 0)],
        "each trigger's cover and sequence"
    );
}

#[test]
fn saga_empty_source_through_the_abi_is_empty_saga_source() {
    // A source book with no pages likewise validates before routing as
    // EMPTY_SAGA_SOURCE, regardless of which saga (if any) consumes its domain.
    let router = Router::with_saga();
    let session = next_session();
    let (ret, bytes) = router.dispatch_saga(session, &pb::EventBook::default());
    assert_eq!(ret, -3, "invalid argument, negated");
    let (_, reason) = decode_status(&bytes);
    assert_eq!(reason, angzarr_router::error::codes::EMPTY_SAGA_SOURCE);
}

// --- process-manager ABI surface (per-kind entry points)

fn pm_descriptor_bytes() -> Vec<u8> {
    abi_pb::ProcessManagerDescriptor {
        name: "OrderPM".to_string(),
        pm_domain: "order-pm".to_string(),
        state_type_url: STATE_TYPE.to_string(),
        fold_callback_id: Some(CB_FOLD),
        events: vec![abi_pb::PmEventEntry {
            input_domain: "orders".to_string(),
            fq_type: FQ_ORDER_SHIPPED.to_string(),
            callback_id: CB_PM_EVENT,
        }],
        target_domains: vec!["inventory".to_string()],
        rules_version: String::new(),
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
fn pm_emits_its_command_through_the_abi() {
    // The whole PM path across raw pointers: the host sees the trigger cover
    // in its aux (it echoes the root into its command), and the command comes
    // back as emitted.
    let router = Router::with_process_manager();
    let mut page = event_page_of(FQ_ORDER_SHIPPED);
    page.sequence = 4;
    let mut req = pm_request("orders", vec![page]);
    req.trigger.as_mut().unwrap().cover.as_mut().unwrap().root = Some(pb::Uuid {
        value: vec![8; 16].into(),
    });
    let (ret, bytes) = router.dispatch_process_manager(next_session(), &req);
    assert_eq!(ret, 0);
    let resp = pb::ProcessManagerHandleResponse::decode(bytes.as_slice()).expect("PMResponse");
    assert_eq!(resp.commands.len(), 1);
    let cmd = &resp.commands[0];
    assert_eq!(
        cmd.cover.as_ref().unwrap().root,
        Some(pb::Uuid {
            value: vec![8; 16].into()
        }),
        "the trigger cover crossed to the host"
    );
    assert_eq!(
        cmd.page.as_ref().unwrap().command_id,
        None,
        "the command crosses back unstamped"
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
        state_type_url: STATE_TYPE.to_string(),
        fold_callback_id: Some(CB_FOLD),
        events: vec![abi_pb::PmEventEntry {
            input_domain: "orders".to_string(),
            fq_type: FQ_ORDER_CREATED.to_string(),
            callback_id: CB_PM_EVENT,
        }],
        target_domains: vec!["inventory".to_string()],
        rules_version: String::new(),
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
/// OrderShipped trigger.
fn pm2_descriptor_bytes() -> Vec<u8> {
    abi_pb::ProcessManagerDescriptor {
        name: "Pm2".to_string(),
        pm_domain: "other-pm".to_string(),
        state_type_url: STATE_TYPE.to_string(),
        fold_callback_id: Some(CB_FOLD),
        events: vec![abi_pb::PmEventEntry {
            input_domain: "orders".to_string(),
            fq_type: FQ_ORDER_SHIPPED.to_string(),
            callback_id: CB_PM2_EVENT,
        }],
        target_domains: vec!["inventory".to_string()],
        rules_version: String::new(),
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
        projection_type_url: STATE_TYPE.to_string(),
        fold_callback_id: Some(CB_PROJ_FOLD_REJECTS),
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

/// The counter aggregate with its fold replaced.
fn counter_with(fold: u64) -> Router {
    let mut desc = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
    desc.fold_callback_id = Some(fold);
    let r = angzarr_router_new();
    assert_eq!(register_aggregate_desc(r, desc), 0);
    Router(r)
}

/// A snapshot of CounterState 1 at sequence 0, then Increased pages at
/// sequences 0 (covered) and 1.
fn snapshot_history_book() -> pb::EventBook {
    let mut book = increased_history(2, 2);
    book.snapshot = Some(pb::Snapshot {
        sequence: 0,
        state: Some(state_any(1)),
        ..Default::default()
    });
    book
}

fn increase_one(events: Option<pb::EventBook>) -> pb::ContextualCommand {
    command_req(FQ_INCREASE_BY, IncreaseBy { n: 1 }.encode_to_vec(), events)
}

// Router-owned state (docs/design-handling.md): a delivered snapshot is the
// initial state the router hands the fold; covered pages never reach it.
#[test]
fn a_delivered_snapshot_is_the_initial_state_the_fold_continues() {
    let router = Router::with_counter();
    let session = next_session();
    let (ret, _) = router.dispatch(session, &increase_one(Some(snapshot_history_book())));
    assert_eq!(ret, 0);
    let seen = session_snapshot(session);
    assert_eq!(seen.fold_calls, vec![vec![1]], "only the uncovered page");
    assert_eq!(seen.observed_state, vec![2], "snapshot 1 + one folded page");
}

#[test]
fn a_fold_returning_status_ok_empty_keeps_the_state() {
    let router = counter_with(CB_OK_EMPTY);
    let session = next_session();
    let (ret, _) = router.dispatch(session, &increase_one(Some(snapshot_history_book())));
    assert_eq!(ret, 0);
    assert_eq!(session_snapshot(session).observed_state, vec![1]);
}

#[test]
fn a_failing_fold_is_data_loss() {
    let router = counter_with(CB_FAILS);
    let (ret, bytes) =
        router.dispatch(next_session(), &increase_one(Some(increased_history(2, 2))));
    assert_eq!(ret, -15);
    assert_eq!(
        decode_status(&bytes).1,
        angzarr_router::error::codes::PERSISTED_EVENT_CORRUPT
    );
}

#[test]
fn an_undecodable_fold_answer_is_an_unhandled_error() {
    let router = counter_with(CB_FACT_GARBAGE);
    let (ret, _) = router.dispatch(next_session(), &increase_one(Some(increased_history(1, 1))));
    assert_eq!(ret, -13);
}

#[test]
fn a_fresh_aggregate_starts_from_the_registered_state_types_default() {
    let router = Router::with_counter();
    let session = next_session();
    let (ret, _) = router.dispatch(session, &increase_one(None));
    assert_eq!(ret, 0);
    let seen = session_snapshot(session);
    assert!(seen.fold_calls.is_empty(), "nothing to fold");
    assert_eq!(seen.observed_state, vec![0]);
    assert_eq!(
        seen.observed_state_type,
        vec![format!("/{STATE_TYPE}")],
        "the default of the registered state type"
    );
}

#[test]
fn projector_fold_returning_status_ok_empty_keeps_the_projection() {
    let mut desc =
        abi_pb::ProjectorDescriptor::decode(projector_descriptor_bytes().as_slice()).unwrap();
    desc.fold_callback_id = Some(CB_OK_EMPTY);
    let bytes = desc.encode_to_vec();
    let r = angzarr_router_new();
    assert_eq!(
        unsafe { angzarr_router_register_projector(r, bytes.as_ptr(), bytes.len(), host_cb) },
        0
    );
    let router = Router(r);
    let (ret, bytes) = router.dispatch_projector(next_session(), &book_in_domain("counter", 2));
    assert_eq!(ret, 0);
    assert_eq!(
        state_value(
            pb::Projection::decode(bytes.as_slice())
                .unwrap()
                .projection
                .as_ref()
        ),
        0
    );
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

/// The order PM with its fold and event handler replaced.
fn pm_with(fold: u64, event: u64) -> Router {
    let mut desc =
        abi_pb::ProcessManagerDescriptor::decode(pm_descriptor_bytes().as_slice()).unwrap();
    desc.fold_callback_id = Some(fold);
    desc.events[0].callback_id = event;
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
fn a_pm_handler_receives_its_router_owned_state() {
    let router = pm_with(CB_FOLD, CB_PM_EVENT);
    let session = next_session();
    let (ret, _) =
        router.dispatch_process_manager(session, &pm_over_state(snapshot_history_book()));
    assert_eq!(ret, 0);
    let seen = session_snapshot(session);
    assert_eq!(seen.fold_calls, vec![vec![1]]);
    assert_eq!(seen.observed_state, vec![2]);
}

#[test]
fn pm_fold_and_handler_succeed_on_either_ok_status() {
    for ok in [CB_OK_ZERO, CB_OK_EMPTY] {
        let router = pm_with(CB_OK_EMPTY, ok);
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
fn failing_pm_fold_is_data_loss() {
    let router = pm_with(CB_FAILS, CB_OK_EMPTY);
    let (ret, bytes) =
        router.dispatch_process_manager(next_session(), &pm_over_state(increased_book(1)));
    assert_eq!(ret, -15);
    assert_eq!(
        decode_status(&bytes).1,
        angzarr_router::error::codes::PERSISTED_EVENT_CORRUPT
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
    decode_events(bytes).pages.len()
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
    assert_eq!(ret, -3, "a type no handler claims is INVALID_ARGUMENT");
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

// --- context aux

#[test]
fn command_handler_sees_its_cover_through_the_abi() {
    let router = Router::with_counter();
    let session = next_session();
    let mut req = command_req(FQ_INCREASE_BY, IncreaseBy { n: 1 }.encode_to_vec(), None);
    req.command.as_mut().unwrap().cover.as_mut().unwrap().root = Some(pb::Uuid {
        value: vec![4; 16].into(),
    });
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
    book.cover.as_mut().unwrap().root = Some(pb::Uuid {
        value: vec![6; 16].into(),
    });
    book.pages[1].sequence = 9;
    let (ret, _) = router.dispatch_projector(session, &book);
    assert_eq!(ret, 0);
    assert_eq!(
        session_snapshot(session).applied_pages,
        vec![(book.cover.clone(), 0), (book.cover.clone(), 9)],
        "the fold sees the book's cover and each page's sequence"
    );
}

// --- applier page context + PM replay

#[test]
fn appliers_see_their_page_place_through_the_abi() {
    let router = Router::with_counter();
    let session = next_session();
    let mut history = increased_history(2, 2);
    history.cover = Some(pb::Cover {
        domain: "counter".to_string(),
        root: Some(pb::Uuid {
            value: vec![5; 16].into(),
        }),
        ..Default::default()
    });
    history.pages[0].sequence = 0;
    history.pages[1].sequence = 1;
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

// --- several aggregates in one domain (C-0010, C-0042) ----------------------

const FQ_AUDIT: &str = "test.counter.Audit";

/// The counter aggregate plus a second "counter" aggregate handling Audit
/// (empty reply).
fn counter_and_auditor() -> Router {
    let router = Router::with_counter();
    let auditor = abi_pb::AggregateDescriptor {
        name: "Auditor".to_string(),
        domain: "counter".to_string(),
        commands: vec![abi_pb::CallbackEntry {
            fq_type: FQ_AUDIT.to_string(),
            callback_id: CB_OK_EMPTY,
        }],
        ..Default::default()
    };
    assert_eq!(register_aggregate_desc(router.0, auditor), 0);
    router
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

// --- facts: the Fact arm of the command page ---------------------------------

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

/// The (type name, CounterState value) of each recorded page.
fn recorded(book: &pb::EventBook) -> Vec<(String, Option<u32>)> {
    book.pages
        .iter()
        .map(|p| {
            let any = p
                .payload
                .as_ref()
                .expect("a recorded page carries its event");
            let name = angzarr_router::type_name_from_url(&any.type_url).to_string();
            let counter = name
                .ends_with("CounterState")
                .then(|| CounterState::decode(any.value.as_slice()).unwrap().value);
            (name, counter)
        })
        .collect()
}

#[test]
fn a_fact_is_annotated_against_folded_state_through_the_abi() {
    let router = counter_with_fact_handler(CB_FACT_ANNOTATE);
    let (ret, bytes) = router.dispatch(
        next_session(),
        &fact_req("counter", FQ_INCREASED, Some(increased_history(3, 3))),
    );
    assert_eq!(ret, 0);
    let book = decode_events(&bytes);
    assert_eq!(
        recorded(&book),
        vec![
            ("test.counter.CounterState".to_string(), Some(3)),
            (FQ_INCREASED.to_string(), None),
        ],
        "the annotated fact against 3 folded events, then its flag"
    );
    assert_eq!(
        book.cover, None,
        "payloads only: the coordinator supplies the cover"
    );
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
    let (ret, bytes) = router.dispatch(next_session(), &fact_req("other", FQ_INCREASED, None));
    assert_eq!(ret, 0);
    assert_eq!(
        recorded(&decode_events(&bytes))[0].0,
        "test.counter.CounterState",
        "other's fact handler ran"
    );
    let (ret, bytes) = router.dispatch(next_session(), &fact_req("counter", FQ_INCREASED, None));
    assert_eq!(ret, -3, "counter records no fact: INVALID_ARGUMENT");
    assert_eq!(
        decode_status(&bytes).1,
        angzarr_router::error::codes::NO_FACT_HANDLER
    );
}

#[test]
fn a_fact_handler_returning_nothing_records_the_fact_as_received() {
    for callback_id in [CB_OK_EMPTY, CB_FACT_KEEP] {
        let router = counter_with_fact_handler(callback_id);
        let (ret, bytes) =
            router.dispatch(next_session(), &fact_req("counter", FQ_INCREASED, None));
        assert_eq!(ret, 0);
        assert_eq!(
            recorded(&decode_events(&bytes)),
            vec![(FQ_INCREASED.to_string(), None)],
            "callback {callback_id}: the fact as received, no flags"
        );
    }
}

#[test]
fn an_undecodable_fact_record_is_an_unhandled_error() {
    let router = counter_with_fact_handler(CB_FACT_GARBAGE);
    let (ret, bytes) = router.dispatch(next_session(), &fact_req("counter", FQ_INCREASED, None));
    assert_eq!(ret, -13, "INTERNAL");
    assert_eq!(
        decode_status(&bytes).1,
        angzarr_router::error::codes::UNHANDLED_HANDLER_ERROR
    );
}

#[test]
fn an_unregistered_fact_never_reaches_the_host() {
    let router = counter_with_fact_handler(CB_FACT_ANNOTATE);
    // FQ_RESERVE is no fact; FQ_INCREASE_BY is a command, not a fact.
    for fq in [FQ_RESERVE, FQ_INCREASE_BY] {
        let session = next_session();
        let (ret, bytes) = router.dispatch(
            session,
            &fact_req("counter", fq, Some(increased_history(2, 2))),
        );
        assert_eq!(ret, -3, "INVALID_ARGUMENT");
        assert_eq!(
            decode_status(&bytes).1,
            angzarr_router::error::codes::NO_FACT_HANDLER
        );
        let seen = session_snapshot(session);
        assert_eq!(seen.counter, 0, "refused before any applier ran");
        assert!(seen.observed_cctx.is_empty(), "no command handler ran");
    }
}

// --- conflict resolution and command upcasting (docs/design-handling.md) -----
//
// types.proto Handling, ConflictResolution and ContextualCommand.conflict_nonce;
// options.proto conflict_units ("The resolver"); upcaster.proto; backlog
// HD-5, NC-1, CR-1, VC-1.

/// IncreaseBy(1) over `history` Increased events, resolved to `resolution`
/// at `expected`, carrying `nonce`.
fn handled_req(
    history: u32,
    expected: u64,
    resolution: pb::ConflictResolution,
    nonce: &[u8],
) -> pb::ContextualCommand {
    let mut req = command_req(
        FQ_INCREASE_BY,
        IncreaseBy { n: 1 }.encode_to_vec(),
        Some(increased_history(history, u64::from(history))),
    );
    req.command.as_mut().unwrap().handling = Some(pb::Handling {
        expected_sequence: Some(expected),
        conflict_resolution: resolution as i32,
    });
    req.conflict_nonce = nonce.to_vec().into();
    req
}

/// The counter aggregate with IncreaseBy on `command` and the given
/// changed() callback.
fn counter_resolving(command: u64, changed: Option<u64>) -> Router {
    let mut desc = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
    desc.commands[0].callback_id = command;
    desc.changed_callback_id = changed;
    let r = angzarr_router_new();
    assert_eq!(register_aggregate_desc(r, desc), 0);
    Router(r)
}

#[test]
fn strict_hands_the_host_its_handling_and_no_window_through_the_abi() {
    let router = Router::with_counter();
    let session = next_session();
    let (ret, _) = router.dispatch(
        session,
        &handled_req(2, 2, pb::ConflictResolution::Strict, &[]),
    );
    assert_eq!(ret, 0);
    assert_eq!(
        session_snapshot(session).observed_handling,
        vec![(2, pb::ConflictResolution::Strict as i32, vec![], vec![])]
    );
}

#[test]
fn aggregate_handles_crosses_the_window_and_nonce_and_keeps_resolved_output() {
    let router = Router::with_counter();
    let session = next_session();
    let (ret, bytes) = router.dispatch(
        session,
        &handled_req(3, 1, pb::ConflictResolution::AggregateHandles, b"n-1"),
    );
    assert_eq!(ret, 0);
    assert_eq!(decode_events(&bytes), increased_book(1));
    assert_eq!(
        session_snapshot(session).observed_handling,
        vec![(
            1,
            pb::ConflictResolution::AggregateHandles as i32,
            vec![1, 2],
            b"n-1".to_vec()
        )]
    );
}

#[test]
fn aggregate_handles_unresolved_or_made_up_is_aborted_through_the_abi() {
    for command in [CB_INCREASE_IGNORES, CB_INCREASE_MADE_UP] {
        let router = counter_resolving(command, None);
        let (ret, bytes) = router.dispatch(
            next_session(),
            &handled_req(1, 0, pb::ConflictResolution::AggregateHandles, b"n-1"),
        );
        assert_eq!(ret, -10, "callback {command}: ABORTED");
        assert_eq!(
            decode_status(&bytes).1,
            angzarr_router::error::codes::CONFLICT_UNRESOLVED
        );
    }
}

#[test]
fn commutative_runs_the_resolver_over_host_state_through_the_abi() {
    // History 0..1, expected 1: expected = 1, head = 2, after = 3.
    let router = counter_resolving(CB_INCREASE_BY, Some(CB_CHANGED_NONE));
    let session = next_session();
    let (ret, bytes) = router.dispatch(
        session,
        &handled_req(2, 1, pb::ConflictResolution::Commutative, b"n-1"),
    );
    assert_eq!(ret, 0, "disjoint: kept");
    assert_eq!(decode_events(&bytes), increased_book(1));
    let seen = session_snapshot(session);
    assert_eq!(seen.changed_calls, vec![(1, 2), (2, 3)]);
    assert_eq!(
        seen.observed_handling[0].3,
        Vec::<u8>::new(),
        "the host never sees the nonce under COMMUTATIVE"
    );
}

#[test]
fn commutative_overlap_is_aborted_through_the_abi() {
    let router = counter_resolving(CB_INCREASE_BY, Some(CB_CHANGED_VALUE));
    let (ret, bytes) = router.dispatch(
        next_session(),
        &handled_req(2, 1, pb::ConflictResolution::Commutative, b"n-1"),
    );
    assert_eq!(ret, -10);
    let (status, reason) = decode_status(&bytes);
    assert_eq!(reason, angzarr_router::error::codes::CONFLICT_OVERLAP);
    assert_eq!(status.code, 10);
}

#[test]
fn commutative_without_changed_is_unsupported_through_the_abi() {
    let router = Router::with_counter();
    let (ret, bytes) = router.dispatch(
        next_session(),
        &handled_req(1, 0, pb::ConflictResolution::Commutative, b"n-1"),
    );
    assert_eq!(ret, -3);
    assert_eq!(
        decode_status(&bytes).1,
        angzarr_router::error::codes::CONFLICT_RESOLUTION_UNSUPPORTED
    );
}

#[test]
fn a_command_upcaster_runs_before_dispatch_through_the_abi() {
    let mut desc = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
    desc.command_upcasters = vec![abi_pb::CallbackEntry {
        fq_type: FQ_INCREASE_BY_V1.to_string(),
        callback_id: CB_UPCAST_V1,
    }];
    let r = angzarr_router_new();
    assert_eq!(register_aggregate_desc(r, desc), 0);
    let router = Router(r);
    let (ret, bytes) = router.dispatch(
        next_session(),
        &command_req(FQ_INCREASE_BY_V1, IncreaseBy { n: 2 }.encode_to_vec(), None),
    );
    assert_eq!(ret, 0, "IncreaseByV1 upcast to IncreaseBy");
    assert_eq!(decode_events(&bytes), increased_book(2));
}

// CommandHandlerResponse / ProcessManagerHandleResponse.rules_version:
// reported on every response from the registered declaration (RV-2).
#[test]
fn every_response_reports_the_registered_rules_version_through_the_abi() {
    let mut desc = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
    desc.rules_version = "1.4.2+build.7".to_string();
    let r = angzarr_router_new();
    assert_eq!(register_aggregate_desc(r, desc), 0);
    let router = Router(r);
    let (ret, bytes) = router.dispatch(
        next_session(),
        &command_req(FQ_INCREASE_BY, IncreaseBy { n: 1 }.encode_to_vec(), None),
    );
    assert_eq!(ret, 0);
    assert_eq!(decode_response(&bytes).rules_version, "1.4.2+build.7");

    let mut pm =
        abi_pb::ProcessManagerDescriptor::decode(pm_descriptor_bytes().as_slice()).unwrap();
    pm.rules_version = "2.0.0".to_string();
    let bytes = pm.encode_to_vec();
    let r = angzarr_router_new();
    assert_eq!(
        unsafe { angzarr_router_register_process_manager(r, bytes.as_ptr(), bytes.len(), host_cb) },
        0
    );
    let router = Router(r);
    let (ret, bytes) = router.dispatch_process_manager(
        next_session(),
        &pm_request("orders", vec![event_page_of(FQ_ORDER_SHIPPED)]),
    );
    assert_eq!(ret, 0);
    assert_eq!(
        pb::ProcessManagerHandleResponse::decode(bytes.as_slice())
            .unwrap()
            .rules_version,
        "2.0.0"
    );
    let (ret, bytes) = router.dispatch_process_manager(
        next_session(),
        &pm_request("billing", vec![event_page_of(FQ_ORDER_SHIPPED)]),
    );
    assert_eq!(ret, 0);
    assert_eq!(
        pb::ProcessManagerHandleResponse::decode(bytes.as_slice())
            .unwrap()
            .rules_version,
        "2.0.0",
        "reported even when no PM consumed the trigger"
    );
}

// Registration carries the upcasters' source types, claimed like command
// types: a second aggregate accepting the same old type is refused.
#[test]
fn an_upcaster_source_type_claimed_twice_is_refused_at_registration() {
    let upcasting = |name: &str, command: &str| {
        let mut desc = abi_pb::AggregateDescriptor::decode(descriptor_bytes().as_slice()).unwrap();
        desc.name = name.to_string();
        desc.commands = vec![abi_pb::CallbackEntry {
            fq_type: command.to_string(),
            callback_id: CB_INCREASE_BY,
        }];
        desc.command_upcasters = vec![abi_pb::CallbackEntry {
            fq_type: FQ_INCREASE_BY_V1.to_string(),
            callback_id: CB_UPCAST_V1,
        }];
        desc
    };
    let r = angzarr_router_new();
    let router = Router(r);
    assert_eq!(
        register_aggregate_desc(r, upcasting("first", FQ_INCREASE_BY)),
        0
    );
    assert_eq!(
        register_aggregate_desc(r, upcasting("second", "test.counter.Other")),
        -3,
        "IncreaseByV1 is already claimed"
    );
    drop(router);
}
