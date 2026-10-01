//! Router — a validated composition of components of ONE kind.
//!
//! [`RouterBuilder`] collects components and validates the configuration at
//! build time: an empty configuration is NO_HANDLERS_REGISTERED (C-0060),
//! mixing kinds is MIXED_HANDLER_KINDS (C-0063), two command handlers
//! claiming one (domain, command type) is DUPLICATE_COMMAND_HANDLER (C-0064),
//! and each table's own validation (ambiguous `compensates` entries) runs once.
//! The built [`Router`] dispatches:
//!
//! - commands by (cover domain, command type); a Notification to the
//!   aggregate in its domain that declares a handler for it (else the first
//!   aggregate of the domain, which answers DelegateToFramework or
//!   NO_UNDO_HANDLER); facts by the facts' cover domain;
//! - saga sources to every saga consuming the source domain, merged in
//!   registration order (C-0013);
//! - process-manager requests through [`select_process_managers`], merged;
//! - an EventBook to every projector, one Projection each (C-0015);
//! - an UpcastRequest through every upcaster of its domain, in registration
//!   order, each one's output feeding the next (C-0136).
//!
//! Transport (gRPC servers), health and readiness stay with the host.

use std::collections::HashMap;

use crate::aggregate::AggregateDispatch;
use crate::error::{codes, extras, messages, CodedError};
use crate::pb;
use crate::process_manager::{
    merge_response, select_process_managers, ProcessManagerDispatch, ProcessManagerRoute,
};
use crate::projector::ProjectorDispatch;
use crate::saga::SagaDispatch;
use crate::upcaster::UpcasterDispatch;

/// An aggregate as the router sees it, whatever its state type.
pub trait CommandHandler: Send + Sync {
    /// The aggregate's domain.
    fn domain(&self) -> &str;
    /// The fully-qualified command types it handles.
    fn command_types(&self) -> Vec<String>;
    /// True when it declares a handler for the Notification.
    fn claims_notification(&self, notification_any: &prost_types::Any) -> bool;
    /// Refuses an invalid table (e.g. ambiguous `compensates` entries).
    fn validate(&self) -> Result<(), CodedError>;
    /// `CommandHandlerService.Handle`.
    fn dispatch(&self, req: &pb::ContextualCommand) -> Result<pb::BusinessResponse, CodedError>;
    /// `CommandHandlerService.HandleFact`.
    fn handle_fact(&self, req: &pb::FactRequest) -> Result<pb::EventBook, CodedError>;
}

impl<S> CommandHandler for AggregateDispatch<S> {
    fn domain(&self) -> &str {
        AggregateDispatch::domain(self)
    }
    fn command_types(&self) -> Vec<String> {
        AggregateDispatch::command_types(self)
    }
    fn claims_notification(&self, notification_any: &prost_types::Any) -> bool {
        AggregateDispatch::claims_notification(self, notification_any)
    }
    fn validate(&self) -> Result<(), CodedError> {
        AggregateDispatch::validate(self)
    }
    fn dispatch(&self, req: &pb::ContextualCommand) -> Result<pb::BusinessResponse, CodedError> {
        AggregateDispatch::dispatch(self, req)
    }
    fn handle_fact(&self, req: &pb::FactRequest) -> Result<pb::EventBook, CodedError> {
        AggregateDispatch::handle_fact(self, req)
    }
}

/// A process manager as the router sees it, whatever its state type.
pub trait ProcessManagerHandler: ProcessManagerRoute + Send + Sync {
    /// Refuses an invalid table (e.g. ambiguous `compensates` entries).
    fn validate(&self) -> Result<(), CodedError>;
    /// `ProcessManagerService.Handle`.
    fn dispatch(
        &self,
        req: &pb::ProcessManagerHandleRequest,
    ) -> Result<pb::ProcessManagerHandleResponse, CodedError>;
}

impl<S> ProcessManagerHandler for ProcessManagerDispatch<S> {
    fn validate(&self) -> Result<(), CodedError> {
        ProcessManagerDispatch::validate(self)
    }
    fn dispatch(
        &self,
        req: &pb::ProcessManagerHandleRequest,
    ) -> Result<pb::ProcessManagerHandleResponse, CodedError> {
        ProcessManagerDispatch::dispatch(self, req)
    }
}

