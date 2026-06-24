//! Component descriptors → core dispatch tables. This is pure table
//! population: every closure built here only marshals across the
//! callback; the semantics stay in angzarr-router.

use std::cell::Cell;
use std::ffi::c_void;

use prost::Message;

use angzarr_router::aggregate::AggregateDispatch;
use angzarr_router::error::{codes, CodedError, HandlerError};
use angzarr_router::process_manager::ProcessManagerDispatch;
use angzarr_router::projector::ProjectorDispatch;
use angzarr_router::rebuild::Rebuilder;
use angzarr_router::saga::SagaDispatch;
use angzarr_router::{pb, NOTIFICATION_TYPE_URL};

use crate::abi::{consume_out, status_to_coded, AngzarrBuf, AngzarrCb, STATUS_OK, STATUS_OK_EMPTY};
use crate::proto::io::angzarr::router::ffi::v1 as abi_pb;

thread_local! {
    /// The host's per-dispatch session pointer. Set for the duration of
    /// one dispatch; callbacks run synchronously on the dispatching
    /// thread, so distinct dispatches (threads) never observe each
    /// other's session — the state-never-crosses principle made concrete.
    static CURRENT_HOST_CTX: Cell<*mut c_void> = const { Cell::new(std::ptr::null_mut()) };
}

struct HostCtxGuard {
    prev: *mut c_void,
}

impl HostCtxGuard {
    fn set(ctx: *mut c_void) -> Self {
        let prev = CURRENT_HOST_CTX.with(|c| c.replace(ctx));
        HostCtxGuard { prev }
    }
}

impl Drop for HostCtxGuard {
    fn drop(&mut self) {
        CURRENT_HOST_CTX.with(|c| c.set(self.prev));
    }
}

/// Invokes the host callback with the current dispatch's session pointer.
/// Returns the status and any host-filled output (ownership taken).
fn invoke(
    cb: AngzarrCb,
    id: u64,
    type_url: &str,
    payload: &[u8],
    aux: &[u8],
) -> (i32, Option<Vec<u8>>) {
    let mut out = AngzarrBuf {
        data: std::ptr::null_mut(),
        len: 0,
    };
    let host_ctx = CURRENT_HOST_CTX.with(|c| c.get());
    let ret = unsafe {
        cb(
            host_ctx,
            id,
            type_url.as_ptr(),
            type_url.len(),
            payload.as_ptr(),
            payload.len(),
            aux.as_ptr(),
            aux.len(),
            &mut out,
        )
    };
    (ret, consume_out(&mut out))
}

fn host_error(ret: i32, bytes: Option<Vec<u8>>) -> HandlerError {
    HandlerError::Coded(status_to_coded(bytes.as_deref(), ret))
}

/// The registered tables behind an opaque router handle.
pub struct FfiRouter {
    aggregates: Vec<(String, AggregateDispatch<()>)>,
    projectors: Vec<(String, ProjectorDispatch<()>)>,
    sagas: Vec<(String, SagaDispatch)>,
    process_managers: Vec<(String, ProcessManagerDispatch<()>)>,
}

impl FfiRouter {
    pub fn new() -> Self {
        FfiRouter {
            aggregates: Vec::new(),
            projectors: Vec::new(),
            sagas: Vec::new(),
            process_managers: Vec::new(),
        }
    }

