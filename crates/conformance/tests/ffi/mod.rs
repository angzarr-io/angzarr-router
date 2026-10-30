//! The conformance fixtures behind the router's C ABI: each registered as a
//! binding registers it (one ComponentSpec, role callbacks through
//! angzarr-router-ffi's Rust host) and dispatched through
//! `angzarr_router_dispatch`, so a `*_ffi` test target runs the very same
//! feature and steps as its native twin and any drift between the ABI and
//! the engine fails it.
//!
//! The business logic is the native fixture's (crates/conformance/src),
//! restated as the role callbacks a generated binding would call.

use prost::Message;
use prost_types::Any;

use angzarr_router::error::{BusinessStatus, CodedError};
use angzarr_router::pb;
use angzarr_router_conformance::{self as conf, counter, ObservedSink};
use angzarr_router_ffi::abi_pb::{self, dispatch_request::Request, dispatch_response::Response};
use angzarr_router_ffi::host::{self, Host};

/// Whether this test binary is a `*_ffi` target: its steps then drive the
/// fixtures through the C ABI rather than the engine directly.
pub fn through_ffi() -> bool {
    env!("CARGO_CRATE_NAME").ends_with("_ffi")
}

fn role(role: abi_pb::Role, fq_type: &str, callback_id: u64) -> abi_pb::RoleEntry {
    abi_pb::RoleEntry {
        role: role as i32,
        fq_type: fq_type.to_string(),
        callback_id,
    }
}

fn counter_state(state: Option<&Any>) -> Result<counter::CounterState, CodedError> {
    match state {
        None => Ok(counter::CounterState::default()),
        Some(any) => {
            counter::CounterState::decode(any.value.as_slice()).map_err(|_| CodedError::unhandled())
        }
    }
}

fn state_any(state: &counter::CounterState) -> Any {
    Any {
        type_url: angzarr_router::type_url("test.counter.CounterState"),
        value: state.encode_to_vec(),
    }
}

/// The CounterAggregate's fold: each Increased page counts one; a page that
/// does not decode fails the fold at its index.
fn counter_fold(call: abi_pb::FoldCall) -> Result<Option<abi_pb::FoldReturn>, CodedError> {
    use abi_pb::fold_return::Outcome;
    let mut state = counter_state(call.state.as_ref())?;
    for (index, page) in call.pages.iter().enumerate() {
        let Some(event) = page.payload.as_ref() else {
            continue;
        };
        if angzarr_router::type_name_from_url(&event.type_url) != "test.counter.Increased" {
            continue;
        }
        if counter::Increased::decode(event.value.as_slice()).is_err() {
            return Ok(Some(abi_pb::FoldReturn {
                outcome: Some(Outcome::Failure(abi_pb::FoldFailure {
                    page_index: Some(index as u32),
                    cause: Some(pb::Diagnostic {
                        code: "ANY_DECODE_FAILED".to_string(),
                        args: [(
                            "message_type".to_string(),
                            "test.counter.Increased".to_string(),
                        )]
                        .into(),
                    }),
                })),
            }));
        }
        state.count += 1;
    }
    Ok(Some(abi_pb::FoldReturn {
        outcome: Some(Outcome::State(state_any(&state))),
    }))
}

/// The CounterAggregate's IncreaseBy: records what it saw; n > 0 emits n
/// Increased, n == 0 is rejected VALUE_NOT_POSITIVE.
fn increase_by(
    observed: &ObservedSink,
    call: abi_pb::DecideCall,
) -> Result<Option<abi_pb::DecideReturn>, CodedError> {
    let state = counter_state(call.state.as_ref())?;
    let ctx = call.context.unwrap_or_default();
    observed.lock().unwrap().push(conf::Observed {
        next_sequence: ctx.next_sequence,
        had_prior_events: ctx.had_prior_events,
        count: state.count,
    });
    let command = call.command.unwrap_or_default();
    let cmd = counter::IncreaseBy::decode(command.value.as_slice())
        .map_err(|_| CodedError::unhandled())?;
    if cmd.n == 0 {
        return Err(CodedError::rejection(
            BusinessStatus::FailedPrecondition,
            conf::VALUE_NOT_POSITIVE,
            [],
        ));
    }
    let increased = Any {
        type_url: angzarr_router::type_url("test.counter.Increased"),
        value: counter::Increased {}.encode_to_vec(),
    };
    Ok(Some(abi_pb::DecideReturn {
        events: Some(pb::EventBook {
            pages: (0..cmd.n)
                .map(|_| pb::EventPage {
                    payload: Some(increased.clone()),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }),
        resolved_nonce: Vec::new(),
    }))
}

/// The CounterAggregate as a ComponentSpec over role callbacks.
fn counter_spec(observed: ObservedSink) -> abi_pb::ComponentSpec {
    use abi_pb::Role;
    let fold = host::callback(counter_fold);
    let increase = host::callback(move |call| increase_by(&observed, call));
    // An unclassified failure: what a binding answers for an exception that
    // is neither a rejection nor a bug.
    let fail_hard = host::callback(|_: abi_pb::DecideCall| {
        Err::<Option<abi_pb::DecideReturn>, _>(CodedError::unhandled())
    });
    abi_pb::ComponentSpec {
        kind: pb::ComponentKind::Aggregate as i32,
        name: "counter-aggregate".to_string(),
        domain: "counter".to_string(),
        state_type: "test.counter.CounterState".to_string(),
        rules_version: "conformance".to_string(),
        roles: vec![
            role(Role::Fold, "", fold),
            role(Role::Decide, "test.counter.IncreaseBy", increase),
            role(Role::Decide, "test.counter.FailHard", fail_hard),
        ],
        ..Default::default()
    }
}

/// Dispatches `cc` to a fresh CounterAggregate behind the C ABI.
pub fn dispatch_counter(
    observed: ObservedSink,
    cc: &pb::ContextualCommand,
) -> Result<pb::CommandHandlerResponse, CodedError> {
    let mut host = Host::new();
    host.register(&counter_spec(observed))?;
    match host.dispatch(Request::Command(cc.clone()))? {
        Response::Command(response) => Ok(response),
        other => panic!("a command answered in another arm: {other:?}"),
    }
}
