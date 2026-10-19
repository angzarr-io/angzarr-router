//! Router — a validated composition of components of ONE kind.
//!
//! [`RouterBuilder`] collects components and validates the configuration at
//! build time: an empty configuration is NO_HANDLERS_REGISTERED (C-0060),
//! mixing kinds is MIXED_HANDLER_KINDS (C-0063), two command handlers
//! claiming one (domain, command type) is DUPLICATE_COMMAND_HANDLER (C-0064).
//! The built [`Router`] dispatches:
//!
//! - commands by (cover domain, command type), facts (a page's Fact arm) by
//!   (cover domain, fact type);
//! - saga sources to every saga consuming the source domain, merged in
//!   registration order (C-0013);
//! - process-manager requests through [`select_process_managers`], merged;
//! - an EventBook to every projector, one Projection each (C-0015);
//! - an UpcastRequest through every upcaster of its domain, in registration
//!   order, each one's output feeding the next (C-0136).
//!
//! Components may be registered boxed (`Box<dyn CommandHandler>` and so on).
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
    /// The fully-qualified fact types it records (none by default).
    fn fact_types(&self) -> Vec<String> {
        Vec::new()
    }
    /// The fully-qualified types its upcaster chains accept (none by
    /// default), claimed for it at registration like its command types.
    fn upcast_types(&self) -> Vec<String> {
        Vec::new()
    }
    /// `CommandHandlerService.Handle`: a command or a fact.
    fn dispatch(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError>;
}

impl<T: CommandHandler + ?Sized> CommandHandler for Box<T> {
    fn domain(&self) -> &str {
        (**self).domain()
    }
    fn command_types(&self) -> Vec<String> {
        (**self).command_types()
    }
    fn fact_types(&self) -> Vec<String> {
        (**self).fact_types()
    }
    fn upcast_types(&self) -> Vec<String> {
        (**self).upcast_types()
    }
    fn dispatch(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError> {
        (**self).dispatch(req)
    }
}

impl<S> CommandHandler for AggregateDispatch<S> {
    fn domain(&self) -> &str {
        AggregateDispatch::domain(self)
    }
    fn command_types(&self) -> Vec<String> {
        AggregateDispatch::command_types(self)
    }
    fn fact_types(&self) -> Vec<String> {
        AggregateDispatch::fact_types(self)
    }
    fn upcast_types(&self) -> Vec<String> {
        AggregateDispatch::upcast_types(self)
    }
    fn dispatch(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError> {
        AggregateDispatch::dispatch(self, req)
    }
}

/// A process manager as the router sees it, whatever its state type.
pub trait ProcessManagerHandler: ProcessManagerRoute + Send + Sync {
    /// The declared rules version (empty by default), reported on every
    /// response.
    fn rules_version(&self) -> &str {
        ""
    }
    /// `ProcessManagerService.Handle`.
    fn dispatch(
        &self,
        req: &pb::ProcessManagerHandleRequest,
    ) -> Result<pb::ProcessManagerHandleResponse, CodedError>;
}

impl<T: ProcessManagerRoute + ?Sized> ProcessManagerRoute for Box<T> {
    fn name(&self) -> &str {
        (**self).name()
    }
    fn pm_domain(&self) -> &str {
        (**self).pm_domain()
    }
    fn consumes(&self, domain: &str) -> bool {
        (**self).consumes(domain)
    }
}

impl<T: ProcessManagerHandler + ?Sized> ProcessManagerHandler for Box<T> {
    fn rules_version(&self) -> &str {
        (**self).rules_version()
    }
    fn dispatch(
        &self,
        req: &pb::ProcessManagerHandleRequest,
    ) -> Result<pb::ProcessManagerHandleResponse, CodedError> {
        (**self).dispatch(req)
    }
}

impl<S> ProcessManagerHandler for ProcessManagerDispatch<S> {
    fn rules_version(&self) -> &str {
        ProcessManagerDispatch::rules_version(self)
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

impl<T: ProjectorHandler + ?Sized> ProjectorHandler for Box<T> {
    fn dispatch(&self, events: &pb::EventBook) -> Result<pb::Projection, CodedError> {
        (**self).dispatch(events)
    }
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
            let mut handlers = CommandHandlers::new();
            for aggregate in aggregates {
                handlers.push(aggregate)?;
            }
            Inner::CommandHandlers(handlers)
        } else if !sagas.is_empty() {
            Inner::Sagas(sagas)
        } else if !pms.is_empty() {
            Inner::ProcessManagers(pms)
        } else if !projectors.is_empty() {
            Inner::Projectors(projectors)
        } else {
            Inner::Upcasters(upcasters)
        };
        Ok(Router { inner })
    }
}

/// The command handlers of one router, in registration order, with their
/// (domain, command type) claims read once, at registration (C-0065).
#[derive(Default)]
pub struct CommandHandlers {
    handlers: Vec<Box<dyn CommandHandler>>,
    /// (domain, command type) → handler index.
    claims: HashMap<(String, String), usize>,
}

impl CommandHandlers {
    /// No handlers.
    pub fn new() -> Self {
        CommandHandlers::default()
    }