    /// Parses an AggregateDescriptor and populates the core tables with
    /// callback-marshaling thunks.
    pub fn register_aggregate(
        &mut self,
        descriptor: &[u8],
        cb: AngzarrCb,
    ) -> Result<(), CodedError> {
        let desc = abi_pb::AggregateDescriptor::decode(descriptor).map_err(|_| {
            CodedError::invalid_argument(
                codes::ANY_DECODE_FAILED,
                "failed to decode AggregateDescriptor",
                [],
            )
        })?;

        let mut rebuilder: Rebuilder<()> = Rebuilder::new(|| ());
        for applier in &desc.appliers {
            let id = applier.callback_id;
            rebuilder = rebuilder.apply(&applier.fq_type, move |_, any| {
                let (ret, _) = invoke(cb, id, &any.type_url, &any.value, &[]);
                if ret < 0 {
                    return Err("host applier failed".into());
                }
                Ok(())
            });
        }
        if let Some(id) = desc.snapshot_callback_id {
            rebuilder = rebuilder.with_snapshot(move |_, any| {
                let (ret, _) = invoke(cb, id, &any.type_url, &any.value, &[]);
                if ret < 0 {
                    return Err("host snapshot loader failed".into());
                }
                Ok(())
            });
        }

        let mut dispatch =
            AggregateDispatch::new(desc.name.clone(), desc.domain.clone(), rebuilder);

        for command in &desc.commands {
            let id = command.callback_id;
            dispatch = dispatch.on_command(&command.fq_type, move |cmd, _, cctx| {
                let aux = abi_pb::CommandContextAux {
                    next_sequence: cctx.next_sequence,
                    had_prior_events: cctx.had_prior_events,
                }
                .encode_to_vec();
                let (ret, bytes) = invoke(cb, id, &cmd.type_url, &cmd.value, &aux);
                match ret {
                    STATUS_OK => {
                        let book = pb::EventBook::decode(bytes.unwrap_or_default().as_slice())
                            .map_err(|_| {
                                HandlerError::Other(
                                    "host handler returned undecodable EventBook bytes".to_string(),
                                )
                            })?;
                        Ok(Some(book))
                    }
                    STATUS_OK_EMPTY => Ok(None),
                    _ => Err(host_error(ret, bytes)),
                }
            });
        }

        for rejection in &desc.rejections {
            for &id in &rejection.callback_ids {
                dispatch = dispatch.on_rejected(
                    &rejection.fq_command_type,
                    move |notification, rejection, _, cctx| {
                        let aux = abi_pb::RejectionAux {
                            notification: notification.encode_to_vec(),
                            rejection: rejection.encode_to_vec(),
                            cctx: Some(abi_pb::CommandContextAux {
                                next_sequence: cctx.next_sequence,
                                had_prior_events: cctx.had_prior_events,
                            }),
                        }
                        .encode_to_vec();
                        let (ret, bytes) = invoke(cb, id, NOTIFICATION_TYPE_URL, &[], &aux);
                        match ret {
                            STATUS_OK => {
                                let resp = pb::BusinessResponse::decode(
                                    bytes.unwrap_or_default().as_slice(),
                                )
                                .map_err(|_| {
                                    HandlerError::Other(
                                        "host compensator returned undecodable BusinessResponse bytes"
                                            .to_string(),
                                    )
                                })?;
                                Ok(resp)
                            }
                            STATUS_OK_EMPTY => Ok(pb::BusinessResponse::default()),
                            _ => Err(host_error(ret, bytes)),
                        }
                    },
                );
            }
        }

        self.aggregates.push((desc.domain, dispatch));
        Ok(())
    }

    /// Parses a ProjectorDescriptor and populates a core projector table
    /// with callback-marshaling thunks. The host owns the projection
    /// instance (parked in host_ctx); folds and finish cross the callback.
    pub fn register_projector(
        &mut self,
        descriptor: &[u8],
        cb: AngzarrCb,
    ) -> Result<(), CodedError> {
        let desc = abi_pb::ProjectorDescriptor::decode(descriptor).map_err(|_| {
            CodedError::invalid_argument(
                codes::ANY_DECODE_FAILED,
                "failed to decode ProjectorDescriptor",
                [],
            )
        })?;

        let mut dispatch = ProjectorDispatch::new(desc.name.clone(), || ());
        if !desc.domains.is_empty() {
            dispatch = dispatch.for_domains(desc.domains.clone());
        }
        for event in &desc.events {
            let id = event.callback_id;
            dispatch = dispatch.on_event(&event.fq_type, move |_, any| {
                let (ret, _) = invoke(cb, id, &any.type_url, &any.value, &[]);
                if ret < 0 {
                    return Err(HandlerError::Other("host fold failed".to_string()));
                }
                Ok(())
            });
        }
        if let Some(id) = desc.unknown_callback_id {
            dispatch = dispatch.on_unknown(move |type_url| {
                invoke(cb, id, type_url, &[], &[]);
            });
        }
        if let Some(id) = desc.finish_callback_id {
            dispatch = dispatch.finish(move |_, events| {
                let book = events.encode_to_vec();
                let (ret, bytes) = invoke(cb, id, "", &book, &[]);
                match ret {
                    STATUS_OK | STATUS_OK_EMPTY => {
                        pb::Projection::decode(bytes.unwrap_or_default().as_slice()).map_err(|_| {
                            HandlerError::Other(
                                "host finisher returned undecodable Projection bytes".to_string(),
                            )
                        })
                    }
                    _ => Err(host_error(ret, bytes)),
                }
            });
        }

        self.projectors.push((desc.name, dispatch));
        Ok(())
    }

