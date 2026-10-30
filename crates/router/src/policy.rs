//! PolicyDispatch — the stateless source-event → commands translator.
//!
//! A policy holds NO state and rebuilds nothing: every page of the source book
//! is a fresh trigger. Declared event types emit commands; an undeclared
//! event type is skipped (C-0051), not an error. There is no compensation
//! input. Emitted commands pass through as the handler returned them,
//! grouped by the page that triggered them (decision 14.42): the policy's
//! coordinator checks each one's destination, edition, root and
//! correlation, stamps the trigger's edition, and stamps its identity,
//! causation and correlation.
//!
//! Refusals (policy.proto, PolicyService, Refusals; decision 14.31): when one of
//! the policy's commands is refused, [`PolicyDispatch::dispatch_refusal`] (the
//! policy's OnRefused) answers the one refusal fact for the policy's own source
//! domain: the standard `CommandRefused` (pass-through), or, when the policy
//! registers a callback for the refused command's type
//! ([`PolicyDispatch::on_refused`]), the domestic fact that callback returns
//! (translate). [`address_refusal_fact`] then addresses and identifies it,
//! as the runtime must, to the exact stream of the triggering event.

use std::collections::HashMap;

use prost::{Message, Name};
use prost_types::Any;

use crate::context::CommandContext;
use crate::error::{call_handler, codes, CodedError, HandlerError};
use crate::pb;
use crate::PageContext;

/// Translates one source event into commands. Generated thunks unmarshal
/// to the typed event and call the typed business method; the source event's
/// place arrives as a [`PageContext`] (the source book's cover and the
/// event's sequence).
pub type EventFn = Box<
    dyn for<'a> Fn(&Any, &PageContext<'a>) -> Result<Vec<pb::CommandBook>, HandlerError>
        + Send
        + Sync,
>;

/// A policy's translating callback for one refused command type: reads the
/// `Refusal` (the triggering event, the command as sent, the target's
/// response with its typed refusal object) and the refused command's
/// [`CommandContext`] (as its target's handler was handed it), and returns
/// a fact of the policy's source domain, packed.
pub type RefusalFn =
    Box<dyn Fn(&pb::Refusal, &CommandContext) -> Result<Any, HandlerError> + Send + Sync>;

/// The dispatch table for one policy component.
pub struct PolicyDispatch {
    name: String,
    input_domain: String,
    output_domain: String,
    handlers: HashMap<String, EventFn>,
    refusals: HashMap<String, RefusalFn>,
    rules_version: String,
}

impl PolicyDispatch {
    /// An empty policy table translating events of its one source domain,
    /// `input_domain`, into commands for its one destination domain,
    /// `output_domain` (options.proto ComponentOptions; decision 14.24).
    pub fn new(
        name: impl Into<String>,
        input_domain: impl Into<String>,
        output_domain: impl Into<String>,
    ) -> Self {
        PolicyDispatch {
            name: name.into(),
            input_domain: input_domain.into(),
            output_domain: output_domain.into(),
            handlers: HashMap::new(),
            refusals: HashMap::new(),
            rules_version: String::new(),
        }
    }

    /// Registers the translation thunk for a fully-qualified event type; it
    /// receives the source book's cover.
    pub fn on_event(
        self,
        full_name: &str,
        thunk: impl Fn(&Any, Option<&pb::Cover>) -> Result<Vec<pb::CommandBook>, HandlerError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.on_event_with_context(full_name, move |any, source| thunk(any, source.cover))
    }