/// A projector as the router sees it, whatever its projection type.
pub trait ProjectorHandler: Send + Sync {
    /// `ProjectorService.Handle`.
    fn dispatch(&self, events: &pb::EventBook) -> Result<pb::Projection, CodedError>;
}

impl<P> ProjectorHandler for ProjectorDispatch<P> {
    fn dispatch(&self, events: &pb::EventBook) -> Result<pb::Projection, CodedError> {
        ProjectorDispatch::dispatch(self, events)
    }
}

enum Component {
    Aggregate(Box<dyn CommandHandler>),
    Saga(SagaDispatch),
    ProcessManager(Box<dyn ProcessManagerHandler>),
    Projector(Box<dyn ProjectorHandler>),
    Upcaster(UpcasterDispatch),
}

impl Component {
    fn kind(&self) -> &'static str {
        match self {
            Component::Aggregate(_) => "aggregate",
            Component::Saga(_) => "saga",
            Component::ProcessManager(_) => "process manager",
            Component::Projector(_) => "projector",
            Component::Upcaster(_) => "upcaster",
        }
    }
}

/// Collects components for one [`Router`]; [`RouterBuilder::build`]
/// validates the configuration.
#[derive(Default)]
pub struct RouterBuilder {
    components: Vec<Component>,
}

impl RouterBuilder {
    /// An empty configuration.
    pub fn new() -> Self {
        RouterBuilder::default()
    }

    /// Adds an aggregate (command handler).
    pub fn aggregate(mut self, aggregate: impl CommandHandler + 'static) -> Self {
        self.components
            .push(Component::Aggregate(Box::new(aggregate)));
        self
    }

    /// Adds a saga.
    pub fn saga(mut self, saga: SagaDispatch) -> Self {
        self.components.push(Component::Saga(saga));
        self
    }

    /// Adds a process manager.
    pub fn process_manager(mut self, pm: impl ProcessManagerHandler + 'static) -> Self {
        self.components
            .push(Component::ProcessManager(Box::new(pm)));
        self
    }

    /// Adds a projector.
    pub fn projector(mut self, projector: impl ProjectorHandler + 'static) -> Self {
        self.components
            .push(Component::Projector(Box::new(projector)));
        self
    }

    /// Adds an upcaster.
    pub fn upcaster(mut self, upcaster: UpcasterDispatch) -> Self {
        self.components.push(Component::Upcaster(upcaster));
        self
    }

    /// Validates the configuration and builds the router.
    pub fn build(self) -> Result<Router, CodedError> {
        let Some(first) = self.components.first() else {
            return Err(CodedError::invalid_argument(
                codes::NO_HANDLERS_REGISTERED,
                messages::NO_HANDLERS_REGISTERED,
                [],
            ));
        };
        let kind = first.kind();
        if let Some(other) = self.components.iter().find(|c| c.kind() != kind) {
            return Err(CodedError::invalid_argument(
                codes::MIXED_HANDLER_KINDS,
                messages::MIXED_HANDLER_KINDS,
                [
                    ("first_kind".to_string(), kind.to_string()),
                    ("other_kind".to_string(), other.kind().to_string()),
                ],
            ));
        }
        let mut aggregates = Vec::new();
        let mut sagas = Vec::new();
        let mut pms = Vec::new();
        let mut projectors = Vec::new();
        let mut upcasters = Vec::new();
        for component in self.components {
            match component {
                Component::Aggregate(a) => aggregates.push(a),
                Component::Saga(s) => sagas.push(s),
                Component::ProcessManager(p) => pms.push(p),
                Component::Projector(p) => projectors.push(p),
                Component::Upcaster(u) => upcasters.push(u),
            }
        }
        let inner = if !aggregates.is_empty() {
            Inner::CommandHandlers(CommandHandlers::new(aggregates)?)
        } else if !sagas.is_empty() {
            Inner::Sagas(sagas)
        } else if !pms.is_empty() {
            for pm in &pms {
                pm.validate()?;
            }
            Inner::ProcessManagers(pms)
        } else if !projectors.is_empty() {
            Inner::Projectors(projectors)
        } else {
            Inner::Upcasters(upcasters)
        };
        Ok(Router { inner })
    }
}