    /// Decodes ContextualCommand bytes, routes to the claiming aggregate
    /// (by cover domain; a sole registered aggregate claims everything),
    /// and runs the core dispatch with the host session installed.
    pub fn dispatch(&self, host_ctx: *mut c_void, request: &[u8]) -> Result<Vec<u8>, CodedError> {
        let req = pb::ContextualCommand::decode(request).map_err(|_| {
            CodedError::invalid_argument(
                codes::ANY_DECODE_FAILED,
                "failed to decode ContextualCommand",
                [],
            )
        })?;

        let domain = req
            .command
            .as_ref()
            .and_then(|c| c.cover.as_ref())
            .map(|c| c.domain.as_str())
            .unwrap_or("");
        let dispatch = match self.aggregates.iter().find(|(d, _)| d == domain) {
            Some((_, dispatch)) => dispatch,
            None if self.aggregates.len() == 1 => &self.aggregates[0].1,
            None => {
                return Err(CodedError::invalid_argument(
                    codes::NO_HANDLER_REGISTERED,
                    "no handler registered for the given (domain, type_url)",
                    [(
                        angzarr_router::error::extras::DOMAIN.to_string(),
                        domain.to_string(),
                    )],
                ));
            }
        };

        let _guard = HostCtxGuard::set(host_ctx);
        let resp = dispatch.dispatch(&req)?;
        Ok(resp.encode_to_vec())
    }

    /// Decodes EventBook bytes, routes to the registered projector (sole
    /// projector claims everything; the core applies its own domain filter),
    /// and runs the core dispatch with the host session installed.
    pub fn dispatch_projector(
        &self,
        host_ctx: *mut c_void,
        request: &[u8],
    ) -> Result<Vec<u8>, CodedError> {
        let book = pb::EventBook::decode(request).map_err(|_| {
            CodedError::invalid_argument(codes::ANY_DECODE_FAILED, "failed to decode EventBook", [])
        })?;

        let dispatch = match self.projectors.as_slice() {
            [(_, only)] => only,
            _ => {
                return Err(CodedError::invalid_argument(
                    codes::NO_HANDLER_REGISTERED,
                    "no single projector registered to claim the EventBook",
                    [],
                ));
            }
        };

        let _guard = HostCtxGuard::set(host_ctx);
        let resp = dispatch.dispatch(&book)?;
        Ok(resp.encode_to_vec())
    }

