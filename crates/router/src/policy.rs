//! PolicyDispatch — the stateless source-event → commands translator.
//!
//! A policy holds NO state and rebuilds nothing: every page of the source book
//! is a fresh trigger. Declared event types emit commands; an undeclared
//! event type is skipped (C-0051), not an error. There is no compensation
//! input. Emitted commands pass through as the handler returned them: the
//! coordinator stamps their identity, causation and correlation; the router
//! addresses them in the trigger's edition (a policy stays in the edition of
//! the event it handles; types.proto, Cover.edition). Every command must
//! name its root, whatever its target (POLICY_COMMAND_NO_ROOT), and may not
//! carry a correlation other than its triggering event's
//! (POLICY_COMMAND_OTHER_CORRELATION; decision 14.36).
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

use crate::error::{codes, extras, map_handler_error, messages, CodedError, HandlerError};
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
/// response with its typed refusal object) and returns a fact of the policy's
/// source domain, packed.
pub type RefusalFn = Box<dyn Fn(&pb::Refusal) -> Result<Any, HandlerError> + Send + Sync>;

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
    /// command of a type with none is passed through.
    pub fn on_refused(
        mut self,
        command: &str,
        thunk: impl Fn(&pb::Refusal) -> Result<Any, HandlerError> + Send + Sync + 'static,
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
    /// event's EventRef. Translate: the callback's fact. A failing callback
    /// is a policy bug (dead-lettered and alerted).
    pub fn dispatch_refusal(
        &self,
        refusal: &pb::Refusal,
    ) -> Result<pb::OnRefusedResponse, CodedError> {
        let missing = |what: &str| {
            CodedError::invalid_argument(
                codes::MISSING_COMMAND_BOOK,
                &format!("a refusal carries its {what}"),
                [],
            )
        };
        let command = refusal.command.as_ref().ok_or_else(|| missing("command"))?;
        let event = refusal.event.as_ref().ok_or_else(|| missing("event"))?;
        let refused_type = command
            .page
            .as_ref()
            .and_then(crate::command_payload)
            .map(|p| crate::type_name_from_url(&p.type_url).to_string())
            .unwrap_or_default();
        let payload = match self.refusals.get(&refused_type) {
            Some(translate) => translate(refusal).map_err(map_handler_error)?,
            None => {
                let trigger = pb::EventRef {
                    stream: event.cover.clone(),
                    sequence: event.pages.last().map_or(0, |p| p.sequence),
                };
                let fact = pb::CommandRefused {
                    command: Some(command.clone()),
                    refusal: refusal.response.as_ref().and_then(|r| r.refusal.clone()),
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

    /// The one domain its commands go to.
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
    /// undeclared types are skipped. Emitted commands pass through
    /// unstamped, in emission order. A source with no pages is
    /// EMPTY_POLICY_SOURCE.
    pub fn dispatch(&self, source: &pb::EventBook) -> Result<pb::PolicyResponse, CodedError> {
        if source.pages.is_empty() {
            return Err(CodedError::invalid_argument(
                codes::EMPTY_POLICY_SOURCE,
                messages::EMPTY_POLICY_SOURCE,
                [],
            ));
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
            };
            let commands = thunk(event_any, &context).map_err(map_handler_error)?;
            if let Some(stray) = commands.iter().find(|c| self.outside_destination(c)) {
                return Err(self.refuse_outside_destination(stray));
            }
            let edition = source
                .cover
                .as_ref()
                .and_then(|c| c.edition.clone())
                .filter(|e| !e.is_empty());
            let mut commands = commands;
            for command in &mut commands {
                let cover = command.cover.get_or_insert_with(Default::default);
                match cover.edition.as_deref().filter(|e| !e.is_empty()) {
                    None => cover.edition = edition.clone(),
                    Some(named) if Some(named) != edition.as_deref() => {
                        return Err(refuse_outside_edition(
                            edition.as_deref().unwrap_or_default(),
                            named,
                        ));
                    }
                    Some(_) => {}
                }
            }
            let trigger_correlation = page.meta.as_ref().and_then(|m| m.correlation_id.as_ref());
            for command in &commands {
                check_root(command)?;
                check_correlation(command, trigger_correlation)?;
            }
            resp.commands.extend(commands);
        }

        Ok(resp)
    }

    fn outside_destination(&self, command: &pb::CommandBook) -> bool {
        command.cover.as_ref().map_or("", |c| c.domain.as_str()) != self.output_domain
    }

    /// A command for any domain but the policy's one destination is a policy
    /// bug (policy.proto PolicyResponse; decision 14.24): the whole answer is
    /// refused INTERNAL, carrying no commands, naming both domains.
    fn refuse_outside_destination(&self, stray: &pb::CommandBook) -> CodedError {
        let actual = stray.cover.as_ref().map_or("", |c| c.domain.as_str());
        CodedError::internal(
            codes::POLICY_COMMAND_OUTSIDE_DESTINATION,
            messages::POLICY_COMMAND_OUTSIDE_DESTINATION,
            [
                (
                    extras::EXPECTED_DOMAIN.to_string(),
                    self.output_domain.clone(),
                ),
                (extras::ACTUAL_DOMAIN.to_string(), actual.to_string()),
            ],
        )
    }
}

/// The command's domain and type, naming it in a refusal's extras.
fn naming(command: &pb::CommandBook) -> [(String, String); 2] {
    let domain = command.cover.as_ref().map_or("", |c| c.domain.as_str());
    let command_type = command
        .page
        .as_ref()
        .and_then(crate::command_payload)
        .map_or("", |p| crate::type_name_from_url(&p.type_url));
    [
        (extras::DOMAIN.to_string(), domain.to_string()),
        (extras::COMMAND_TYPE.to_string(), command_type.to_string()),
    ]
}

/// Every command a policy sends names its root, whatever its target: an
/// aggregate's or a process manager's alike (decision 14.36, amended; there
/// is no default root). An unset root, or a UUID holding no bytes, is a
/// policy bug, refused INTERNAL.
fn check_root(command: &pb::CommandBook) -> Result<(), CodedError> {
    let named = command
        .cover
        .as_ref()
        .and_then(|c| c.root.as_ref())
        .is_some_and(|r| !r.value.is_empty());
    if named {
        return Ok(());
    }
    Err(CodedError::internal(
        codes::POLICY_COMMAND_NO_ROOT,
        messages::POLICY_COMMAND_NO_ROOT,
        naming(command),
    ))
}

/// Every command a policy sends carries its trigger's correlation unchanged
/// (decision 14.36): left unset for the coordinator to stamp, or set to
/// exactly the trigger's. Another one is a policy bug, refused INTERNAL.
fn check_correlation(
    command: &pb::CommandBook,
    trigger: Option<&pb::Uuid>,
) -> Result<(), CodedError> {
    let carried = command
        .page
        .as_ref()
        .and_then(|p| p.correlation_id.as_ref());
    match (carried, trigger) {
        (Some(carried), Some(trigger)) if carried != trigger => Err(CodedError::internal(
            codes::POLICY_COMMAND_OTHER_CORRELATION,
            messages::POLICY_COMMAND_OTHER_CORRELATION,
            naming(command),
        )),
        _ => Ok(()),
    }
}

/// A command naming an edition other than its
/// trigger's: a policy bug, refused INTERNAL.
fn refuse_outside_edition(expected: &str, actual: &str) -> CodedError {
    CodedError::internal(
        codes::POLICY_COMMAND_OUTSIDE_EDITION,
        messages::POLICY_COMMAND_OUTSIDE_EDITION,
        [
            (extras::EXPECTED_EDITION.to_string(), expected.to_string()),
            (extras::ACTUAL_EDITION.to_string(), actual.to_string()),
        ],
    )
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
    let malformed = |why: &str| {
        CodedError::internal(
            codes::MALFORMED_REFUSAL_FACT,
            messages::MALFORMED_REFUSAL_FACT,
            [(extras::REASON.to_string(), why.to_string())],
        )
    };
    let fact = answer.fact.as_ref().ok_or_else(|| malformed("no fact"))?;
    if fact.cover.is_some() || fact.handling.is_some() {
        return Err(malformed("the policy set the fact's cover or handling"));
    }
    let page = fact.page.as_ref().ok_or_else(|| malformed("no page"))?;
    if crate::fact_payload(page).is_none() {
        return Err(malformed("the page is not a Fact"));
    }
    if page.command_id.is_some()
        || page.correlation_id.is_some()
        || page.causation_id.is_some()
        || page.fact_origin.is_some()
    {
        return Err(malformed("the policy set the fact's identity"));
    }
    let refused = refusal
        .command
        .as_ref()
        .and_then(|c| c.page.as_ref())
        .ok_or_else(|| malformed("the refusal carries no refused command"))?;
    let command_id = refused
        .command_id
        .clone()
        .ok_or_else(|| malformed("the refused command carries no command_id"))?;
    let trigger = refusal
        .event
        .as_ref()
        .and_then(|e| e.cover.clone())
        .ok_or_else(|| malformed("the refusal carries no triggering stream"))?;
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
