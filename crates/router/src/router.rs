//! Router: the one component of one business process (owner ruling: one
//! coordinator, one component, one business process). A component is one
//! aggregate, saga, process manager or projector; its upcaster chain is part
//! of it, not another component.
//!
//! [`RouterBuilder`] takes the component (and, optionally, its upcaster
//! chain) and validates the configuration at build time: no component is
//! NO_HANDLERS_REGISTERED (C-0060); a second component, of any kind, or a
//! second upcaster chain is DUPLICATE_REGISTRATION; an aggregate whose
//! chain accepts one of its own current types is DUPLICATE_COMMAND_HANDLER.
//! The built [`Router`] dispatches to that component:
//!
//! - a command or a fact (a page's Fact arm) to the aggregate;
//! - a saga source to the saga, when it comes from the saga's input domain;
//! - a process-manager request to the process manager;
//! - an EventBook to the projector, one Projection;
//! - an UpcastRequest for the chain's domain through the component's chain,
//!   each rule's output feeding the next (C-0136); other requests pass
//!   through unchanged.
//!
//! The component may be registered boxed (`Box<dyn CommandHandler>` and so
//! on). Transport (gRPC servers), health and readiness stay with the host.

use crate::aggregate::AggregateDispatch;
use crate::error::{codes, extras, messages, CodedError};
use crate::pb;
use crate::process_manager::ProcessManagerDispatch;
use crate::projector::ProjectorDispatch;
use crate::saga::SagaDispatch;
use crate::upcaster::UpcasterDispatch;

/// An aggregate as the router sees it, whatever its state type.
pub trait CommandHandler: Send + Sync {
    /// The component's name.
    fn name(&self) -> &str;
    /// The fully-qualified command types it handles.
    fn command_types(&self) -> Vec<String>;
    /// The fully-qualified types its upcaster chains accept (none by
    /// default).
    fn upcast_types(&self) -> Vec<String> {
        Vec::new()
    }
    /// Checks the aggregate's own claims at build time (none to check by
    /// default): an old type its chain accepts that is also a current type
    /// is DUPLICATE_COMMAND_HANDLER.
    fn check_claims(&self) -> Result<(), CodedError> {
        Ok(())
    }
    /// `CommandHandlerService.Handle`: a command or a fact.
    fn dispatch(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError>;
    /// The registration hello (types.proto, ComponentDescriptor).
    fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError>;
}

impl<T: CommandHandler + ?Sized> CommandHandler for Box<T> {
    fn name(&self) -> &str {
        (**self).name()
    }
    fn command_types(&self) -> Vec<String> {
        (**self).command_types()
    }
    fn upcast_types(&self) -> Vec<String> {
        (**self).upcast_types()
    }
    fn check_claims(&self) -> Result<(), CodedError> {
        (**self).check_claims()
    }
    fn dispatch(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError> {
        (**self).dispatch(req)
    }
    fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError> {
        (**self).component_descriptor()
    }
}

impl<S> CommandHandler for AggregateDispatch<S> {
    fn name(&self) -> &str {
        AggregateDispatch::name(self)
    }
    fn command_types(&self) -> Vec<String> {
        AggregateDispatch::command_types(self)
    }
    fn upcast_types(&self) -> Vec<String> {
        AggregateDispatch::upcast_types(self)
    }
    fn check_claims(&self) -> Result<(), CodedError> {
        AggregateDispatch::check_claims(self)
    }
    fn dispatch(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError> {
        AggregateDispatch::dispatch(self, req)
    }
    fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError> {
        AggregateDispatch::component_descriptor(self)
    }
}

/// A process manager as the router sees it, whatever its state type.
pub trait ProcessManagerHandler: Send + Sync {
    /// The component's name.
    fn name(&self) -> &str;
    /// `ProcessManagerService.Handle`.
    fn dispatch(
        &self,
        req: &pb::ProcessManagerHandleRequest,
    ) -> Result<pb::ProcessManagerHandleResponse, CodedError>;
    /// The registration hello (types.proto, ComponentDescriptor).
    fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError>;
}

impl<T: ProcessManagerHandler + ?Sized> ProcessManagerHandler for Box<T> {
    fn name(&self) -> &str {
        (**self).name()
    }
    fn dispatch(
        &self,
        req: &pb::ProcessManagerHandleRequest,
    ) -> Result<pb::ProcessManagerHandleResponse, CodedError> {
        (**self).dispatch(req)
    }
    fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError> {
        (**self).component_descriptor()
    }
}

impl<S> ProcessManagerHandler for ProcessManagerDispatch<S> {
    fn name(&self) -> &str {
        ProcessManagerDispatch::name(self)
    }
    fn dispatch(
        &self,
        req: &pb::ProcessManagerHandleRequest,
    ) -> Result<pb::ProcessManagerHandleResponse, CodedError> {
        ProcessManagerDispatch::dispatch(self, req)
    }
    fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError> {
        ProcessManagerDispatch::component_descriptor(self)
    }
}

/// A projector as the router sees it, whatever its projection type.
pub trait ProjectorHandler: Send + Sync {
    /// The component's name.
    fn name(&self) -> &str;
    /// `ProjectorService.Handle`.
    fn dispatch(&self, events: &pb::EventBook) -> Result<pb::Projection, CodedError>;
    /// The registration hello (types.proto, ComponentDescriptor).
    fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError>;
}

impl<T: ProjectorHandler + ?Sized> ProjectorHandler for Box<T> {
    fn name(&self) -> &str {
        (**self).name()
    }
    fn dispatch(&self, events: &pb::EventBook) -> Result<pb::Projection, CodedError> {
        (**self).dispatch(events)
    }
    fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError> {
        (**self).component_descriptor()
    }
}

impl<P> ProjectorHandler for ProjectorDispatch<P> {
    fn name(&self) -> &str {
        ProjectorDispatch::name(self)
    }
    fn dispatch(&self, events: &pb::EventBook) -> Result<pb::Projection, CodedError> {
        ProjectorDispatch::dispatch(self, events)
    }
    fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError> {
        ProjectorDispatch::component_descriptor(self)
    }
}

/// The one component a router hosts.
enum Component {
    Aggregate(Box<dyn CommandHandler>),
    Saga(SagaDispatch),
    ProcessManager(Box<dyn ProcessManagerHandler>),
    Projector(Box<dyn ProjectorHandler>),
}

impl Component {
    fn name(&self) -> &str {
        match self {
            Component::Aggregate(c) => c.name(),
            Component::Saga(c) => c.name(),
            Component::ProcessManager(c) => c.name(),
            Component::Projector(c) => c.name(),
        }
    }

