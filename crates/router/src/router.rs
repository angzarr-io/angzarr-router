//! Router: the one component of one business process (owner ruling: one
//! coordinator, one component, one business process). A component is one
//! aggregate, policy, process manager or projector; its upcaster chain is part
//! of it, not another component.
//!
//! [`RouterBuilder`] takes the component (and, optionally, its upcaster
//! chain) and validates the configuration at build time: no component is
//! NO_HANDLERS_REGISTERED (C-0060); a second component, of any kind, or a
//! second upcaster chain is DUPLICATE_REGISTRATION; an aggregate whose
//! chain accepts one of its own current types is DUPLICATE_COMMAND_HANDLER.
//! The built [`Router`] dispatches to that component:
//!
//! - a command or a fact (a page's Fact arm) to the domain: an aggregate, or
//!   a process manager, which is a domain (decision 14.31) served the same
//!   way, after [`Router::admit`] has applied the rules a domain's input
//!   meets before its handler runs;
//! - a policy source to the policy, when it comes from the policy's input domain,
//!   and a refusal of one of its commands to the policy's OnRefused;
//! - an EventBook to the projector, one Projection;
//! - an UpcastRequest for the chain's domain through the component's chain,
//!   each rule's output feeding the next (C-0136); other requests pass
//!   through unchanged.
//!
//! The component may be registered boxed (`Box<dyn CommandHandler>` and so
//! on). Transport (gRPC servers), health and readiness stay with the host.

use crate::domain::{Admission, Arrival, DomainDispatch};
use crate::error::{codes, extras, CodedError};
use crate::pb;
use crate::policy::PolicyDispatch;
use crate::projector::ProjectorDispatch;
use crate::upcaster::UpcasterDispatch;