struct CommandHandlers {
    handlers: Vec<Box<dyn CommandHandler>>,
    /// (domain, command type) → handler index; claims are read once, at build.
    claims: HashMap<(String, String), usize>,
}

impl CommandHandlers {
    fn new(handlers: Vec<Box<dyn CommandHandler>>) -> Result<Self, CodedError> {
        let mut claims = HashMap::new();
        for (index, handler) in handlers.iter().enumerate() {
            handler.validate()?;
            for command in handler.command_types() {
                let key = (handler.domain().to_string(), command);
                if claims.insert(key.clone(), index).is_some() {
                    return Err(CodedError::invalid_argument(
                        codes::DUPLICATE_COMMAND_HANDLER,
                        messages::DUPLICATE_COMMAND_HANDLER,
                        [
                            (extras::DOMAIN.to_string(), key.0),
                            (extras::COMMAND_TYPE.to_string(), key.1),
                        ],
                    ));
                }
            }
        }
        Ok(CommandHandlers { handlers, claims })
    }

    fn in_domain<'a>(&'a self, domain: &'a str) -> impl Iterator<Item = &'a dyn CommandHandler> {
        self.handlers
            .iter()
            .map(|h| h.as_ref())
            .filter(move |h| h.domain() == domain)
    }

    fn no_handler(domain: &str) -> CodedError {
        CodedError::invalid_argument(
            codes::NO_HANDLER_REGISTERED,
            messages::UNKNOWN_COMMAND,
            [(extras::DOMAIN.to_string(), domain.to_string())],
        )
    }

    fn dispatch(&self, req: &pb::ContextualCommand) -> Result<pb::BusinessResponse, CodedError> {
        let book = req.command.as_ref();
        let domain = book
            .and_then(|b| b.cover.as_ref())
            .map_or("", |c| c.domain.as_str());
        let payload = book
            .and_then(|b| b.pages.first())
            .and_then(crate::command_payload);
        let Some(any) = payload else {
            // Envelope errors are the aggregate's to report.
            return match self.in_domain(domain).next() {
                Some(handler) => handler.dispatch(req),
                None => Err(Self::no_handler(domain)),
            };
        };
        if crate::is_notification_type_url(&any.type_url) {
            let handler = self
                .in_domain(domain)
                .find(|h| h.claims_notification(any))
                .or_else(|| self.in_domain(domain).next())
                .ok_or_else(|| Self::no_handler(domain))?;
            return handler.dispatch(req);
        }
        let key = (
            domain.to_string(),
            crate::type_name_from_url(&any.type_url).to_string(),
        );
        match self.claims.get(&key) {
            Some(&index) => self.handlers[index].dispatch(req),
            None => Err(CodedError::invalid_argument(
                codes::NO_HANDLER_REGISTERED,
                messages::UNKNOWN_COMMAND,
                [
                    (extras::DOMAIN.to_string(), key.0),
                    (extras::TYPE_URL.to_string(), any.type_url.clone()),
                ],
            )),
        }
    }

    fn handle_fact(&self, req: &pb::FactRequest) -> Result<pb::EventBook, CodedError> {
        let domain = req
            .facts
            .as_ref()
            .and_then(|f| f.cover.as_ref())
            .map_or("", |c| c.domain.as_str());
        match self.in_domain(domain).next() {
            Some(handler) => handler.handle_fact(req),
            None => Err(Self::no_handler(domain)),
        }
    }
}

enum Inner {
    CommandHandlers(CommandHandlers),
    Sagas(Vec<SagaDispatch>),
    ProcessManagers(Vec<Box<dyn ProcessManagerHandler>>),
    Projectors(Vec<Box<dyn ProjectorHandler>>),
    Upcasters(Vec<UpcasterDispatch>),
}