    /// Parses a SagaDescriptor and populates a core saga table with
    /// callback-marshaling thunks. Event thunks pass the coordinator-supplied
    /// destination sequences to the host (which stamps and returns a
    /// SagaResponse); compensators run in registration order (C-0042).
    pub fn register_saga(&mut self, descriptor: &[u8], cb: AngzarrCb) -> Result<(), CodedError> {
        let desc = abi_pb::SagaDescriptor::decode(descriptor).map_err(|_| {
            CodedError::invalid_argument(
                codes::ANY_DECODE_FAILED,
                "failed to decode SagaDescriptor",
                [],
            )
        })?;

        let mut dispatch = SagaDispatch::new(
            desc.name.clone(),
            desc.input_domain.clone(),
            desc.target_domains.clone(),
        );

        for event in &desc.events {
            let id = event.callback_id;
            dispatch = dispatch.on_event(&event.fq_type, move |any, dests, source_cover| {
                let destination_sequences = dests
                    .domains()
                    .into_iter()
                    .filter_map(|d| dests.sequence_for(&d).map(|s| (d, s)))
                    .collect();
                let aux = abi_pb::SagaEventAux {
                    destination_sequences,
                    source_cover: source_cover.cloned(),
                }
                .encode_to_vec();
                let (ret, bytes) = invoke(cb, id, &any.type_url, &any.value, &aux);
                match ret {
                    STATUS_OK => {
                        let resp = pb::SagaResponse::decode(bytes.unwrap_or_default().as_slice())
                            .map_err(|_| {
                            HandlerError::Other(
                                "host saga handler returned undecodable SagaResponse bytes"
                                    .to_string(),
                            )
                        })?;
                        Ok((resp.commands, resp.events))
                    }
                    STATUS_OK_EMPTY => Ok((Vec::new(), Vec::new())),
                    _ => Err(host_error(ret, bytes)),
                }
            });
        }

        for rejection in &desc.rejections {
            for &id in &rejection.callback_ids {
                dispatch = dispatch.on_rejected(
                    &rejection.fq_command_type,
                    move |notification, rejection| {
                        let aux = abi_pb::RejectionAux {
                            notification: notification.encode_to_vec(),
                            rejection: rejection.encode_to_vec(),
                            cctx: None, // sagas are stateless — no CommandContext
                        }
                        .encode_to_vec();
                        let (ret, bytes) = invoke(cb, id, NOTIFICATION_TYPE_URL, &[], &aux);
                        match ret {
                            STATUS_OK => {
                                let resp =
                                    pb::SagaResponse::decode(bytes.unwrap_or_default().as_slice())
                                        .map_err(|_| {
                                            HandlerError::Other(
                                                "host saga compensator returned undecodable \
                                                 SagaResponse bytes"
                                                    .to_string(),
                                            )
                                        })?;
                                Ok(resp.events)
                            }
                            STATUS_OK_EMPTY => Ok(Vec::new()),
                            _ => Err(host_error(ret, bytes)),
                        }
                    },
                );
            }
        }

        self.sagas.push((desc.name, dispatch));
        Ok(())
    }

    /// Decodes SagaHandleRequest bytes, routes to the registered saga (sole
    /// saga claims the source), and runs the core dispatch with the host
    /// session installed.
    pub fn dispatch_saga(
        &self,
        host_ctx: *mut c_void,
        request: &[u8],
    ) -> Result<Vec<u8>, CodedError> {
        let req = pb::SagaHandleRequest::decode(request).map_err(|_| {
            CodedError::invalid_argument(
                codes::ANY_DECODE_FAILED,
                "failed to decode SagaHandleRequest",
                [],
            )
        })?;

        // Route by the source book's domain and merge: every saga consuming
        // that domain runs, each skipping event types it does not declare
        // (spec C-0051). This lets one router host multiple sagas — the
        // in-process coordinator the poker example needs — instead of the
        // single-saga special case.
        let domain = req
            .source
            .as_ref()
            .and_then(|s| s.cover.as_ref())
            .map(|c| c.domain.as_str())
            .unwrap_or("");
        let _guard = HostCtxGuard::set(host_ctx);
        let mut merged = pb::SagaResponse::default();
        let mut matched = false;
        for (_, dispatch) in &self.sagas {
            if dispatch.input_domain() == domain {
                matched = true;
                let resp = dispatch.dispatch(&req)?;
                merged.commands.extend(resp.commands);
                merged.events.extend(resp.events);
            }
        }
        if !matched {
            return Err(CodedError::invalid_argument(
                codes::NO_HANDLER_REGISTERED,
                "no saga registered for the source domain",
                [],
            ));
        }
        Ok(merged.encode_to_vec())
    }