    /// Adds `handler`, claiming its command types and the source types of
    /// its upcaster chains. Two claims of one (domain, type) — two handlers,
    /// or an old type that is also a current one — is
    /// DUPLICATE_COMMAND_HANDLER (C-0064); the table is left unchanged.
    pub fn push(&mut self, handler: Box<dyn CommandHandler>) -> Result<(), CodedError> {
        let index = self.handlers.len();
        let keys: Vec<(String, String)> = handler
            .command_types()
            .into_iter()
            .chain(handler.upcast_types())
            .map(|command| (handler.domain().to_string(), command))
            .collect();
        for (n, key) in keys.iter().enumerate() {
            if self.claims.contains_key(key) || keys[..n].contains(key) {
                return Err(CodedError::invalid_argument(
                    codes::DUPLICATE_COMMAND_HANDLER,
                    messages::DUPLICATE_COMMAND_HANDLER,
                    [
                        (extras::DOMAIN.to_string(), key.0.clone()),
                        (extras::COMMAND_TYPE.to_string(), key.1.clone()),
                    ],
                ));
            }
        }
        self.claims.extend(keys.into_iter().map(|key| (key, index)));
        self.handlers.push(handler);
        Ok(())
    }

    /// The only registered handler, when exactly one is.
    pub fn sole(&self) -> Option<&dyn CommandHandler> {
        match self.handlers.as_slice() {
            [only] => Some(only.as_ref()),
            _ => None,
        }
    }

    /// True when some handler serves `domain`.
    pub fn serves(&self, domain: &str) -> bool {
        self.in_domain(domain).next().is_some()
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

    /// Routes a ContextualCommand by its page's body: a Command by its
    /// (cover domain, type) claim — a command type, or a source type of the
    /// claimant's upcaster chain — and a Fact to the first handler of the
    /// cover domain recording its type or accepting it upcast. An unclaimed
    /// type is refused here, naming it (no chain ran); an envelope with no
    /// payload goes to the domain's first handler, which reports it.
    pub fn dispatch(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError> {
        let book = req.command.as_ref();
        let domain = book
            .and_then(|b| b.cover.as_ref())
            .map_or("", |c| c.domain.as_str());
        let page = book.and_then(|b| b.page.as_ref());
        if let Some(any) = page.and_then(crate::command_payload) {
            let key = (
                domain.to_string(),
                crate::type_name_from_url(&any.type_url).to_string(),
            );
            return match self.claims.get(&key) {
                Some(&index) => self.handlers[index].dispatch(req),
                None => Err(CodedError::unknown_type(
                    codes::NO_HANDLER_REGISTERED,
                    &any.type_url,
                    &any.type_url,
                    [(extras::DOMAIN.to_string(), key.0)],
                )),
            };
        }
        if let Some(any) = page.and_then(crate::fact_payload) {
            let fq = crate::type_name_from_url(&any.type_url).to_string();
            let recorder = self
                .in_domain(domain)
                .find(|h| h.fact_types().contains(&fq) || h.upcast_types().contains(&fq));
            return match recorder {
                Some(handler) => handler.dispatch(req),
                None => Err(CodedError::unknown_type(
                    codes::NO_FACT_HANDLER,
                    &any.type_url,
                    &any.type_url,
                    [(extras::DOMAIN.to_string(), domain.to_string())],
                )),
            };
        }
        match self.in_domain(domain).next() {
            Some(handler) => handler.dispatch(req),
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
    /// Routes a command or a fact to its aggregate.
    pub fn dispatch_command(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError> {
        match &self.inner {
            Inner::CommandHandlers(handlers) => handlers.dispatch(req),
            _ => Err(wrong_kind()),
        }
    }

    /// Runs every saga consuming the source domain and merges their output in
    /// registration order; a domain no saga consumes is NO_HANDLER_REGISTERED.
    pub fn dispatch_saga(&self, source: &pb::EventBook) -> Result<pb::SagaResponse, CodedError> {
        let Inner::Sagas(sagas) = &self.inner else {
            return Err(wrong_kind());
        };
        let domain = source.cover.as_ref().map_or("", |c| c.domain.as_str());
        let mut merged = pb::SagaResponse::default();
        let mut matched = false;
        for saga in sagas.iter().filter(|s| s.input_domain() == domain) {
            matched = true;
            merged.commands.extend(saga.dispatch(source)?.commands);
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
        // Co-hosted PMs share one binary, so one rules version: reported even
        // when no PM consumed the trigger.
        let mut merged = pb::ProcessManagerHandleResponse {
            rules_version: pms
                .first()
                .map(|pm| pm.rules_version().to_string())
                .unwrap_or_default(),
            ..Default::default()
        };
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
