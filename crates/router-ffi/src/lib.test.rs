//! The C ABI, driven as a binding drives it (crate::host): one register of
//! a ComponentSpec, one dispatch, role Calls in and Returns out, errors as
//! keys and args (decision 14.37). Each kind's every role is pinned here:
//! a domain (aggregate, process manager), a policy, a projector.

use std::sync::{Arc, Mutex};

use prost::Message;
use prost_types::{Any, Timestamp};

use angzarr_router::error::{codes, extras, BusinessStatus, CodedError, GrpcCode, RetryableStatus};
use angzarr_router::pb;

use super::*;
use crate::abi_pb::dispatch_request::Request as Req;
use crate::abi_pb::dispatch_response::Response as Resp;
use crate::abi_pb::{ComponentSpec, Role, RoleEntry};
use crate::host::{self, Host};

// --- the host's business messages (what generated protobuf classes would be)

#[derive(Clone, PartialEq, prost::Message)]
struct CounterState {
    #[prost(uint32, tag = "1")]
    count: u32,
}

#[derive(Clone, PartialEq, prost::Message)]
struct IncreaseBy {
    #[prost(uint32, tag = "1")]
    n: u32,
}

const STATE: &str = "test.counter.CounterState";
const INCREASE_BY: &str = "test.counter.IncreaseBy";
const INCREASED: &str = "test.counter.Increased";
const NOTED: &str = "test.counter.Noted";
const START: &str = "test.fulfillment.Start";
const ADVANCE: &str = "test.fulfillment.Advance";
const TIMED_OUT: &str = "test.fulfillment.TimedOut";
const RESERVE: &str = "test.inventory.Reserve";
const ORDER_PLACED: &str = "test.order.OrderPlaced";
const PROJECTION: &str = "test.counter.CounterView";

fn any(fq: &str, value: Vec<u8>) -> Any {
    Any {
        type_url: format!("/{fq}"),
        value,
    }
}

fn state_any(count: u32) -> Any {
    any(STATE, CounterState { count }.encode_to_vec())
}

fn count(state: Option<&Any>) -> u32 {
    state.map_or(0, |s| {
        CounterState::decode(s.value.as_slice())
            .expect("CounterState")
            .count
    })
}

fn uuid(byte: u8) -> pb::Uuid {
    pb::Uuid {
        value: vec![byte; 16].into(),
    }
}

fn cover(domain: &str) -> pb::Cover {
    pb::Cover {
        domain: domain.to_string(),
        root: Some(uuid(7)),
        ..Default::default()
    }
}

fn entry(role: Role, fq_type: &str, callback_id: u64) -> RoleEntry {
    RoleEntry {
        role: role as i32,
        fq_type: fq_type.to_string(),
        callback_id,
    }
}

fn spec(kind: pb::ComponentKind, roles: Vec<RoleEntry>) -> ComponentSpec {
    ComponentSpec {
        kind: kind as i32,
        name: "counter-aggregate".to_string(),
        domain: "counter".to_string(),
        state_type: STATE.to_string(),
        rules_version: "1.2.3".to_string(),
        roles,
        ..Default::default()
    }
}

fn hosting(spec: ComponentSpec) -> Host {
    let mut host = Host::new();
    host.register(&spec).expect("the spec registers");
    host
}

/// A fold counting every page into the state.
fn counting_fold() -> u64 {
    host::callback(|call: abi_pb::FoldCall| {
        let n = count(call.state.as_ref()) + call.pages.len() as u32;
        Ok(Some(abi_pb::FoldReturn {
            outcome: Some(abi_pb::fold_return::Outcome::State(state_any(n))),
        }))
    })
}

type Seen<T> = Arc<Mutex<Vec<T>>>;

/// A decide recording its Call and emitting one Increased.
fn recording_decide(seen: Seen<abi_pb::DecideCall>) -> u64 {
    host::callback(move |call: abi_pb::DecideCall| {
        seen.lock().unwrap().push(call);
        Ok(Some(abi_pb::DecideReturn {
            events: Some(pb::EventBook {
                pages: vec![page(0, INCREASED)],
                ..Default::default()
            }),
            resolved_nonce: Vec::new(),
        }))
    })
}

/// A decide failing with `err`.
fn failing_decide(err: CodedError) -> u64 {
    host::callback(move |_: abi_pb::DecideCall| Err::<Option<abi_pb::DecideReturn>, _>(err.clone()))
}

fn page(sequence: u64, fq: &str) -> pb::EventPage {
    pb::EventPage {
        sequence,
        payload: Some(any(fq, Vec::new())),
        ..Default::default()
    }
}

fn prior(sequences: &[u64]) -> pb::EventBook {
    pb::EventBook {
        cover: Some(cover("counter")),
        pages: sequences.iter().map(|s| page(*s, INCREASED)).collect(),
        next_sequence: sequences.last().map_or(0, |s| s + 1),
        ..Default::default()
    }
}

fn recorded_at() -> Timestamp {
    Timestamp {
        seconds: 1_761_696_000,
        nanos: 5,
    }
}