    /// Parses a ProcessManagerDescriptor and populates a core PM table.
    /// Stateful: appliers/snapshot rebuild the PM's own state across the
    /// callback (host owns the instance); event thunks pass destination
    /// sequences to the host and return a ProcessManagerHandleResponse;
    /// compensators run in registration order (C-0042).
    pub fn register_process_manager(
        &mut self,
        descriptor: &[u8],
        cb: AngzarrCb,
    ) -> Result<(), CodedError> {
        let desc = abi_pb::ProcessManagerDescriptor::decode(descriptor).map_err(|_| {
            CodedError::invalid_argument(
                codes::ANY_DECODE_FAILED,
                "failed to decode ProcessManagerDescriptor",
                [],
            )
        })?;

        let mut rebuilder: Rebuilder<()> = Rebuilder::new(|| ());
        for applier in &desc.appliers {
            let id = applier.callback_id;
            rebuilder = rebuilder.apply(&applier.fq_type, move |_, any| {
                let (ret, _) = invoke(cb, id, &any.type_url, &any.value, &[]);
                if ret < 0 {
                    return Err("host applier failed".into());
                }
                Ok(())
            });
        }
        if let Some(id) = desc.snapshot_callback_id {
            rebuilder = rebuilder.with_snapshot(move |_, any| {
                let (ret, _) = invoke(cb, id, &any.type_url, &any.value, &[]);
                if ret < 0 {
                    return Err("host snapshot loader failed".into());
                }
                Ok(())
            });
        }

        let mut dispatch =
            ProcessManagerDispatch::new(desc.name.clone(), desc.pm_domain.clone(), rebuilder);

        for event in &desc.events {
            let id = event.callback_id;
            dispatch = dispatch.on_event(&event.input_domain, &event.fq_type, move |any, _state, dests| {
                let destination_sequences = dests
                    .domains()
                    .into_iter()
                    .filter_map(|d| dests.sequence_for(&d).map(|s| (d, s)))
                    .collect();
                let aux = abi_pb::PmEventAux {
                    destination_sequences,
                }
                .encode_to_vec();
                let (ret, bytes) = invoke(cb, id, &any.type_url, &any.value, &aux);
                match ret {
                    STATUS_OK => pb::ProcessManagerHandleResponse::decode(
                        bytes.unwrap_or_default().as_slice(),
                    )
                    .map_err(|_| {
                        HandlerError::Other(
                            "host PM handler returned undecodable \
                             ProcessManagerHandleResponse bytes"
                                .to_string(),
                        )
                    }),
                    STATUS_OK_EMPTY => Ok(pb::ProcessManagerHandleResponse::default()),
                    _ => Err(host_error(ret, bytes)),
                }
            });
        }

        for rejection in &desc.rejections {
            for &id in &rejection.callback_ids {
                dispatch = dispatch.on_rejected(
                    &rejection.fq_command_type,
                    move |notification, rejection, _state| {
                        let aux = abi_pb::RejectionAux {
                            notification: notification.encode_to_vec(),
                            rejection: rejection.encode_to_vec(),
                            cctx: None, // PM compensators read rebuilt state, not CommandContext
                        }
                        .encode_to_vec();
                        let (ret, bytes) = invoke(cb, id, NOTIFICATION_TYPE_URL, &[], &aux);
                        match ret {
                            STATUS_OK => {
                                let resp = pb::ProcessManagerHandleResponse::decode(
                                    bytes.unwrap_or_default().as_slice(),
                                )
                                .map_err(|_| {
                                    HandlerError::Other(
                                        "host PM compensator returned undecodable \
                                         ProcessManagerHandleResponse bytes"
                                            .to_string(),
                                    )
                                })?;
                                Ok((resp.process_events, resp.notification))
                            }
                            STATUS_OK_EMPTY => Ok((Vec::new(), None)),
                            _ => Err(host_error(ret, bytes)),
                        }
                    },
                );
            }
        }

        self.process_managers.push((desc.name, dispatch));
        Ok(())
    }

    /// Decodes ProcessManagerHandleRequest bytes, routes to the registered PM
    /// (sole PM claims the trigger), and runs the core dispatch with the host
    /// session installed.
    pub fn dispatch_process_manager(
        &self,
        host_ctx: *mut c_void,
        request: &[u8],
    ) -> Result<Vec<u8>, CodedError> {
        let req = pb::ProcessManagerHandleRequest::decode(request).map_err(|_| {
            CodedError::invalid_argument(
                codes::ANY_DECODE_FAILED,
                "failed to decode ProcessManagerHandleRequest",
                [],
            )
        })?;

        let dispatch = match self.process_managers.as_slice() {
            [(_, only)] => only,
            _ => {
                return Err(CodedError::invalid_argument(
                    codes::NO_HANDLER_REGISTERED,
                    "no single process manager registered to claim the trigger",
                    [],
                ));
            }
        };

        let _guard = HostCtxGuard::set(host_ctx);
        let resp = dispatch.dispatch(&req)?;
        Ok(resp.encode_to_vec())
    }
}