    /// Registers the translation thunk for a fully-qualified event type; it
    /// receives the source event's [`PageContext`]: the source book's cover
    /// and the event's sequence (0 when the page carries none).
    pub fn on_event_with_context(
        mut self,
        full_name: &str,
        thunk: impl for<'a> Fn(&Any, &PageContext<'a>) -> Result<Vec<pb::CommandBook>, HandlerError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.handlers.insert(full_name.to_string(), Box::new(thunk));
        self
    }

    /// Registers the translating callback for refusals of the
    /// fully-qualified command type `command` (translate mode; policy.proto,
    /// Refusals). Its presence IS translate mode for that command; a refused
    /// command of a type with none is passed through. The callback reads the
    /// `Refusal` and the refused command's [`CommandContext`], whole.
    pub fn on_refused(
        mut self,
        command: &str,
        thunk: impl Fn(&pb::Refusal, &CommandContext) -> Result<Any, HandlerError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.refusals.insert(command.to_string(), Box::new(thunk));
        self
    }

    /// `PolicyService.OnRefused`: the refusal fact for the policy's own source
    /// domain, as a CommandBook whose page body is a Fact and whose cover
    /// and identity are left for the runtime ([`address_refusal_fact`]).
    /// Pass-through (no callback for the refused command's type): the
    /// standard `CommandRefused { command, refusal, trigger }`, `refusal`
    /// the typed refusal object still packed, `trigger` the triggering
    /// event's EventRef. Translate: the callback's fact, the callback handed
    /// `ctx`, the refused command's context as its target's handler was
    /// handed it: the runtime passes on
    /// [`command_context`](crate::context::command_context) of the
    /// `ContextualCommand` it delivered. Over the wire, policy.proto's
    /// OnRefused request is the `Refusal` alone, so a server answering it
    /// knows only what the refused command's book carries (its cover,
    /// identity and Handling): the next sequence, the history flag and the
    /// conflict stay unset there. A failing callback is a policy bug
    /// (dead-lettered and alerted).
    pub fn dispatch_refusal(
        &self,
        refusal: &pb::Refusal,
        ctx: &CommandContext,
    ) -> Result<pb::OnRefusedResponse, CodedError> {
        let missing = |code: &str| CodedError::internal(code, []);
        let command = refusal
            .command
            .as_ref()
            .ok_or_else(|| missing(codes::REFUSAL_WITHOUT_COMMAND))?;
        let event = refusal
            .event
            .as_ref()
            .ok_or_else(|| missing(codes::REFUSAL_WITHOUT_EVENT))?;
        let refused_type = command
            .page
            .as_ref()
            .and_then(crate::command_payload)
            .map(|p| crate::type_name_from_url(&p.type_url).to_string())
            .unwrap_or_default();
        let payload = match self.refusals.get(&refused_type) {
            Some(translate) => call_handler(|| translate(refusal, ctx))?,
            None => {
                let trigger = pb::EventRef {
                    stream: event.cover.clone(),
                    sequence: event.pages.last().map_or(0, |p| p.sequence),
                };
                let fact = pb::CommandRefused {
                    command: Some(command.clone()),
                    refusal: refusal.response.as_ref().and_then(|r| match &r.reason {
                        Some(pb::refusal_response::Reason::Refusal(any)) => Some(any.clone()),
                        _ => None,
                    }),
                    trigger: Some(trigger),
                };
                Any {
                    type_url: crate::type_url(&pb::CommandRefused::full_name()),
                    value: fact.encode_to_vec(),
                }
            }
        };
        Ok(pb::OnRefusedResponse {
            fact: Some(pb::CommandBook {
                page: Some(pb::CommandPage {
                    body: Some(pb::command_page::Body::Fact(pb::Fact {
                        payload: Some(payload),
                    })),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        })
    }

    /// The translation of a refusal: the payload of the translating
    /// callback for the refused command's type, handed the `Refusal` and
    /// `ctx` (a failing one is a policy bug, as in [`Self::dispatch_refusal`]);
    /// `None` when there is none for that type, the runtime then passing
    /// the refusal through (decision 14.42).
    pub fn translate_refusal(
        &self,
        refusal: &pb::Refusal,
        ctx: &CommandContext,
    ) -> Result<Option<Any>, CodedError> {
        let refused_type = refusal
            .command
            .as_ref()
            .and_then(|c| c.page.as_ref())
            .and_then(crate::command_payload)
            .map(|p| crate::type_name_from_url(&p.type_url).to_string())
            .unwrap_or_default();
        match self.refusals.get(&refused_type) {
            Some(translate) => call_handler(|| translate(refusal, ctx)).map(Some),
            None => Ok(None),
        }
    }

    /// The rules version the policy registers with: the business binary's
    /// full version, baked in at build. Empty until declared.
    pub fn with_rules_version(mut self, rules_version: impl Into<String>) -> Self {
        self.rules_version = rules_version.into();
        self
    }

    /// The registration hello (types.proto, ComponentDescriptor): kind POLICY,
    /// its input domain as its input, and no state. Refused
    /// MISSING_RULES_VERSION without a rules version.
    pub fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError> {
        crate::descriptor::component_descriptor(
            &self.name,
            pb::ComponentKind::Policy,
            [self.input_domain.clone()],
            &self.rules_version,
            None,
            Vec::new(),
        )
    }

    /// The component name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The domain whose events this policy consumes.
    pub fn input_domain(&self) -> &str {
        &self.input_domain
    }

    /// The one domain its commands go to: the descriptor's output domain,
    /// which the coordinator checks them against (decision 14.42).
    pub fn output_domain(&self) -> &str {
        &self.output_domain
    }

    /// The registered fully-qualified event type names.
    pub fn event_types(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    /// The subscription map: input domain → declared event types.
    pub fn subscriptions(&self) -> HashMap<String, Vec<String>> {
        let mut m = HashMap::new();
        m.insert(self.input_domain.clone(), self.event_types());
        m
    }

    /// Walks EVERY page of the source book: declared event types emit;
    /// undeclared types are skipped. The answer holds one
    /// `PolicyReaction` per handled page, in page order, naming its
    /// trigger's sequence, an empty one included, and none for an
    /// unhandled page (decision 14.42). Emitted commands pass through as
    /// business code returned them, unchecked and unstamped, in emission
    /// order (C-0690). A source with no pages is
    /// EMPTY_POLICY_SOURCE.
    pub fn dispatch(&self, source: &pb::EventBook) -> Result<pb::PolicyResponse, CodedError> {
        if source.pages.is_empty() {
            return Err(CodedError::internal(codes::EMPTY_POLICY_SOURCE, []));
        }

        let mut resp = pb::PolicyResponse::default();

        for page in &source.pages {
            let Some(event_any) = crate::page_event(page) else {
                continue;
            };
            let Some(thunk) = self
                .handlers
                .get(crate::type_name_from_url(&event_any.type_url))
            else {
                continue; // a policy handles only its declared types (spec C-0051)
            };
            let context = PageContext {
                cover: source.cover.as_ref(),
                sequence: page.sequence,
                meta: page.meta.as_ref(),
            };
            let commands = call_handler(|| thunk(event_any, &context))?;
            // One reaction per handled page, an empty one included
            // (decision 14.42).
            resp.reactions.push(pb::PolicyReaction {
                trigger_sequence: page.sequence,
                commands,
            });
        }

        Ok(resp)
    }
}

/// Addresses and identifies the refusal fact OnRefused answered, as the
/// policy's runtime does (types.proto, the command_identity region,
/// Validation; policy.proto, Refusals, Mechanics): its cover is the exact
/// stream of the triggering event (domain, root and edition); its
/// `command_id` is the refused command's (no prefix, no derivation); its
/// `causation_id` and `correlation_id` are the refused command's; and its
/// `fact_origin.external_id` is that command_id's canonical text. An
/// answer whose page is not a Fact, or that sets its own cover, identity or
/// Handling, is a policy bug, refused.
pub fn address_refusal_fact(
    refusal: &pb::Refusal,
    answer: &pb::OnRefusedResponse,
) -> Result<pb::CommandBook, CodedError> {
    let malformed = |code: &str| CodedError::internal(code, []);
    let fact = answer
        .fact
        .as_ref()
        .ok_or_else(|| malformed(codes::REFUSAL_FACT_MISSING))?;
    if fact.cover.is_some() || fact.handling.is_some() {
        return Err(malformed(codes::REFUSAL_FACT_ADDRESSED));
    }
    let page = fact
        .page
        .as_ref()
        .ok_or_else(|| malformed(codes::REFUSAL_FACT_NO_PAGE))?;
    if crate::fact_payload(page).is_none() {
        return Err(malformed(codes::REFUSAL_FACT_NOT_A_FACT));
    }
    if page.command_id.is_some()
        || page.correlation_id.is_some()
        || page.causation_id.is_some()
        || page.fact_origin.is_some()
    {
        return Err(malformed(codes::REFUSAL_FACT_IDENTIFIED));
    }
    let refused = refusal
        .command
        .as_ref()
        .and_then(|c| c.page.as_ref())
        .ok_or_else(|| malformed(codes::REFUSAL_WITHOUT_REFUSED_COMMAND))?;
    let command_id = refused
        .command_id
        .clone()
        .ok_or_else(|| malformed(codes::REFUSED_COMMAND_WITHOUT_ID))?;
    let trigger = refusal
        .event
        .as_ref()
        .and_then(|e| e.cover.clone())
        .ok_or_else(|| malformed(codes::REFUSAL_WITHOUT_TRIGGER_STREAM))?;
    Ok(pb::CommandBook {
        cover: Some(trigger),
        page: Some(pb::CommandPage {
            body: page.body.clone(),
            fact_origin: Some(pb::FactOrigin {
                external_id: command_id.to_string(),
                ..Default::default()
            }),
            command_id: Some(command_id),
            correlation_id: refused.correlation_id.clone(),
            causation_id: refused.causation_id.clone(),
        }),
        handling: None,
    })
}

#[cfg(test)]
#[path = "policy.test.rs"]
mod policy_tests;

#[cfg(test)]
#[path = "refusal.test.rs"]
mod refusal_tests;

#[cfg(test)]
#[path = "addressing.test.rs"]
mod addressing_tests;