/// A command delivered with its whole identity and acceptance time.
fn command(fq: &str, events: Option<pb::EventBook>) -> pb::ContextualCommand {
    pb::ContextualCommand {
        events,
        command: Some(pb::CommandBook {
            cover: Some(cover("counter")),
            page: Some(pb::CommandPage {
                body: Some(pb::command_page::Body::Command(pb::Command {
                    payload: Some(any(fq, IncreaseBy { n: 1 }.encode_to_vec())),
                })),
                command_id: Some(uuid(1)),
                correlation_id: Some(uuid(2)),
                ..Default::default()
            }),
            ..Default::default()
        }),
        recorded_at: Some(recorded_at()),
        ..Default::default()
    }
}

fn fact(payload: Any, events: Option<pb::EventBook>) -> pb::ContextualCommand {
    pb::ContextualCommand {
        events,
        command: Some(pb::CommandBook {
            cover: Some(cover("counter")),
            page: Some(pb::CommandPage {
                body: Some(pb::command_page::Body::Fact(pb::Fact {
                    payload: Some(payload),
                })),
                command_id: Some(uuid(3)),
                correlation_id: Some(uuid(2)),
                fact_origin: Some(pb::FactOrigin {
                    external_id: "webhook-9".to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }),
        recorded_at: Some(recorded_at()),
        ..Default::default()
    }
}

fn handled(host: &Host, cc: pb::ContextualCommand) -> pb::CommandHandlerResponse {
    match host
        .dispatch(Req::Command(cc))
        .expect("the command is handled")
    {
        Resp::Command(response) => response,
        other => panic!("answered in another arm: {other:?}"),
    }
}

fn refused(host: &Host, request: Req) -> CodedError {
    host.dispatch(request).expect_err("the request is refused")
}

fn event_types(response: &pb::CommandHandlerResponse) -> Vec<String> {
    response
        .events
        .as_ref()
        .map(|b| {
            b.pages
                .iter()
                .map(|p| p.payload.as_ref().unwrap().type_url.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn args(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

// --- the entry points --------------------------------------------------------

#[test]
fn the_abi_version_is_the_generator_s() {
    assert_eq!(angzarr_abi_version(), angzarr_abigen::abi_version());
}

#[test]
fn the_exported_symbols_are_exactly_the_generator_s_with_their_c_signatures() {
    let names: Vec<&str> = angzarr_abigen::SYMBOLS.iter().map(|s| s.name).collect();
    assert_eq!(
        names,
        [
            "angzarr_abi_version",
            "angzarr_buf_alloc",
            "angzarr_buf_release",
            "angzarr_router_new",
            "angzarr_router_free",
            "angzarr_router_register",
            "angzarr_router_dispatch",
        ]
    );
    // Each export has the C signature SYMBOLS declares (a compile-time check).
    let _: extern "C" fn() -> u32 = angzarr_abi_version;
    let _: extern "C" fn(usize) -> *mut u8 = angzarr_buf_alloc;
    let _: unsafe extern "C" fn(*mut u8, usize) = angzarr_buf_release;
    let _: extern "C" fn() -> *mut c_void = angzarr_router_new;
    let _: unsafe extern "C" fn(*mut c_void) = angzarr_router_free;
    let _: unsafe extern "C" fn(*mut c_void, *const u8, usize, AngzarrCb, *mut AngzarrBuf) -> i32 =
        angzarr_router_register;
    let _: unsafe extern "C" fn(*mut c_void, *const u8, usize, *mut AngzarrBuf) -> i32 =
        angzarr_router_dispatch;
}

#[test]
fn a_null_router_is_null_abi_pointer_with_the_key_as_its_status_message() {
    for ret_and_out in [
        {
            let mut out = AngzarrBuf {
                data: std::ptr::null_mut(),
                len: 0,
            };
            let ret = unsafe {
                angzarr_router_register(
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    0,
                    host::trampoline,
                    &mut out,
                )
            };
            (ret, out)
        },
        {
            let mut out = AngzarrBuf {
                data: std::ptr::null_mut(),
                len: 0,
            };
            let ret = unsafe {
                angzarr_router_dispatch(std::ptr::null_mut(), std::ptr::null(), 0, &mut out)
            };
            (ret, out)
        },
    ] {
        let (ret, out) = ret_and_out;
        assert_eq!(ret, -(GrpcCode::Internal as i32));
        assert!(!out.data.is_null(), "a failure fills out with its Status");
        let bytes = unsafe { std::slice::from_raw_parts(out.data, out.len) }.to_vec();
        unsafe { angzarr_buf_release(out.data, out.len) };
        let status = crate::google::rpc::Status::decode(bytes.as_slice()).expect("Status");
        assert_eq!(status.message, codes::NULL_ABI_POINTER);
        let err = host::coded_from_status(ret, &bytes);
        assert_eq!(err.code, codes::NULL_ABI_POINTER);
        assert!(err.extras.is_empty());
    }
}

// --- registration ----------------------------------------------------------

#[test]
fn an_undecodable_spec_is_any_decode_failed_naming_component_spec() {
    let mut host = Host::new();
    let (ret, bytes) = host.register_raw(b"\xff\xff\xff");
    let err = host::coded_from_status(ret, &bytes);
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, codes::ANY_DECODE_FAILED);
    assert_eq!(
        err.extras.get(extras::MESSAGE_TYPE).map(String::as_str),
        Some("io.angzarr.router.ffi.v1.ComponentSpec")
    );
    assert!(err.extras.contains_key(extras::EXT_ERR_TXT));
}

#[test]
fn a_spec_with_no_kind_is_missing_component_kind() {
    let mut host = Host::new();
    let err = host
        .register(&spec(pb::ComponentKind::Unspecified, vec![]))
        .expect_err("no kind");
    assert_eq!(err.code, codes::MISSING_COMPONENT_KIND);
    assert_eq!(err.grpc, GrpcCode::Internal);
}

#[test]
fn a_role_the_kind_does_not_take_is_role_not_for_kind() {
    for (kind, role, kind_name) in [
        (pb::ComponentKind::Policy, Role::Decide, "POLICY"),
        (pb::ComponentKind::Aggregate, Role::Starts, "AGGREGATE"),
        (pb::ComponentKind::Aggregate, Role::PolicyEvent, "AGGREGATE"),
        (pb::ComponentKind::Projector, Role::Fold, "PROJECTOR"),
        (
            pb::ComponentKind::ProcessManager,
            Role::Project,
            "PROCESS_MANAGER",
        ),
    ] {
        let mut host = Host::new();
        let err = host
            .register(&spec(kind, vec![entry(role, INCREASE_BY, 1)]))
            .expect_err("a role the kind does not take");
        assert_eq!(
            err.code,
            codes::ROLE_NOT_FOR_KIND,
            "{role:?} on {kind_name}"
        );
        assert_eq!(err.grpc, GrpcCode::Internal);
        assert_eq!(
            err.extras,
            args(&[
                (extras::ROLE, role.as_str_name()),
                (extras::COMPONENT_KIND, kind_name),
            ])
        );
    }
}

#[test]
fn a_second_component_is_duplicate_component_naming_the_first() {
    let mut host = hosting(spec(pb::ComponentKind::Aggregate, vec![]));
    let err = host
        .register(&spec(pb::ComponentKind::Projector, vec![]))
        .expect_err("a second component");
    assert_eq!(err.code, codes::DUPLICATE_COMPONENT);
    assert_eq!(
        err.extras,
        args(&[
            (extras::COMPONENT, "counter-aggregate"),
            (extras::COMPONENT_KIND, "AGGREGATE"),
        ])
    );
}

#[test]
fn a_dispatch_before_registration_is_no_handlers_registered() {
    let host = Host::new();
    let err = refused(&host, Req::Command(command(INCREASE_BY, None)));
    assert_eq!(err.code, codes::NO_HANDLERS_REGISTERED);
    assert_eq!(err.grpc, GrpcCode::Internal);
}

#[test]
fn an_undecodable_or_empty_request_is_refused_as_a_bug() {
    let host = hosting(spec(pb::ComponentKind::Aggregate, vec![]));
    let (ret, bytes) = host.dispatch_raw(b"\xff\xff\xff");
    let err = host::coded_from_status(ret, &bytes);
    assert_eq!(err.code, codes::ANY_DECODE_FAILED);
    assert_eq!(
        err.extras.get(extras::MESSAGE_TYPE).map(String::as_str),
        Some("io.angzarr.router.ffi.v1.DispatchRequest")
    );
    let (ret, bytes) = host.dispatch_raw(&[]);
    let err = host::coded_from_status(ret, &bytes);
    assert_eq!(err.code, codes::EMPTY_DISPATCH_REQUEST);
    assert_eq!(err.grpc, GrpcCode::Internal);
}

#[test]
fn a_request_for_a_kind_the_router_does_not_host_is_no_component_of_kind() {
    let host = hosting(spec(pb::ComponentKind::Aggregate, vec![]));
    let err = refused(&host, Req::Project(prior(&[0])));
    assert_eq!(err.code, codes::NO_COMPONENT_OF_KIND);
    assert_eq!(err.grpc, GrpcCode::Unimplemented);
}

#[test]
fn describe_answers_the_registration_hello() {
    let host = hosting(spec(pb::ComponentKind::Aggregate, vec![]));
    let Resp::Describe(descriptor) = host
        .dispatch(Req::Describe(Default::default()))
        .expect("described")
    else {
        panic!("another arm");
    };
    assert_eq!(descriptor.name, "counter-aggregate");
    assert_eq!(descriptor.kind, pb::ComponentKind::Aggregate as i32);
    assert_eq!(descriptor.rules_version, "1.2.3");
    assert_eq!(
        descriptor.state.as_ref().map(|s| s.type_url.as_str()),
        Some("/test.counter.CounterState")
    );
}

// --- the callback convention -------------------------------------------------

#[test]
fn a_callback_is_handed_its_call_message_name_and_bytes_and_no_aux() {
    let seen: Seen<(String, Vec<u8>)> = Arc::default();
    let sink = seen.clone();
    let decide = host::raw_callback(move |type_url, payload| {
        sink.lock()
            .unwrap()
            .push((type_url.to_string(), payload.to_vec()));
        (STATUS_OK_EMPTY, Vec::new())
    });
    let host = hosting(spec(
        pb::ComponentKind::Aggregate,
        vec![entry(Role::Decide, INCREASE_BY, decide)],
    ));
    handled(&host, command(INCREASE_BY, None));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "io.angzarr.router.ffi.v1.DecideCall");
    abi_pb::DecideCall::decode(seen[0].1.as_slice()).expect("the payload is the DecideCall");
}

// --- a domain: fold and decide -----------------------------------------------

#[test]
fn decide_is_handed_the_folded_state_and_the_whole_context() {
    let seen: Seen<abi_pb::DecideCall> = Arc::default();
    let host = hosting(spec(
        pb::ComponentKind::Aggregate,
        vec![
            entry(Role::Fold, "", counting_fold()),
            entry(Role::Decide, INCREASE_BY, recording_decide(seen.clone())),
        ],
    ));
    let response = handled(&host, command(INCREASE_BY, Some(prior(&[0, 1]))));
    assert_eq!(event_types(&response), ["/test.counter.Increased"]);
    assert_eq!(response.rules_version, "1.2.3");

    let call = seen.lock().unwrap().pop().expect("decide ran");
    assert_eq!(
        call.command.as_ref().map(|c| c.type_url.as_str()),
        Some("/test.counter.IncreaseBy")
    );
    assert_eq!(count(call.state.as_ref()), 2, "the two prior pages folded");
    let ctx = call.context.expect("the context");
    assert_eq!(ctx.next_sequence, 2);
    assert!(ctx.had_prior_events);
    assert_eq!(ctx.cover, Some(cover("counter")));
    assert_eq!(ctx.expected_sequence, 2);
    assert_eq!(ctx.command_id, Some(uuid(1)));
    assert_eq!(ctx.correlation_id, Some(uuid(2)));
    assert_eq!(ctx.fact_origin, None);
    assert_eq!(ctx.recorded_at, Some(recorded_at()));
}

#[test]
fn every_status_a_handler_fails_with_crosses_exactly() {
    let cases = [
        CodedError::rejection(BusinessStatus::FailedPrecondition, "NOT_OPEN", []),
        CodedError::rejection(BusinessStatus::InvalidArgument, "BAD_AMOUNT", []),
        CodedError::rejection(BusinessStatus::NotFound, "NO_SUCH_ITEM", []),
        CodedError::rejection(BusinessStatus::AlreadyExists, "ALREADY_OPEN", []),
        CodedError::rejection(
            BusinessStatus::OutOfRange,
            "TOO_LARGE",
            [("limit".to_string(), "10".to_string())],
        ),
        CodedError::rejection(BusinessStatus::PermissionDenied, "NOT_YOURS", []),
        CodedError::immediate("STALE_QUOTE", []),
        CodedError::bug("INVARIANT_BROKEN", [("field".to_string(), "x".to_string())]),
        CodedError::retryable_bug(RetryableStatus::Unavailable, "LEDGER_DOWN", []),
        CodedError::retryable_bug(RetryableStatus::DeadlineExceeded, "LEDGER_SLOW", []),
        CodedError::retryable_bug(RetryableStatus::ResourceExhausted, "LEDGER_QUOTA", []),
        CodedError::refused(any("test.counter.Overdrawn", vec![8, 3])),
    ];
    for original in cases {
        let host = hosting(spec(
            pb::ComponentKind::Aggregate,
            vec![entry(
                Role::Decide,
                INCREASE_BY,
                failing_decide(original.clone()),
            )],
        ));
        let err = refused(&host, Req::Command(command(INCREASE_BY, None)));
        assert_eq!(err, original, "crossed as itself");
    }
}

#[test]
fn undecodable_host_status_bytes_are_host_status_undecodable() {
    let decide = host::raw_callback(|_, _| (-9, b"\xff\xff\xff".to_vec()));
    let host = hosting(spec(
        pb::ComponentKind::Aggregate,
        vec![entry(Role::Decide, INCREASE_BY, decide)],
    ));
    let err = refused(&host, Req::Command(command(INCREASE_BY, None)));
    assert_eq!(err.code, codes::HOST_STATUS_UNDECODABLE);
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert!(err.extras.contains_key(extras::EXT_ERR_TXT));
}

#[test]
fn a_host_failure_with_no_status_bytes_is_unhandled_under_its_returned_status() {
    let decide = host::raw_callback(|_, _| (-14, Vec::new()));
    let host = hosting(spec(
        pb::ComponentKind::Aggregate,
        vec![entry(Role::Decide, INCREASE_BY, decide)],
    ));
    let err = refused(&host, Req::Command(command(INCREASE_BY, None)));
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
    assert_eq!(err.grpc, GrpcCode::Unavailable);
}

#[test]
fn an_undecodable_return_is_any_decode_failed_naming_the_return_message() {
    let decide = host::raw_callback(|_, _| (STATUS_OK, b"\xff\xff\xff".to_vec()));
    let host = hosting(spec(
        pb::ComponentKind::Aggregate,
        vec![entry(Role::Decide, INCREASE_BY, decide)],
    ));
    let err = refused(&host, Req::Command(command(INCREASE_BY, None)));
    assert_eq!(err.code, codes::ANY_DECODE_FAILED);
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(
        err.extras.get(extras::MESSAGE_TYPE).map(String::as_str),
        Some("io.angzarr.router.ffi.v1.DecideReturn")
    );
    assert!(err.extras.contains_key(extras::EXT_ERR_TXT));
}

#[test]
fn a_fold_failure_names_its_page_and_carries_its_cause_as_args() {
    let fold = host::callback(|_: abi_pb::FoldCall| {
        Ok(Some(abi_pb::FoldReturn {
            outcome: Some(abi_pb::fold_return::Outcome::Failure(abi_pb::FoldFailure {
                page_index: Some(1),
                cause: Some(pb::Diagnostic {
                    code: "NEGATIVE_BALANCE".to_string(),
                    args: [("balance".to_string(), "-3".to_string())].into(),
                }),
            })),
        }))
    });
    let host = hosting(spec(
        pb::ComponentKind::Aggregate,
        vec![
            entry(Role::Fold, "", fold),
            entry(Role::Decide, INCREASE_BY, recording_decide(Arc::default())),
        ],
    ));
    let err = refused(
        &host,
        Req::Command(command(INCREASE_BY, Some(prior(&[0, 1])))),
    );
    assert_eq!(err.code, codes::FOLD_FAILED);
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(
        err.extras,
        args(&[
            (extras::SEQUENCE, "1"),
            (extras::TYPE_URL, "/test.counter.Increased"),
            (extras::STATE_TYPE_URL, "/test.counter.CounterState"),
            (extras::CAUSE, "NEGATIVE_BALANCE"),
            ("cause.balance", "-3"),
        ])
    );
}

#[test]
fn a_fold_failing_with_a_status_fails_the_batch_with_that_key_as_its_cause() {
    let fold = host::callback(|_: abi_pb::FoldCall| {
        Err::<Option<abi_pb::FoldReturn>, _>(CodedError::bug("BAD_PAGE", []))
    });
    let host = hosting(spec(
        pb::ComponentKind::Aggregate,
        vec![
            entry(Role::Fold, "", fold),
            entry(Role::Decide, INCREASE_BY, recording_decide(Arc::default())),
        ],
    ));
    let err = refused(
        &host,
        Req::Command(command(INCREASE_BY, Some(prior(&[0, 1])))),
    );
    assert_eq!(err.code, codes::FOLD_FAILED);
    assert_eq!(
        err.extras,
        args(&[
            (extras::FIRST_SEQUENCE, "0"),
            (extras::LAST_SEQUENCE, "1"),
            (extras::STATE_TYPE_URL, "/test.counter.CounterState"),
            (extras::CAUSE, codes::UNHANDLED_HANDLER_ERROR),
            ("cause.cause", "BAD_PAGE"),
        ])
    );
}

#[test]
fn a_command_no_decide_claims_is_unknown_command_type_before_any_callback() {
    let fold_calls: Seen<()> = Arc::default();
    let sink = fold_calls.clone();
    let fold = host::callback(move |_: abi_pb::FoldCall| {
        sink.lock().unwrap().push(());
        Ok(None::<abi_pb::FoldReturn>)
    });
    let host = hosting(spec(
        pb::ComponentKind::Aggregate,
        vec![entry(Role::Fold, "", fold)],
    ));
    let err = refused(&host, Req::Command(command(INCREASE_BY, Some(prior(&[0])))));
    assert_eq!(err.code, codes::UNKNOWN_COMMAND_TYPE);
    assert!(fold_calls.lock().unwrap().is_empty());
}

#[test]
fn aggregate_handles_hands_over_the_window_and_nonce_and_keeps_only_a_resolved_answer() {
    let seen: Seen<abi_pb::DecideCall> = Arc::default();
    let sink = seen.clone();
    let resolving = host::callback(move |call: abi_pb::DecideCall| {
        let nonce = call.context.as_ref().unwrap().conflict_nonce.clone();
        sink.lock().unwrap().push(call);
        Ok(Some(abi_pb::DecideReturn {
            events: Some(pb::EventBook {
                pages: vec![page(0, INCREASED)],
                ..Default::default()
            }),
            resolved_nonce: nonce,
        }))
    });
    let mut cc = command(INCREASE_BY, Some(prior(&[0, 1, 2])));
    cc.conflict_nonce = b"nonce-1".to_vec().into();
    cc.command.as_mut().unwrap().handling = Some(pb::Handling {
        expected_sequence: Some(1),
        conflict_resolution: pb::ConflictResolution::AggregateHandles as i32,
    });
    let host = hosting(spec(
        pb::ComponentKind::Aggregate,
        vec![
            entry(Role::Fold, "", counting_fold()),
            entry(Role::Decide, INCREASE_BY, resolving),
        ],
    ));
    assert_eq!(event_types(&handled(&host, cc.clone())).len(), 1);
    let ctx = seen.lock().unwrap().pop().unwrap().context.unwrap();
    assert_eq!(ctx.conflict_nonce, b"nonce-1");
    assert_eq!(
        ctx.window.iter().map(|p| p.sequence).collect::<Vec<_>>(),
        [1, 2]
    );

    let ignoring = hosting(spec(
        pb::ComponentKind::Aggregate,
        vec![
            entry(Role::Fold, "", counting_fold()),
            entry(Role::Decide, INCREASE_BY, recording_decide(Arc::default())),
        ],
    ));
    let err = refused(&ignoring, Req::Command(cc));
    assert_eq!(err.code, codes::CONFLICT_UNRESOLVED);
    assert_eq!(err.grpc, GrpcCode::Aborted);
}

#[test]
fn commutative_compares_states_through_changed_and_refuses_an_overlap() {
    let calls: Seen<abi_pb::ChangedCall> = Arc::default();
    let sink = calls.clone();
    let changed = host::callback(move |call: abi_pb::ChangedCall| {
        sink.lock().unwrap().push(call);
        Ok(Some(abi_pb::ChangedReturn {
            units: vec!["count".to_string()],
        }))
    });
    let mut cc = command(INCREASE_BY, Some(prior(&[0, 1])));
    cc.command.as_mut().unwrap().handling = Some(pb::Handling {
        expected_sequence: Some(1),
        conflict_resolution: pb::ConflictResolution::Commutative as i32,
    });
    let host = hosting(spec(
        pb::ComponentKind::Aggregate,
        vec![
            entry(Role::Fold, "", counting_fold()),
            entry(Role::Decide, INCREASE_BY, recording_decide(Arc::default())),
            entry(Role::Changed, "", changed),
        ],
    ));
    let err = refused(&host, Req::Command(cc));
    assert_eq!(err.code, codes::CONFLICT_OVERLAP);
    assert_eq!(err.grpc, GrpcCode::Aborted);
    assert_eq!(
        err.extras,
        args(&[
            (extras::WINDOW_UNITS, "count"),
            (extras::COMMAND_UNITS, "count")
        ])
    );
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        2,
        "changed(expected, head) and changed(head, after)"
    );
    assert_eq!(count(calls[0].before.as_ref()), 1);
    assert_eq!(count(calls[0].after.as_ref()), 2);
}

// --- a domain: facts and refusals ----------------------------------------------

#[test]
fn a_fact_is_handed_its_origin_and_records_what_its_handler_returns() {
    let seen: Seen<abi_pb::FactCall> = Arc::default();
    let sink = seen.clone();
    let noted = host::callback(move |call: abi_pb::FactCall| {
        sink.lock().unwrap().push(call);
        Ok(Some(abi_pb::FactReturn {
            fact: None,
            flags: vec![any(INCREASED, Vec::new())],
        }))
    });
    let host = hosting(spec(
        pb::ComponentKind::Aggregate,
        vec![
            entry(Role::Fold, "", counting_fold()),
            entry(Role::Fact, NOTED, noted),
        ],
    ));
    let response = handled(&host, fact(any(NOTED, vec![1]), Some(prior(&[0]))));
    assert_eq!(
        event_types(&response),
        ["/test.counter.Noted", "/test.counter.Increased"],
        "the fact as received, then its flags"
    );
    let call = seen.lock().unwrap().pop().unwrap();
    assert_eq!(call.fact, Some(any(NOTED, vec![1])));
    assert_eq!(count(call.state.as_ref()), 1);
    let ctx = call.context.unwrap();
    assert_eq!(ctx.command_id, Some(uuid(3)));
    assert_eq!(
        ctx.fact_origin.map(|o| o.external_id),
        Some("webhook-9".to_string())
    );
    assert_eq!(ctx.recorded_at, Some(recorded_at()));
}

fn command_refused(command_type: &str) -> Any {
    let refused = pb::CommandRefused {
        command: Some(pb::CommandBook {
            cover: Some(cover("inventory")),
            page: Some(pb::CommandPage {
                body: Some(pb::command_page::Body::Command(pb::Command {
                    payload: Some(any(command_type, Vec::new())),
                })),
                ..Default::default()
            }),
            ..Default::default()
        }),
        refusal: Some(any("test.inventory.OutOfStock", vec![8, 1])),
        trigger: None,
    };
    any("io.angzarr.v1.CommandRefused", refused.encode_to_vec())
}

#[test]
fn a_passed_through_refusal_reaches_the_refused_handler_of_its_command_type() {
    let seen: Seen<abi_pb::RefusedCall> = Arc::default();
    let sink = seen.clone();
    let on_refused = host::callback(move |call: abi_pb::RefusedCall| {
        sink.lock().unwrap().push(call);
        Ok(Some(abi_pb::RefusedReturn {
            fact: None,
            flags: vec![any(INCREASED, Vec::new())],
        }))
    });
    let host = hosting(spec(
        pb::ComponentKind::Aggregate,
        vec![
            entry(Role::Fold, "", counting_fold()),
            entry(Role::Refused, RESERVE, on_refused),
        ],
    ));
    let response = handled(&host, fact(command_refused(RESERVE), None));
    assert_eq!(
        event_types(&response),
        ["/io.angzarr.v1.CommandRefused", "/test.counter.Increased"]
    );
    let call = seen.lock().unwrap().pop().unwrap();
    assert_eq!(
        call.refused.and_then(|r| r.refusal),
        Some(any("test.inventory.OutOfStock", vec![8, 1]))
    );
    assert_eq!(call.context.unwrap().command_id, Some(uuid(3)));

    let err = refused(&host, Req::Command(fact(command_refused(START), None)));
    assert_eq!(err.code, codes::NO_REFUSAL_HANDLER);
}

// --- a process manager: a domain with a start rule ----------------------------

fn pm() -> Host {
    let mut pm_spec = spec(
        pb::ComponentKind::ProcessManager,
        vec![
            entry(Role::Fold, "", counting_fold()),
            entry(Role::Decide, START, recording_decide(Arc::default())),
            entry(Role::Decide, ADVANCE, recording_decide(Arc::default())),
            entry(
                Role::Fact,
                TIMED_OUT,
                host::callback(|_: abi_pb::FactCall| Ok(None::<abi_pb::FactReturn>)),
            ),
            entry(Role::Starts, START, 0),
        ],
    );
    pm_spec.name = "fulfillment".to_string();
    pm_spec.domain = "fulfillment".to_string();
    hosting(pm_spec)
}

fn arrival(body: pb::command_page::Body, exists: bool, recorded: Vec<pb::EventPage>) -> Req {
    Req::Admit(abi_pb::AdmitRequest {
        command: Some(pb::CommandBook {
            cover: Some(cover("fulfillment")),
            page: Some(pb::CommandPage {
                body: Some(body),
                command_id: Some(uuid(4)),
                ..Default::default()
            }),
            ..Default::default()
        }),
        recorded,
        stream_exists: exists,
    })
}

fn a_command(fq: &str) -> pb::command_page::Body {
    pb::command_page::Body::Command(pb::Command {
        payload: Some(any(fq, Vec::new())),
    })
}

fn admission(host: &Host, request: Req) -> abi_pb::admission::Outcome {
    match host.dispatch(request).expect("admitted") {
        Resp::Admit(abi_pb::Admission {
            outcome: Some(outcome),
        }) => outcome,
        other => panic!("answered {other:?}"),
    }
}

#[test]
fn a_process_manager_describes_its_starting_types() {
    let Resp::Describe(descriptor) = pm()
        .dispatch(Req::Describe(Default::default()))
        .expect("described")
    else {
        panic!("another arm");
    };
    assert_eq!(descriptor.kind, pb::ComponentKind::ProcessManager as i32);
    assert_eq!(descriptor.starting_types, [START]);
}

#[test]
fn admit_applies_the_start_rule_deduplication_and_skip() {
    use abi_pb::admission::Outcome;
    let host = pm();
    assert!(matches!(
        admission(&host, arrival(a_command(START), false, vec![])),
        Outcome::Dispatch(_)
    ));
    assert!(matches!(
        admission(&host, arrival(a_command(ADVANCE), true, vec![])),
        Outcome::Dispatch(_)
    ));
    let err = refused(&host, arrival(a_command(ADVANCE), false, vec![]));
    assert_eq!(err.code, codes::PM_NO_INSTANCE);
    assert_eq!(err.grpc, GrpcCode::FailedPrecondition);
    assert!(matches!(
        admission(
            &host,
            arrival(
                pb::command_page::Body::Fact(pb::Fact {
                    payload: Some(any(TIMED_OUT, Vec::new())),
                }),
                false,
                vec![]
            )
        ),
        Outcome::Skip(_)
    ));
    let recorded = vec![page(4, INCREASED)];
    assert_eq!(
        admission(&host, arrival(a_command(START), true, recorded.clone())),
        Outcome::Duplicate(abi_pb::admission::Duplicate { events: recorded })
    );
}

// --- a policy -----------------------------------------------------------------

fn policy_spec(roles: Vec<RoleEntry>) -> ComponentSpec {
    ComponentSpec {
        kind: pb::ComponentKind::Policy as i32,
        name: "order-inventory".to_string(),
        domain: "order".to_string(),
        output_domain: "inventory".to_string(),
        rules_version: "1.2.3".to_string(),
        roles,
        ..Default::default()
    }
}

fn reserve_for(trigger: &pb::EventMeta) -> pb::CommandBook {
    pb::CommandBook {
        cover: Some(cover("inventory")),
        page: Some(pb::CommandPage {
            body: Some(a_command(RESERVE)),
            correlation_id: trigger.correlation_id.clone(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn order_placed_source() -> pb::EventBook {
    pb::EventBook {
        cover: Some(cover("order")),
        pages: vec![pb::EventPage {
            sequence: 4,
            payload: Some(any(ORDER_PLACED, vec![1])),
            meta: Some(pb::EventMeta {
                correlation_id: Some(uuid(2)),
                ..Default::default()
            }),
        }],
        ..Default::default()
    }
}

#[test]
fn a_policy_event_is_handed_its_place_and_its_commands_are_answered() {
    let seen: Seen<abi_pb::PolicyEventCall> = Arc::default();
    let sink = seen.clone();
    let translate = host::callback(move |call: abi_pb::PolicyEventCall| {
        let command = reserve_for(call.meta.as_ref().unwrap());
        sink.lock().unwrap().push(call);
        Ok(Some(abi_pb::PolicyEventReturn {
            commands: vec![command],
        }))
    });
    let host = hosting(policy_spec(vec![entry(
        Role::PolicyEvent,
        ORDER_PLACED,
        translate,
    )]));
    let Resp::Policy(response) = host
        .dispatch(Req::Policy(order_placed_source()))
        .expect("translated")
    else {
        panic!("another arm");
    };
    assert_eq!(response.commands.len(), 1);
    let call = seen.lock().unwrap().pop().unwrap();
    assert_eq!(call.event, Some(any(ORDER_PLACED, vec![1])));
    assert_eq!(call.source_cover, Some(cover("order")));
    assert_eq!(call.sequence, 4);
    assert_eq!(call.meta.and_then(|m| m.correlation_id), Some(uuid(2)));
}

fn refusal_of(command_type: &str) -> pb::Refusal {
    pb::Refusal {
        event: Some(order_placed_source()),
        command: Some(pb::CommandBook {
            cover: Some(cover("inventory")),
            page: Some(pb::CommandPage {
                body: Some(a_command(command_type)),
                command_id: Some(uuid(5)),
                ..Default::default()
            }),
            ..Default::default()
        }),
        response: Some(pb::RefusalResponse {
            reason: Some(pb::refusal_response::Reason::Refusal(any(
                "test.inventory.OutOfStock",
                vec![8, 1],
            ))),
        }),
    }
}

fn fact_payload(response: &pb::OnRefusedResponse) -> Any {
    match response
        .fact
        .as_ref()
        .and_then(|f| f.page.as_ref())
        .and_then(|p| p.body.as_ref())
    {
        Some(pb::command_page::Body::Fact(fact)) => fact.payload.clone().unwrap(),
        other => panic!("no fact: {other:?}"),
    }
}

#[test]
fn a_refusal_with_no_translating_callback_is_passed_through_as_command_refused() {
    let host = hosting(policy_spec(vec![]));
    let Resp::Refusal(response) = host
        .dispatch(Req::Refusal(abi_pb::RefusalRequest {
            refusal: Some(refusal_of(RESERVE)),
            delivered: None,
        }))
        .expect("answered")
    else {
        panic!("another arm");
    };
    let payload = fact_payload(&response);
    assert_eq!(payload.type_url, "/io.angzarr.v1.CommandRefused");
    let refused = pb::CommandRefused::decode(payload.value.as_slice()).unwrap();
    assert_eq!(
        refused.refusal,
        Some(any("test.inventory.OutOfStock", vec![8, 1]))
    );
}

#[test]
fn a_translating_callback_is_handed_the_refusal_and_the_delivered_context() {
    let seen: Seen<abi_pb::PolicyRefusedCall> = Arc::default();
    let sink = seen.clone();
    let translate = host::callback(move |call: abi_pb::PolicyRefusedCall| {
        sink.lock().unwrap().push(call);
        Ok(Some(abi_pb::PolicyRefusedReturn {
            fact: Some(any("test.order.ReservationFailed", vec![2])),
        }))
    });
    let host = hosting(policy_spec(vec![entry(
        Role::PolicyRefused,
        RESERVE,
        translate,
    )]));
    let mut delivered = command(RESERVE, Some(prior(&[0, 1, 2])));
    delivered.command = refusal_of(RESERVE).command;
    let Resp::Refusal(response) = host
        .dispatch(Req::Refusal(abi_pb::RefusalRequest {
            refusal: Some(refusal_of(RESERVE)),
            delivered: Some(delivered),
        }))
        .expect("translated")
    else {
        panic!("another arm");
    };
    assert_eq!(
        fact_payload(&response),
        any("test.order.ReservationFailed", vec![2])
    );
    let call = seen.lock().unwrap().pop().unwrap();
    assert_eq!(call.refusal, Some(refusal_of(RESERVE)));
    let ctx = call.context.unwrap();
    assert_eq!(ctx.next_sequence, 3, "built from the delivered command");
    assert!(ctx.had_prior_events);
    assert_eq!(ctx.command_id, Some(uuid(5)));
    assert_eq!(ctx.recorded_at, Some(recorded_at()));
}

// --- a projector ---------------------------------------------------------------

fn projector_spec(project: u64) -> ComponentSpec {
    ComponentSpec {
        kind: pb::ComponentKind::Projector as i32,
        name: "counter-view".to_string(),
        domains: vec!["counter".to_string()],
        state_type: PROJECTION.to_string(),
        rules_version: "1.2.3".to_string(),
        roles: vec![entry(Role::Project, "", project)],
        ..Default::default()
    }
}

#[test]
fn a_projector_folds_the_book_from_its_default_and_the_router_answers_the_projection() {
    let seen: Seen<abi_pb::ProjectCall> = Arc::default();
    let sink = seen.clone();
    let project = host::callback(move |call: abi_pb::ProjectCall| {
        let n = call.pages.len() as u32;
        sink.lock().unwrap().push(call);
        Ok(Some(abi_pb::ProjectReturn {
            outcome: Some(abi_pb::project_return::Outcome::Projection(any(
                PROJECTION,
                CounterState { count: n }.encode_to_vec(),
            ))),
        }))
    });
    let host = hosting(projector_spec(project));
    let Resp::Project(projection) = host
        .dispatch(Req::Project(prior(&[0, 1, 2])))
        .expect("projected")
    else {
        panic!("another arm");
    };
    assert_eq!(projection.projector, "counter-view");
    assert_eq!(projection.cover, Some(cover("counter")));
    assert_eq!(projection.next_sequence, 3);
    assert_eq!(count(projection.projection.as_ref()), 3);
    let call = seen.lock().unwrap().pop().unwrap();
    assert_eq!(call.projection, Some(any(PROJECTION, Vec::new())));
    assert_eq!(call.pages.len(), 3);
    assert_eq!(call.cover, Some(cover("counter")));
}

#[test]
fn a_failing_projection_is_fold_failed_with_its_cause() {
    let project = host::callback(|_: abi_pb::ProjectCall| {
        Ok(Some(abi_pb::ProjectReturn {
            outcome: Some(abi_pb::project_return::Outcome::Failure(
                abi_pb::FoldFailure {
                    page_index: Some(0),
                    cause: Some(pb::Diagnostic {
                        code: "UNPARSEABLE".to_string(),
                        args: Default::default(),
                    }),
                },
            )),
        }))
    });
    let host = hosting(projector_spec(project));
    let err = refused(&host, Req::Project(prior(&[0, 1])));
    assert_eq!(err.code, codes::FOLD_FAILED);
    assert_eq!(
        err.extras,
        args(&[
            (extras::SEQUENCE, "0"),
            (extras::TYPE_URL, "/test.counter.Increased"),
            (extras::STATE_TYPE_URL, "/test.counter.CounterView"),
            (extras::CAUSE, "UNPARSEABLE"),
        ])
    );
}