    fn kind(&self) -> pb::ComponentKind {
        match self {
            Component::Aggregate(_) => pb::ComponentKind::Aggregate,
            Component::Saga(_) => pb::ComponentKind::Saga,
            Component::ProcessManager(_) => pb::ComponentKind::ProcessManager,
            Component::Projector(_) => pb::ComponentKind::Projector,
        }
    }

    /// The refusal of a second component (or a second upcaster chain) beside
    /// this one: DUPLICATE_REGISTRATION naming this component and its kind
    /// (C-0580).
    fn refuse_another(&self, message: &str) -> CodedError {
        CodedError::invalid_argument(
            codes::DUPLICATE_REGISTRATION,
            message,
            [
                (extras::COMPONENT.to_string(), self.name().to_string()),
                (
                    extras::COMPONENT_KIND.to_string(),
                    self.kind()
                        .as_str_name()
                        .trim_start_matches("COMPONENT_KIND_")
                        .to_string(),
                ),
            ],
        )
    }
}

/// Collects the one component of a [`Router`] and its upcaster chain;
/// [`RouterBuilder::build`] validates the configuration.
#[derive(Default)]
pub struct RouterBuilder {
    components: Vec<Component>,
    upcasters: Vec<UpcasterDispatch>,
}

impl RouterBuilder {
    /// An empty configuration.
    pub fn new() -> Self {
        RouterBuilder::default()
    }

    /// Hosts an aggregate (command handler).
    pub fn aggregate(mut self, aggregate: impl CommandHandler + 'static) -> Self {
        self.components
            .push(Component::Aggregate(Box::new(aggregate)));
        self
    }

    /// Hosts a saga.
    pub fn saga(mut self, saga: SagaDispatch) -> Self {
        self.components.push(Component::Saga(saga));
        self
    }

    /// Hosts a process manager.
    pub fn process_manager(mut self, pm: impl ProcessManagerHandler + 'static) -> Self {
        self.components
            .push(Component::ProcessManager(Box::new(pm)));
        self
    }

    /// Hosts a projector.
    pub fn projector(mut self, projector: impl ProjectorHandler + 'static) -> Self {
        self.components
            .push(Component::Projector(Box::new(projector)));
        self
    }

    /// Gives the component its upcaster chain (upcaster.proto), which serves
    /// [`Router::upcast`].
    pub fn upcaster(mut self, upcaster: UpcasterDispatch) -> Self {
        self.upcasters.push(upcaster);
        self
    }

