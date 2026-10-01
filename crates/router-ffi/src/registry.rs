//! Component descriptors → core dispatch tables. This is pure table
//! population: every closure built here only marshals across the
//! callback; the semantics stay in angzarr-router.

use std::cell::Cell;
use std::ffi::c_void;

use prost::Message;

use angzarr_router::aggregate::{AggregateDispatch, CommandContext};
use angzarr_router::error::{codes, messages, CodedError, HandlerError};
use angzarr_router::process_manager::{
    merge_response, select_process_managers, ProcessManagerDispatch, ProcessManagerRoute,
};
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

fn command_context_aux(cctx: &CommandContext) -> abi_pb::CommandContextAux {
    abi_pb::CommandContextAux {
        next_sequence: cctx.next_sequence,
        had_prior_events: cctx.had_prior_events,
        cover: cctx.cover.clone(),
    }
}

fn host_error(ret: i32, bytes: Option<Vec<u8>>) -> HandlerError {
    HandlerError::Coded(status_to_coded(bytes.as_deref(), ret))
}

/// One registered aggregate: its domain, dispatch table, the host callback
/// that packs its state for Replay (when supported), and the host gateway.
struct RegisteredAggregate {
    domain: String,
    dispatch: AggregateDispatch<()>,
    state_callback: Option<u64>,
    cb: AngzarrCb,
}