/// A validated composition of components of one kind. Dispatching a kind the
/// router does not host is NO_HANDLER_REGISTERED.
pub struct Router {
    inner: Inner,
}

fn wrong_kind() -> CodedError {
    CodedError::invalid_argument(
        codes::NO_HANDLER_REGISTERED,
        messages::NO_COMPONENT_OF_KIND,
        [],
    )
}

impl Router {
    /// Routes a command (or Notification delivery) to its aggregate.
    pub fn dispatch_command(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::BusinessResponse, CodedError> {
        match &self.inner {
            Inner::CommandHandlers(handlers) => handlers.dispatch(req),
            _ => Err(wrong_kind()),
        }
    }

    /// Routes facts to the aggregate of their cover domain.
    pub fn handle_fact(&self, req: &pb::FactRequest) -> Result<pb::EventBook, CodedError> {
        match &self.inner {
            Inner::CommandHandlers(handlers) => handlers.handle_fact(req),
            _ => Err(wrong_kind()),
        }
    }

    /// Runs every saga consuming the source domain and merges their output in
    /// registration order; a domain no saga consumes is NO_HANDLER_REGISTERED.
    pub fn dispatch_saga(
        &self,
        req: &pb::SagaHandleRequest,
    ) -> Result<pb::SagaResponse, CodedError> {
        let Inner::Sagas(sagas) = &self.inner else {
            return Err(wrong_kind());
        };
        let domain = req
            .source
            .as_ref()
            .and_then(|s| s.cover.as_ref())
            .map_or("", |c| c.domain.as_str());
        let mut merged = pb::SagaResponse::default();
        let mut matched = false;
        for saga in sagas.iter().filter(|s| s.input_domain() == domain) {
            matched = true;
            let resp = saga.dispatch(req)?;
            merged.commands.extend(resp.commands);
            merged.events.extend(resp.events);
        }
        if !matched {
            return Err(CodedError::invalid_argument(
                codes::NO_HANDLER_REGISTERED,
                "no saga registered for the source domain",
                [(extras::DOMAIN.to_string(), domain.to_string())],
            ));
        }
        Ok(merged)
    }

    /// Runs the process managers [`select_process_managers`] addresses and
    /// merges their responses in registration order.
    pub fn dispatch_process_manager(
        &self,
        req: &pb::ProcessManagerHandleRequest,
    ) -> Result<pb::ProcessManagerHandleResponse, CodedError> {
        let Inner::ProcessManagers(pms) = &self.inner else {
            return Err(wrong_kind());
        };
        let routes: Vec<&dyn ProcessManagerRoute> = pms
            .iter()
            .map(|pm| pm.as_ref() as &dyn ProcessManagerRoute)
            .collect();
        let mut merged = pb::ProcessManagerHandleResponse::default();
        for index in select_process_managers(&routes, req) {
            merge_response(&mut merged, pms[index].dispatch(req)?);
        }
        Ok(merged)
    }

    /// Runs every projector over the book, one Projection each, in
    /// registration order.
    pub fn dispatch_projectors(
        &self,
        events: &pb::EventBook,
    ) -> Result<Vec<pb::Projection>, CodedError> {
        let Inner::Projectors(projectors) = &self.inner else {
            return Err(wrong_kind());
        };
        projectors.iter().map(|p| p.dispatch(events)).collect()
    }

    /// Runs every page through each upcaster of the request's domain, in
    /// registration order, each one's output feeding the next.
    pub fn upcast(&self, req: &pb::UpcastRequest) -> Result<pb::UpcastResponse, CodedError> {
        let Inner::Upcasters(upcasters) = &self.inner else {
            return Err(wrong_kind());
        };
        let mut events = req.events.clone();
        for upcaster in upcasters.iter().filter(|u| u.domain() == req.domain) {
            events = events
                .iter()
                .map(|page| upcaster.upcast_page(page))
                .collect::<Result<_, _>>()?;
        }
        Ok(pb::UpcastResponse { events })
    }
}

#[cfg(test)]
#[path = "router.test.rs"]
mod router_tests;