    /// Validates the configuration and builds the router.
    pub fn build(mut self) -> Result<Router, CodedError> {
        if self.components.is_empty() {
            return Err(CodedError::invalid_argument(
                codes::NO_HANDLERS_REGISTERED,
                messages::NO_HANDLERS_REGISTERED,
                [],
            ));
        }
        let component = self.components.remove(0);
        if !self.components.is_empty() {
            return Err(component.refuse_another("a router hosts one component"));
        }
        if self.upcasters.len() > 1 {
            return Err(component.refuse_another("a component has one upcaster chain"));
        }
        if let Component::Aggregate(aggregate) = &component {
            aggregate.check_claims()?;
        }
        Ok(Router {
            component,
            upcaster: self.upcasters.pop(),
        })
    }
}

/// The one component of one business process, ready to dispatch. Dispatching
/// a kind the router does not host is NO_HANDLER_REGISTERED.
pub struct Router {
    component: Component,
    upcaster: Option<UpcasterDispatch>,
}

fn wrong_kind() -> CodedError {
    CodedError::invalid_argument(
        codes::NO_HANDLER_REGISTERED,
        messages::NO_COMPONENT_OF_KIND,
        [],
    )
}

impl Router {
    /// The refusal of another component registered beside this one
    /// (DUPLICATE_REGISTRATION naming this one; C-0580), for hosts that
    /// register components one at a time (the C ABI).
    pub fn refuse_another_component(&self) -> CodedError {
        self.component
            .refuse_another("this business process already hosts its one component")
    }

    /// The component's registration hello (types.proto, ComponentDescriptor),
    /// the one `ComponentService.Describe` answers: the same the C ABI's
    /// `angzarr_router_component_descriptor` answers.
    pub fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError> {
        match &self.component {
            Component::Aggregate(aggregate) => aggregate.component_descriptor(),
            Component::Saga(saga) => saga.component_descriptor(),
            Component::ProcessManager(pm) => pm.component_descriptor(),
            Component::Projector(projector) => projector.component_descriptor(),
        }
    }

    /// Handles a command or a fact with the aggregate.
    pub fn dispatch_command(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError> {
        match &self.component {
            Component::Aggregate(aggregate) => aggregate.dispatch(req),
            _ => Err(wrong_kind()),
        }
    }

    /// Runs the saga over a source from its input domain; a source with
    /// pages from any other domain is NO_HANDLER_REGISTERED (a source with
    /// none is the saga's EMPTY_SAGA_SOURCE).
    pub fn dispatch_saga(&self, source: &pb::EventBook) -> Result<pb::SagaResponse, CodedError> {
        let Component::Saga(saga) = &self.component else {
            return Err(wrong_kind());
        };
        let domain = source.cover.as_ref().map_or("", |c| c.domain.as_str());
        if !source.pages.is_empty() && saga.input_domain() != domain {
            return Err(CodedError::invalid_argument(
                codes::NO_HANDLER_REGISTERED,
                "the saga does not consume the source domain",
                [(extras::DOMAIN.to_string(), domain.to_string())],
            ));
        }
        saga.dispatch(source)
    }

    /// Handles a trigger with the process manager.
    pub fn dispatch_process_manager(
        &self,
        req: &pb::ProcessManagerHandleRequest,
    ) -> Result<pb::ProcessManagerHandleResponse, CodedError> {
        match &self.component {
            Component::ProcessManager(pm) => pm.dispatch(req),
            _ => Err(wrong_kind()),
        }
    }

    /// Runs the projector over the book: one Projection.
    pub fn dispatch_projector(&self, events: &pb::EventBook) -> Result<pb::Projection, CodedError> {
        match &self.component {
            Component::Projector(projector) => projector.dispatch(events),
            _ => Err(wrong_kind()),
        }
    }

    /// Runs every page through the component's upcaster chain when the
    /// request is for the chain's domain, each rule's output feeding the next
    /// (C-0136); otherwise, or with no chain, the pages pass through
    /// unchanged.
    pub fn upcast(&self, req: &pb::UpcastRequest) -> Result<pb::UpcastResponse, CodedError> {
        let events = match self.upcaster.as_ref() {
            Some(upcaster) if upcaster.domain() == req.domain => req
                .events
                .iter()
                .map(|page| upcaster.upcast_page(page))
                .collect::<Result<_, _>>()?,
            _ => req.events.clone(),
        };
        Ok(pb::UpcastResponse { events })
    }
}

#[cfg(test)]
#[path = "router.test.rs"]
mod router_tests;