/// The registered tables behind an opaque router handle.
pub struct FfiRouter {
    aggregates: Vec<RegisteredAggregate>,
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
        // Commands route by domain, so a second aggregate for a domain could
        // never receive a command: refuse the claim at registration (C-0010).
        if self.aggregates.iter().any(|a| a.domain == desc.domain) {
            return Err(CodedError::invalid_argument(
                codes::DUPLICATE_REGISTRATION,
                "an aggregate is already registered for this domain",
                [(
                    angzarr_router::error::extras::DOMAIN.to_string(),
                    desc.domain.clone(),
                )],
            ));
        }

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
                let aux = command_context_aux(&cctx).encode_to_vec();
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
                    &rejection.compensates,
                    move |notification, rejection, _, cctx| {
                        let aux = abi_pb::RejectionAux {
                            notification: notification.encode_to_vec(),
                            rejection: rejection.encode_to_vec(),
                            cctx: Some(command_context_aux(&cctx)),
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

        for undo in &desc.undoes {
            let id = undo.callback_id;
            dispatch = dispatch.on_undo(&undo.fq_type, move |notification, compensate, _, cctx| {
                let aux = abi_pb::UndoAux {
                    notification: notification.encode_to_vec(),
                    compensate: compensate.encode_to_vec(),
                    cctx: Some(command_context_aux(&cctx)),
                }
                .encode_to_vec();
                let (ret, bytes) = invoke(cb, id, NOTIFICATION_TYPE_URL, &[], &aux);
                match ret {
                    STATUS_OK => pb::BusinessResponse::decode(bytes.unwrap_or_default().as_slice())
                        .map_err(|_| {
                            HandlerError::Other(
                                "host undo handler returned undecodable BusinessResponse bytes"
                                    .to_string(),
                            )
                        }),
                    STATUS_OK_EMPTY => Ok(pb::BusinessResponse::default()),
                    _ => Err(host_error(ret, bytes)),
                }
            });
        }
        for fact in &desc.facts {
            let id = fact.callback_id;
            dispatch = dispatch.on_fact(&fact.fq_type, move |any, _| {
                let (ret, bytes) = invoke(cb, id, &any.type_url, &any.value, &[]);
                match ret {
                    STATUS_OK => prost_types::Any::decode(bytes.unwrap_or_default().as_slice())
                        .map_err(|_| {
                            HandlerError::Other(
                                "host fact handler returned undecodable Any bytes".to_string(),
                            )
                        }),
                    STATUS_OK_EMPTY => Ok(any.clone()),
                    _ => Err(host_error(ret, bytes)),
                }
            });
        }
        dispatch.validate()?;

        self.aggregates.push(RegisteredAggregate {
            domain: desc.domain,
            dispatch,
            state_callback: desc.state_callback_id,
            cb,
        });
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
        // A projector dispatch returns one Projection per EventBook, so a
        // router hosts at most one projector.
        if !self.projectors.is_empty() {
            return Err(CodedError::invalid_argument(
                codes::DUPLICATE_REGISTRATION,
                "a projector is already registered on this router",
                [],
            ));
        }

        let mut dispatch = ProjectorDispatch::new(desc.name.clone(), || ());
        if !desc.domains.is_empty() {
            dispatch = dispatch.for_domains(desc.domains.clone());
        }
        for event in &desc.events {
            let id = event.callback_id;
            dispatch = dispatch.on_event(&event.fq_type, move |_, any, ctx| {
                let aux = abi_pb::ProjectorEventAux {
                    cover: ctx.cover.cloned(),
                    sequence: ctx.sequence,
                }
                .encode_to_vec();
                let (ret, bytes) = invoke(cb, id, &any.type_url, &any.value, &aux);
                if ret < 0 {
                    return Err(host_error(ret, bytes));
                }
                Ok(())
            });
        }
        if let Some(id) = desc.unknown_callback_id {
            // The unknown-event hook is an observer: its status is not part
            // of the projection outcome.
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
        let dispatch = &self.aggregate_for(domain)?.dispatch;
        let _guard = HostCtxGuard::set(host_ctx);
        let resp = dispatch.dispatch(&req)?;
        Ok(resp.encode_to_vec())
    }

    /// The aggregate claiming `domain`: the one registered for it, else a sole
    /// registered aggregate, else NO_HANDLER_REGISTERED.
    fn aggregate_for(&self, domain: &str) -> Result<&RegisteredAggregate, CodedError> {
        match self.aggregates.iter().find(|a| a.domain == domain) {
            Some(entry) => Ok(entry),
            None if self.aggregates.len() == 1 => Ok(&self.aggregates[0]),
            None => Err(CodedError::invalid_argument(
                codes::NO_HANDLER_REGISTERED,
                "no handler registered for the given (domain, type_url)",
                [(
                    angzarr_router::error::extras::DOMAIN.to_string(),
                    domain.to_string(),
                )],
            )),
        }
    }

    /// Decodes FactRequest bytes, routes to the aggregate claiming the facts'
    /// cover domain, and runs the core's fact handling with the host session
    /// installed. Returns the EventBook of facts to record.
    pub fn dispatch_fact(
        &self,
        host_ctx: *mut c_void,
        request: &[u8],
    ) -> Result<Vec<u8>, CodedError> {
        let req = pb::FactRequest::decode(request).map_err(|_| {
            CodedError::invalid_argument(
                codes::ANY_DECODE_FAILED,
                "failed to decode FactRequest",
                [],
            )
        })?;
        let domain = req
            .facts
            .as_ref()
            .and_then(|f| f.cover.as_ref())
            .map(|c| c.domain.as_str())
            .unwrap_or("");
        let dispatch = &self.aggregate_for(domain)?.dispatch;
        let _guard = HostCtxGuard::set(host_ctx);
        Ok(dispatch.handle_fact(&req)?.encode_to_vec())
    }

    /// Decodes ReplayCall bytes, routes to the aggregate claiming its domain,
    /// rebuilds the host state through the appliers, and has the host pack it
    /// (the state callback). Returns ReplayResponse bytes; an aggregate with no
    /// state callback does not support Replay (NO_HANDLER_REGISTERED).
    pub fn dispatch_replay(
        &self,
        host_ctx: *mut c_void,
        request: &[u8],
    ) -> Result<Vec<u8>, CodedError> {
        let call = abi_pb::ReplayCall::decode(request).map_err(|_| {
            CodedError::invalid_argument(
                codes::ANY_DECODE_FAILED,
                "failed to decode ReplayCall",
                [],
            )
        })?;
        let aggregate = self.aggregate_for(&call.domain)?;
        let Some(id) = aggregate.state_callback else {
            return Err(CodedError::invalid_argument(
                codes::NO_HANDLER_REGISTERED,
                "the aggregate does not support Replay",
                [(
                    angzarr_router::error::extras::DOMAIN.to_string(),
                    call.domain.clone(),
                )],
            ));
        };
        let _guard = HostCtxGuard::set(host_ctx);
        aggregate
            .dispatch
            .replay(&call.request.unwrap_or_default())?;
        let (ret, bytes) = invoke(aggregate.cb, id, "", &[], &[]);
        if ret < 0 {
            return Err(status_to_coded(bytes.as_deref(), ret));
        }
        let state =
            prost_types::Any::decode(bytes.unwrap_or_default().as_slice()).map_err(|_| {
                CodedError::unhandled("host state packer returned undecodable Any bytes")
            })?;
        Ok(pb::ReplayResponse { state: Some(state) }.encode_to_vec())
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
    /// callback-marshaling thunks. Event thunks pass the source cover to the
    /// host, which returns a SagaResponse; the core stamps its commands
    /// deferred.
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
            dispatch = dispatch.on_event(&event.fq_type, move |any, _dests, source_cover| {
                let aux = abi_pb::SagaEventAux {
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

        // Source-shape validation precedes routing: a nil source is
        // MISSING_SAGA_SOURCE and a source with no pages is EMPTY_SAGA_SOURCE,
        // regardless of which (if any) saga consumes its domain. Otherwise an
        // empty/absent source has no cover domain, matches no saga, and would
        // mis-report as NO_HANDLER_REGISTERED.
        let Some(source) = req.source.as_ref() else {
            return Err(CodedError::invalid_argument(
                codes::MISSING_SAGA_SOURCE,
                messages::MISSING_SAGA_SOURCE,
                [],
            ));
        };
        if source.pages.is_empty() {
            return Err(CodedError::invalid_argument(
                codes::EMPTY_SAGA_SOURCE,
                messages::EMPTY_SAGA_SOURCE,
                [],
            ));
        }

        // Route by the source book's domain and merge: every saga consuming
        // that domain runs, each skipping event types it does not declare
        // (spec C-0051), so one router can host several sagas.
        let domain = source
            .cover
            .as_ref()
            .map(|c| c.domain.as_str())
            .unwrap_or("");
        let _guard = HostCtxGuard::set(host_ctx);
        let mut merged = pb::SagaResponse::default();
        let mut matched = false;
        for (_, saga) in self
            .sagas
            .iter()
            .filter(|(_, s)| s.input_domain() == domain)
        {
            matched = true;
            let resp = saga.dispatch(&req)?;
            merged.commands.extend(resp.commands);
            merged.events.extend(resp.events);
        }
        // A source domain no saga consumes is NO_HANDLER_REGISTERED (a PM
        // trigger no PM consumes is a no-op instead, C-0022).
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
    /// callback (host owns the instance); event thunks return a
    /// ProcessManagerHandleResponse whose commands the core stamps deferred;
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

        let mut dispatch = ProcessManagerDispatch::new(
            desc.name.clone(),
            desc.pm_domain.clone(),
            desc.target_domains.clone(),
            rebuilder,
        );

        for event in &desc.events {
            let id = event.callback_id;
            dispatch = dispatch.on_event(
                &event.input_domain,
                &event.fq_type,
                move |any, _state, _dests, trigger_cover| {
                    let aux = abi_pb::PmEventAux {
                        trigger_cover: trigger_cover.cloned(),
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
                },
            );
        }

        for rejection in &desc.rejections {
            for &id in &rejection.callback_ids {
                dispatch = dispatch.on_rejected(
                    &rejection.compensates,
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
                                Ok(resp)
                            }
                            STATUS_OK_EMPTY => Ok(pb::ProcessManagerHandleResponse::default()),
                            _ => Err(host_error(ret, bytes)),
                        }
                    },
                );
            }
        }

        dispatch.validate()?;
        self.process_managers.push((desc.name, dispatch));
        Ok(())
    }

    /// Decodes ProcessManagerHandleRequest bytes, routes to the addressed
    /// co-resident PMs, and runs each core dispatch with the host session
    /// installed. Every selected PM runs under the same host_ctx; the host keys
    /// its lazily created state per component (by callback id), so no PM ever
    /// folds into another's state.
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

        // Trigger-shape validation precedes routing (mirrors dispatch_saga): a
        // nil trigger is MISSING_PM_TRIGGER and a trigger with no pages is
        // EMPTY_PM_TRIGGER, regardless of which PM (if any) consumes its domain.
        let Some(trigger) = req.trigger.as_ref() else {
            return Err(CodedError::invalid_argument(
                codes::MISSING_PM_TRIGGER,
                messages::MISSING_PM_TRIGGER,
                [],
            ));
        };
        if trigger.pages.is_empty() {
            return Err(CodedError::invalid_argument(
                codes::EMPTY_PM_TRIGGER,
                messages::EMPTY_PM_TRIGGER,
                [],
            ));
        }

        // Route by identity, then by subscription (select_process_managers):
        // a process-state book and a rejection each belong to one PM; a new
        // workflow's trigger reaches every PM consuming its domain, each
        // no-opping on event types it does not declare (C-0022). Responses
        // merge in registration order (merge_response).
        let routes: Vec<&dyn ProcessManagerRoute> = self
            .process_managers
            .iter()
            .map(|(_, pm)| pm as &dyn ProcessManagerRoute)
            .collect();
        let selected = select_process_managers(&routes, &req);
        let _guard = HostCtxGuard::set(host_ctx);
        let mut merged = pb::ProcessManagerHandleResponse::default();
        for index in selected {
            merge_response(&mut merged, self.process_managers[index].1.dispatch(&req)?);
        }
        // PM tail: an unconsumed trigger domain is a no-op (C-0022), not an error.
        Ok(merged.encode_to_vec())
    }
}

#[cfg(test)]
#[path = "registry.test.rs"]
mod registry_tests;