/// A domain (an aggregate or a process manager; decision 14.31) as the
/// router sees it, whatever its state type.
pub trait CommandHandler: Send + Sync {
    /// The component's name.
    fn name(&self) -> &str;
    /// AGGREGATE or PROCESS_MANAGER.
    fn kind(&self) -> pb::ComponentKind {
        pb::ComponentKind::Aggregate
    }
    /// The rules an input meets before its handler runs (deduplication,
    /// editions, a process manager's start rule): [`DomainDispatch::admit`].
    /// A hand-written domain gets the framework's rules for a domain with
    /// no start rule (validation, deduplication, editions); it never
    /// reimplements them.
    fn admit(&self, arrival: &Arrival<'_>) -> Result<Admission, CodedError> {
        crate::domain::admit_rules(arrival, None)
    }
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
    fn kind(&self) -> pb::ComponentKind {
        (**self).kind()
    }
    fn admit(&self, arrival: &Arrival<'_>) -> Result<Admission, CodedError> {
        (**self).admit(arrival)
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

impl<S> CommandHandler for DomainDispatch<S> {
    fn name(&self) -> &str {
        DomainDispatch::name(self)
    }
    fn kind(&self) -> pb::ComponentKind {
        DomainDispatch::kind(self)
    }
    fn admit(&self, arrival: &Arrival<'_>) -> Result<Admission, CodedError> {
        DomainDispatch::admit(self, arrival)
    }
    fn command_types(&self) -> Vec<String> {
        DomainDispatch::command_types(self)
    }
    fn upcast_types(&self) -> Vec<String> {
        DomainDispatch::upcast_types(self)
    }
    fn check_claims(&self) -> Result<(), CodedError> {
        DomainDispatch::check_claims(self)
    }
    fn dispatch(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError> {
        DomainDispatch::dispatch(self, req)
    }
    fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError> {
        DomainDispatch::component_descriptor(self)
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
    Policy(PolicyDispatch),
    Projector(Box<dyn ProjectorHandler>),
}

impl Component {
    fn name(&self) -> &str {
        match self {
            Component::Aggregate(c) => c.name(),
            Component::Policy(c) => c.name(),
            Component::Projector(c) => c.name(),
        }
    }

    fn kind(&self) -> pb::ComponentKind {
        match self {
            Component::Aggregate(c) => c.kind(),
            Component::Policy(_) => pb::ComponentKind::Policy,
            Component::Projector(_) => pb::ComponentKind::Projector,
        }
    }

    /// The refusal of a second component (or a second upcaster chain) beside
    /// this one: DUPLICATE_REGISTRATION naming this component and its kind
    /// (C-0580).
    ///
    /// RED (14.37): one key for two conditions; DUPLICATE_COMPONENT and
    /// DUPLICATE_UPCASTER_CHAIN tell them apart.
    fn refuse_another(&self) -> CodedError {
        CodedError::invalid_argument(
            codes::DUPLICATE_REGISTRATION,
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

    /// Hosts a policy.
    pub fn policy(mut self, policy: PolicyDispatch) -> Self {
        self.components.push(Component::Policy(policy));
        self
    }

    /// Hosts a process manager: a domain (decision 14.31), hosted as an
    /// aggregate is ([`DomainDispatch::process_manager`]).
    pub fn process_manager(self, pm: impl CommandHandler + 'static) -> Self {
        self.aggregate(pm)
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
                [],
            ));
        }
        let component = self.components.remove(0);
        if !self.components.is_empty() {
            return Err(component.refuse_another());
        }
        if self.upcasters.len() > 1 {
            return Err(component.refuse_another());
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
    CodedError::unimplemented(codes::NO_HANDLER_REGISTERED, [])
}

impl Router {
    /// The refusal of another component registered beside this one
    /// (DUPLICATE_REGISTRATION naming this one; C-0580), for hosts that
    /// register components one at a time (the C ABI).
    pub fn refuse_another_component(&self) -> CodedError {
        self.component.refuse_another()
    }

    /// The component's registration hello (types.proto, ComponentDescriptor),
    /// the one `ComponentService.Describe` answers: the same the C ABI's
    /// `angzarr_router_component_descriptor` answers.
    pub fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError> {
        match &self.component {
            Component::Aggregate(aggregate) => aggregate.component_descriptor(),
            Component::Policy(policy) => policy.component_descriptor(),
            Component::Projector(projector) => projector.component_descriptor(),
        }
    }

    /// The rules the domain's input meets before its handler runs
    /// ([`DomainDispatch::admit`]): the runtime calls it first, acts on
    /// the [`Admission`] (appending a fork, answering a duplicate or a
    /// skip), and only then reads the stream and calls
    /// [`Self::dispatch_command`].
    pub fn admit(&self, arrival: &Arrival<'_>) -> Result<Admission, CodedError> {
        match &self.component {
            Component::Aggregate(domain) => domain.admit(arrival),
            _ => Err(wrong_kind()),
        }
    }

    /// The policy's OnRefused (policy.proto): the refusal fact for its own
    /// source domain, passed through or translated, the callback handed the
    /// refused command's context `ctx`
    /// ([`PolicyDispatch::dispatch_refusal`]).
    pub fn dispatch_refusal(
        &self,
        refusal: &pb::Refusal,
        ctx: &crate::context::CommandContext,
    ) -> Result<pb::OnRefusedResponse, CodedError> {
        match &self.component {
            Component::Policy(policy) => policy.dispatch_refusal(refusal, ctx),
            _ => Err(wrong_kind()),
        }
    }

    /// Handles a command or a fact with the domain (an aggregate or a
    /// process manager).
    pub fn dispatch_command(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError> {
        match &self.component {
            Component::Aggregate(aggregate) => aggregate.dispatch(req),
            _ => Err(wrong_kind()),
        }
    }

    /// Runs the policy over a source from its input domain; a source with
    /// pages from any other domain is NO_HANDLER_REGISTERED (a source with
    /// none is the policy's EMPTY_POLICY_SOURCE).
    pub fn dispatch_policy(
        &self,
        source: &pb::EventBook,
    ) -> Result<pb::PolicyResponse, CodedError> {
        let Component::Policy(policy) = &self.component else {
            return Err(wrong_kind());
        };
        let domain = source.cover.as_ref().map_or("", |c| c.domain.as_str());
        if !source.pages.is_empty() && policy.input_domain() != domain {
            return Err(CodedError::invalid_argument(
                codes::NO_HANDLER_REGISTERED,
                [(extras::DOMAIN.to_string(), domain.to_string())],
            ));
        }
        policy.dispatch(source)
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
