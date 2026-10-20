//! Component descriptors → core dispatch tables. This is pure table
//! population: every closure built here only marshals across the
//! callback; the semantics stay in angzarr-router.

use prost::Message;
use prost_types::Any;

use angzarr_router::aggregate::{AggregateDispatch, CommandContext, FactRecord};
use angzarr_router::error::{codes, messages, CodedError, HandlerError};
use angzarr_router::pb;
use angzarr_router::process_manager::ProcessManagerDispatch;
use angzarr_router::projector::ProjectorDispatch;
use angzarr_router::rebuild::{FoldFailure, Rebuilder};
use angzarr_router::router::{Router, RouterBuilder};
use angzarr_router::saga::SagaDispatch;
use angzarr_router::upcaster::UpcasterDispatch;

use crate::abi::{consume_out, status_to_coded, AngzarrBuf, AngzarrCb, STATUS_OK, STATUS_OK_EMPTY};
use crate::proto::io::angzarr::router::ffi::v1 as abi_pb;

/// Invokes the host callback.
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
    let ret = unsafe {
        cb(
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

fn command_context_aux(cctx: &CommandContext, state: &Any) -> abi_pb::CommandContextAux {
    abi_pb::CommandContextAux {
        state: Some(state.clone()),
        next_sequence: cctx.next_sequence,
        had_prior_events: cctx.had_prior_events,
        cover: cctx.cover.clone(),
        expected_sequence: cctx.expected_sequence,
        conflict_resolution: cctx.conflict_resolution as i32,
        conflict_nonce: cctx
            .conflict
            .as_ref()
            .map(|c| c.nonce().to_vec())
            .unwrap_or_default(),
        window: cctx
            .conflict
            .as_ref()
            .map(|c| c.window().to_vec())
            .unwrap_or_default(),
    }
}

fn host_error(ret: i32, bytes: Option<Vec<u8>>) -> HandlerError {
    HandlerError::Coded(status_to_coded(bytes.as_deref(), ret))
}

/// Calls the host fold over `pages` from `state` (cover optional), folding
/// its answer into `state`: STATUS_OK carries the new state, STATUS_OK_EMPTY
/// keeps it. A failure carries the host's FoldFailure (abi.proto), read
/// from the one call; missing or undecodable failure output, or an
/// undecodable answer, is a failure at no known page. The caller turns it
/// into the coded error; the fold is never called again.
fn host_fold(
    cb: AngzarrCb,
    id: u64,
    state: &mut Any,
    pages: &[pb::EventPage],
    cover: Option<&pb::Cover>,
) -> Result<(), FoldFailure> {
    let call = abi_pb::FoldCall {
        state: Some(state.clone()),
        pages: pages.to_vec(),
        cover: cover.cloned(),
    }
    .encode_to_vec();
    let (ret, bytes) = invoke(cb, id, "", &call, &[]);
    match ret {
        STATUS_OK => {
            *state =
                Any::decode(bytes.unwrap_or_default().as_slice()).map_err(|_| FoldFailure {
                    page_index: None,
                    message: "host fold returned undecodable Any bytes".to_string(),
                })?;
            Ok(())
        }
        STATUS_OK_EMPTY => Ok(()),
        _ => Err(bytes
            .and_then(|out| abi_pb::FoldFailure::decode(out.as_slice()).ok())
            .map(|failure| FoldFailure {
                page_index: failure.page_index.map(|index| index as usize),
                message: failure.message,
            })
            .unwrap_or_default()),
    }
}

/// A rebuilder over router-owned state: a serialized `Any` of
/// `state_type_url`, starting from the message's default (empty bytes) or a
/// delivered snapshot's state, folded through the host's stateless fold
/// callback in one batched call per rebuild (abi.proto, FoldCall). A failed
/// fold is a handler bug (INTERNAL) naming the page the host's FoldFailure
/// indexes, or the batch's range.
fn state_rebuilder(state_type_url: &str, fold: Option<u64>, cb: AngzarrCb) -> Rebuilder<Any> {
    let initial = Any {
        type_url: angzarr_router::type_url(state_type_url),
        value: Vec::new(),
    };
    let mut rebuilder = Rebuilder::new(move || initial.clone())
        .with_state_type(state_type_url)
        .with_state_image(|state: &Any| state.clone())
        .with_snapshot(|state, snapshot| {
            *state = snapshot.clone();
            Ok(())
        });
    if let Some(id) = fold {
        rebuilder = rebuilder
            .with_fold(move |state: &mut Any, pages, cover| host_fold(cb, id, state, pages, cover));
    }
    rebuilder
}

/// The one component behind an opaque router handle (owner ruling: one
/// coordinator, one component, one business process), held as the core's
/// [`Router`]. Registering a second component, of any kind, is refused
/// DUPLICATE_REGISTRATION.
pub struct FfiRouter {
    router: Option<Router>,
}

impl FfiRouter {
    pub fn new() -> Self {
        FfiRouter { router: None }
    }

    /// The component's registration hello (types.proto ComponentDescriptor,
    /// the one ComponentService.Describe answers) as bytes; refused
    /// MISSING_RULES_VERSION or MISSING_STATE_TYPE when it cannot be built,
    /// and NO_HANDLERS_REGISTERED before a component registers.
    pub fn component_descriptor(&self) -> Result<Vec<u8>, CodedError> {
        Ok(self.router()?.component_descriptor()?.encode_to_vec())
    }

    /// The hosted component's router; NO_HANDLER_REGISTERED before one
    /// registers.
    fn router(&self) -> Result<&Router, CodedError> {
        self.router.as_ref().ok_or_else(|| {
            CodedError::invalid_argument(
                codes::NO_HANDLERS_REGISTERED,
                messages::NO_HANDLERS_REGISTERED,
                [],
            )
        })
    }

    /// Refuses a second component (DUPLICATE_REGISTRATION).
    fn refuse_a_second(&self) -> Result<(), CodedError> {
        if self.router.is_some() {
            return Err(CodedError::invalid_argument(
                codes::DUPLICATE_REGISTRATION,
                "this router already hosts its one component",
                [],
            ));
        }
        Ok(())
    }

    /// Hosts the component `builder` holds, once its build validates.
    fn host(&mut self, builder: RouterBuilder) -> Result<(), CodedError> {
        self.router = Some(builder.build()?);
        Ok(())
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
        self.refuse_a_second()?;
        let rebuilder = state_rebuilder(&desc.state_type_url, desc.fold_callback_id, cb);

        let mut dispatch =
            AggregateDispatch::new(desc.name.clone(), desc.domain.clone(), rebuilder)
                .with_rules_version(desc.rules_version.clone());

        for command in &desc.commands {
            let id = command.callback_id;
            dispatch = dispatch.on_command(&command.fq_type, move |cmd, state, cctx| {
                let aux = command_context_aux(&cctx, state).encode_to_vec();
                let (ret, bytes) = invoke(cb, id, &cmd.type_url, &cmd.value, &aux);
                match ret {
                    STATUS_OK => {
                        let output =
                            abi_pb::CommandOutput::decode(bytes.unwrap_or_default().as_slice())
                                .map_err(|_| {
                                    HandlerError::Other(
                                        "host handler returned undecodable CommandOutput bytes"
                                            .to_string(),
                                    )
                                })?;
                        // The host's resolve_conflicts(window, nonce), as data.
                        if let Some(conflict) = cctx.conflict.as_ref() {
                            if !output.resolved_nonce.is_empty() {
                                conflict
                                    .resolve_conflicts(conflict.window(), &output.resolved_nonce);
                            }
                        }
                        Ok(output.events)
                    }
                    STATUS_OK_EMPTY => Ok(None),
                    _ => Err(host_error(ret, bytes)),
                }
            });
        }
        if let Some(changed_id) = desc.changed_callback_id {
            // Router-owned state: COMMUTATIVE captures and restores by copying
            // the state bytes; only changed() crosses to the host.
            dispatch = dispatch.with_commutative_via(
                |state: &Any| Ok(state.clone()),
                |state: &mut Any, image: &Any| {
                    *state = image.clone();
                    Ok(())
                },
                move |before: &Any, after: &Any| {
                    let call = abi_pb::ChangedCall {
                        before: Some(before.clone()),
                        after: Some(after.clone()),
                    }
                    .encode_to_vec();
                    let (ret, bytes) = invoke(cb, changed_id, "", &call, &[]);
                    if ret < 0 {
                        return Err(host_error(ret, bytes));
                    }
                    let units = abi_pb::ChangedUnits::decode(bytes.unwrap_or_default().as_slice())
                        .map_err(|_| {
                            HandlerError::Other(
                                "host changed() returned undecodable ChangedUnits bytes"
                                    .to_string(),
                            )
                        })?;
                    Ok(units.units)
                },
            );
        }
        if !desc.command_upcasters.is_empty() {
            let mut chain = UpcasterDispatch::new(desc.name.clone(), desc.domain.clone());
            for rule in &desc.command_upcasters {
                let id = rule.callback_id;
                chain = chain.on_event(&rule.fq_type, move |any| {
                    let (ret, bytes) = invoke(cb, id, &any.type_url, &any.value, &[]);
                    match ret {
                        STATUS_OK => prost_types::Any::decode(bytes.unwrap_or_default().as_slice())
                            .map_err(|_| {
                                HandlerError::Other(
                                    "host upcaster returned undecodable Any bytes".to_string(),
                                )
                            }),
                        STATUS_OK_EMPTY => Ok(any.clone()),
                        _ => Err(host_error(ret, bytes)),
                    }
                });
            }
            dispatch = dispatch.with_command_upcaster(chain);
        }

        for fact in &desc.facts {
            let id = fact.callback_id;
            dispatch = dispatch.on_fact(&fact.fq_type, move |any, state| {
                let aux = abi_pb::CommandContextAux {
                    state: Some(state.clone()),
                    ..Default::default()
                }
                .encode_to_vec();
                let (ret, bytes) = invoke(cb, id, &any.type_url, &any.value, &aux);
                match ret {
                    STATUS_OK => {
                        let record =
                            abi_pb::FactRecord::decode(bytes.unwrap_or_default().as_slice())
                                .map_err(|_| {
                                    HandlerError::Other(
                                        "host fact handler returned undecodable FactRecord bytes"
                                            .to_string(),
                                    )
                                })?;
                        // A record without a fact records the fact as received.
                        Ok(FactRecord {
                            fact: record.fact.unwrap_or_else(|| any.clone()),
                            flags: record.flags,
                        })
                    }
                    STATUS_OK_EMPTY => Ok(FactRecord::from(any.clone())),
                    _ => Err(host_error(ret, bytes)),
                }
            });
        }
        // The build refuses an old type colliding with a current one.
        self.host(RouterBuilder::new().aggregate(dispatch))
    }

    /// Parses a ProjectorDescriptor and populates a core projector table over
    /// a router-owned projection (projection_type_url): one stateless fold
    /// call per delivery, and the router builds the Projection.
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
        self.refuse_a_second()?;

        let initial = Any {
            type_url: angzarr_router::type_url(&desc.projection_type_url),
            value: Vec::new(),
        };
        let mut dispatch = ProjectorDispatch::new(desc.name.clone(), move || initial.clone())
            .with_rules_version(desc.rules_version.clone())
            .with_projection_type(&desc.projection_type_url);
        if !desc.domains.is_empty() {
            dispatch = dispatch.for_domains(desc.domains.clone());
        }
        if let Some(id) = desc.fold_callback_id {
            dispatch = dispatch.with_fold(move |projection: &mut Any, pages, cover| {
                host_fold(cb, id, projection, pages, Some(cover))
            });
        }
        // The router answers the Projection itself (types.proto, Projection):
        // the projector's position is the last projected page's sequence + 1.
        let name = desc.name.clone();
        dispatch = dispatch.finish(move |projection: &mut Any, events| {
            Ok(pb::Projection {
                cover: events.cover.clone(),
                projector: name.clone(),
                next_sequence: angzarr_router::projector::next_sequence_after(events),
                projection: Some(projection.clone()),
            })
        });

        self.host(RouterBuilder::new().projector(dispatch))
    }

    /// Decodes ContextualCommand bytes and handles them with the aggregate.
    pub fn dispatch(&self, request: &[u8]) -> Result<Vec<u8>, CodedError> {
        let req = pb::ContextualCommand::decode(request).map_err(|_| {
            CodedError::invalid_argument(
                codes::ANY_DECODE_FAILED,
                "failed to decode ContextualCommand",
                [],
            )
        })?;
        Ok(self.router()?.dispatch_command(&req)?.encode_to_vec())
    }

    /// Decodes EventBook bytes and runs the projector over them (the core
    /// applies its domain filter).
    pub fn dispatch_projector(&self, request: &[u8]) -> Result<Vec<u8>, CodedError> {
        let book = pb::EventBook::decode(request).map_err(|_| {
            CodedError::invalid_argument(codes::ANY_DECODE_FAILED, "failed to decode EventBook", [])
        })?;
        Ok(self.router()?.dispatch_projector(&book)?.encode_to_vec())
    }

    /// Parses a SagaDescriptor and populates a core saga table with
    /// callback-marshaling thunks. Event thunks pass the source cover and the
    /// event's sequence to the host, which returns a SagaResponse whose
    /// commands are taken as emitted.
    pub fn register_saga(&mut self, descriptor: &[u8], cb: AngzarrCb) -> Result<(), CodedError> {
        let desc = abi_pb::SagaDescriptor::decode(descriptor).map_err(|_| {
            CodedError::invalid_argument(
                codes::ANY_DECODE_FAILED,
                "failed to decode SagaDescriptor",
                [],
            )
        })?;
        self.refuse_a_second()?;
        let mut dispatch = SagaDispatch::new(
            desc.name.clone(),
            desc.input_domain.clone(),
            desc.target_domains.clone(),
        )
        .with_rules_version(desc.rules_version.clone());

        for event in &desc.events {
            let id = event.callback_id;
            dispatch = dispatch.on_event_with_context(&event.fq_type, move |any, source| {
                let aux = abi_pb::SagaEventAux {
                    source_cover: source.cover.cloned(),
                    source_seq: source.sequence,
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
                        Ok(resp.commands)
                    }
                    STATUS_OK_EMPTY => Ok(Vec::new()),
                    _ => Err(host_error(ret, bytes)),
                }
            });
        }

        self.host(RouterBuilder::new().saga(dispatch))
    }

    /// Decodes the source EventBook bytes and runs the saga over them.
    pub fn dispatch_saga(&self, request: &[u8]) -> Result<Vec<u8>, CodedError> {
        let source = pb::EventBook::decode(request).map_err(|_| {
            CodedError::invalid_argument(
                codes::ANY_DECODE_FAILED,
                "failed to decode the saga source EventBook",
                [],
            )
        })?;
        Ok(self.router()?.dispatch_saga(&source)?.encode_to_vec())
    }

    /// Parses a ProcessManagerDescriptor and populates a core PM table.
    /// Stateful over router-owned state (state_type_url, fold_callback_id):
    /// event thunks receive the rebuilt state in PmEventAux and return a
    /// ProcessManagerHandleResponse, taken as emitted.
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
        self.refuse_a_second()?;

        let rebuilder = state_rebuilder(&desc.state_type_url, desc.fold_callback_id, cb);

        let mut dispatch = ProcessManagerDispatch::new(
            desc.name.clone(),
            desc.pm_domain.clone(),
            desc.target_domains.clone(),
            rebuilder,
        )
        .with_rules_version(desc.rules_version.clone());

        for event in &desc.events {
            let id = event.callback_id;
            dispatch = dispatch.on_event(
                &event.input_domain,
                &event.fq_type,
                move |any, state, trigger_cover| {
                    let aux = abi_pb::PmEventAux {
                        trigger_cover: trigger_cover.cloned(),
                        state: Some(state.clone()),
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

        self.host(RouterBuilder::new().process_manager(dispatch))
    }

    /// Decodes ProcessManagerHandleRequest bytes and handles them with the
    /// process manager, over its router-owned state.
    pub fn dispatch_process_manager(&self, request: &[u8]) -> Result<Vec<u8>, CodedError> {
        let req = pb::ProcessManagerHandleRequest::decode(request).map_err(|_| {
            CodedError::invalid_argument(
                codes::ANY_DECODE_FAILED,
                "failed to decode ProcessManagerHandleRequest",
                [],
            )
        })?;
        Ok(self
            .router()?
            .dispatch_process_manager(&req)?
            .encode_to_vec())
    }
}
