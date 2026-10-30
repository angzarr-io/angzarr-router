//! Router: the one component of one business process (owner ruling: one
//! coordinator, one component, one business process). A component is one
//! aggregate, policy, process manager or projector. It upcasts nothing: the
//! coordinator upcasts every payload and event through the component's
//! upcaster sidecar before the router sees it (decision 14.39).
//!
//! [`RouterBuilder`] takes the component and validates the configuration at
//! build time: no component is ROUTER_REASON_NO_HANDLERS_REGISTERED (C-0060); a second
//! component, of any kind, is ROUTER_REASON_DUPLICATE_COMPONENT.
//! The built [`Router`] dispatches to that component:
//!
//! - a command or a fact (a page's Fact arm) to the domain: an aggregate, or
//!   a process manager, which is a domain (decision 14.31) served the same
//!   way, once the coordinator has admitted it (decision 14.42);
//! - a policy source to the policy, when it comes from the policy's input domain,
//!   and a refusal of one of its commands to the policy's OnRefused;
//! - an EventBook to the projector, one Projection.
//!
//! The component may be registered boxed (`Box<dyn DynCommandHandler>` and so
//! on). Transport (gRPC servers), health and readiness stay with the host.

use std::future::Future;

use crate::domain::DomainDispatch;
use crate::error::{extras, CodedError, RouterReason};
use crate::pb;
use crate::policy::PolicyDispatch;
use crate::projector::ProjectorDispatch;
use crate::send::{BoxFuture, MaybeSend, MaybeSync};

/// A domain (an aggregate or a process manager; decision 14.31) as the
/// router sees it, whatever its state type.
///
/// Kernel-facing (owner ruling 2026-10-30): the coordinator awaits
/// [`dispatch`](Self::dispatch). [`DomainDispatch`] implements it by
/// running the business code's synchronous functions and answering at
/// once (a ready future); a hand-written domain (one in another process,
/// say) awaits its I/O. Business code is never async. Hold one boxed as
/// `Box<dyn DynCommandHandler>`, its object-safe twin.
pub trait CommandHandler: MaybeSend + MaybeSync {
    /// The component's name.
    fn name(&self) -> &str;
    /// AGGREGATE or PROCESS_MANAGER.
    fn kind(&self) -> pb::ComponentKind {
        pb::ComponentKind::Aggregate
    }
    /// The fully-qualified command types it handles.
    fn command_types(&self) -> Vec<String>;
    /// `CommandHandlerService.Handle`: a command or a fact.
    fn dispatch(
        &self,
        req: &pb::ContextualCommand,
    ) -> impl Future<Output = Result<pb::CommandHandlerResponse, CodedError>> + MaybeSend;
}

/// [`CommandHandler`], object-safe: its future boxed. Every
/// `CommandHandler` is one, and `dyn DynCommandHandler` (boxed, too) is a
/// `CommandHandler` again, so a router or a coordinator holds any domain
/// as `Box<dyn DynCommandHandler>`.
pub trait DynCommandHandler: MaybeSend + MaybeSync {
    /// [`CommandHandler::name`].
    fn name(&self) -> &str;
    /// [`CommandHandler::kind`].
    fn kind(&self) -> pb::ComponentKind;
    /// [`CommandHandler::command_types`].
    fn command_types(&self) -> Vec<String>;
    /// [`CommandHandler::dispatch`], boxed.
    fn dispatch<'a>(
        &'a self,
        req: &'a pb::ContextualCommand,
    ) -> BoxFuture<'a, Result<pb::CommandHandlerResponse, CodedError>>;
}

impl<T: CommandHandler> DynCommandHandler for T {
    fn name(&self) -> &str {
        CommandHandler::name(self)
    }
    fn kind(&self) -> pb::ComponentKind {
        CommandHandler::kind(self)
    }
    fn command_types(&self) -> Vec<String> {
        CommandHandler::command_types(self)
    }
    fn dispatch<'a>(
        &'a self,
        req: &'a pb::ContextualCommand,
    ) -> BoxFuture<'a, Result<pb::CommandHandlerResponse, CodedError>> {
        Box::pin(CommandHandler::dispatch(self, req))
    }
}

impl CommandHandler for dyn DynCommandHandler + '_ {
    fn name(&self) -> &str {
        DynCommandHandler::name(self)
    }
    fn kind(&self) -> pb::ComponentKind {
        DynCommandHandler::kind(self)
    }
    fn command_types(&self) -> Vec<String> {
        DynCommandHandler::command_types(self)
    }
    async fn dispatch(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError> {
        DynCommandHandler::dispatch(self, req).await
    }
}

impl<T: CommandHandler + ?Sized> CommandHandler for Box<T> {
    fn name(&self) -> &str {
        (**self).name()
    }
    fn kind(&self) -> pb::ComponentKind {
        (**self).kind()
    }
    fn command_types(&self) -> Vec<String> {
        (**self).command_types()
    }
    fn dispatch(
        &self,
        req: &pb::ContextualCommand,
    ) -> impl Future<Output = Result<pb::CommandHandlerResponse, CodedError>> + MaybeSend {
        (**self).dispatch(req)
    }
}

impl<S> CommandHandler for DomainDispatch<S> {
    fn name(&self) -> &str {
        DomainDispatch::name(self)
    }
    fn kind(&self) -> pb::ComponentKind {
        DomainDispatch::kind(self)
    }
    fn command_types(&self) -> Vec<String> {
        DomainDispatch::command_types(self)
    }
    /// [`DomainDispatch::dispatch`]'s answer, ready.
    fn dispatch(
        &self,
        req: &pb::ContextualCommand,
    ) -> impl Future<Output = Result<pb::CommandHandlerResponse, CodedError>> + MaybeSend {
        std::future::ready(DomainDispatch::dispatch(self, req))
    }
}

/// A policy as a runtime holds it (decisions 14.41, 14.42): a
/// [`PolicyDispatch`], or one written by hand (a policy in another process,
/// behind its `PolicyService`). It answers its commands and its refusals'
/// translations; the runtime checks its source, stamps and checks its
/// commands, builds a pass-through refusal fact when there is no
/// translation, and addresses every refusal fact. It carries no descriptor
/// (nor does any component): descriptors are generated by angzarr-cli.
///
/// Kernel-facing, as [`CommandHandler`] is: its dispatch is awaited, a
/// [`PolicyDispatch`] answering at once. Held boxed as
/// `Box<dyn DynPolicyComponent>`.
pub trait PolicyComponent: MaybeSend + MaybeSync {
    /// The component's name.
    fn name(&self) -> &str;
    /// The one domain whose events it handles, and to which its refusal
    /// facts go.
    fn input_domain(&self) -> &str;
    /// `PolicyService.Handle`: the commands for one source book.
    fn dispatch(
        &self,
        source: &pb::EventBook,
    ) -> impl Future<Output = Result<pb::PolicyResponse, CodedError>> + MaybeSend;
    /// `PolicyService.OnRefused`: the payload of the fact its translating
    /// callback for the refused command's type answers, the refused
    /// command's context `ctx` beside it; `None` when it has no callback
    /// for that type (no translation).
    fn dispatch_refusal(
        &self,
        refusal: &pb::Refusal,
        ctx: &crate::context::CommandContext,
    ) -> impl Future<Output = Result<Option<prost_types::Any>, CodedError>> + MaybeSend;
}

/// [`PolicyComponent`], object-safe: its futures boxed. Every
/// `PolicyComponent` is one, and `dyn DynPolicyComponent` is a
/// `PolicyComponent` again.
pub trait DynPolicyComponent: MaybeSend + MaybeSync {
    /// [`PolicyComponent::name`].
    fn name(&self) -> &str;
    /// [`PolicyComponent::input_domain`].
    fn input_domain(&self) -> &str;
    /// [`PolicyComponent::dispatch`], boxed.
    fn dispatch<'a>(
        &'a self,
        source: &'a pb::EventBook,
    ) -> BoxFuture<'a, Result<pb::PolicyResponse, CodedError>>;
    /// [`PolicyComponent::dispatch_refusal`], boxed.
    fn dispatch_refusal<'a>(
        &'a self,
        refusal: &'a pb::Refusal,
        ctx: &'a crate::context::CommandContext,
    ) -> BoxFuture<'a, Result<Option<prost_types::Any>, CodedError>>;
}

impl<T: PolicyComponent> DynPolicyComponent for T {
    fn name(&self) -> &str {
        PolicyComponent::name(self)
    }
    fn input_domain(&self) -> &str {
        PolicyComponent::input_domain(self)
    }
    fn dispatch<'a>(
        &'a self,
        source: &'a pb::EventBook,
    ) -> BoxFuture<'a, Result<pb::PolicyResponse, CodedError>> {
        Box::pin(PolicyComponent::dispatch(self, source))
    }
    fn dispatch_refusal<'a>(
        &'a self,
        refusal: &'a pb::Refusal,
        ctx: &'a crate::context::CommandContext,
    ) -> BoxFuture<'a, Result<Option<prost_types::Any>, CodedError>> {
        Box::pin(PolicyComponent::dispatch_refusal(self, refusal, ctx))
    }
}

impl PolicyComponent for dyn DynPolicyComponent + '_ {
    fn name(&self) -> &str {
        DynPolicyComponent::name(self)
    }
    fn input_domain(&self) -> &str {
        DynPolicyComponent::input_domain(self)
    }
    async fn dispatch(&self, source: &pb::EventBook) -> Result<pb::PolicyResponse, CodedError> {
        DynPolicyComponent::dispatch(self, source).await
    }
    async fn dispatch_refusal(
        &self,
        refusal: &pb::Refusal,
        ctx: &crate::context::CommandContext,
    ) -> Result<Option<prost_types::Any>, CodedError> {
        DynPolicyComponent::dispatch_refusal(self, refusal, ctx).await
    }
}

impl<T: PolicyComponent + ?Sized> PolicyComponent for Box<T> {
    fn name(&self) -> &str {
        (**self).name()
    }
    fn input_domain(&self) -> &str {
        (**self).input_domain()
    }
    fn dispatch(
        &self,
        source: &pb::EventBook,
    ) -> impl Future<Output = Result<pb::PolicyResponse, CodedError>> + MaybeSend {
        (**self).dispatch(source)
    }
    fn dispatch_refusal(
        &self,
        refusal: &pb::Refusal,
        ctx: &crate::context::CommandContext,
    ) -> impl Future<Output = Result<Option<prost_types::Any>, CodedError>> + MaybeSend {
        (**self).dispatch_refusal(refusal, ctx)
    }
}

impl PolicyComponent for PolicyDispatch {
    fn name(&self) -> &str {
        PolicyDispatch::name(self)
    }
    fn input_domain(&self) -> &str {
        PolicyDispatch::input_domain(self)
    }
    /// [`PolicyDispatch::dispatch`]'s answer, ready.
    fn dispatch(
        &self,
        source: &pb::EventBook,
    ) -> impl Future<Output = Result<pb::PolicyResponse, CodedError>> + MaybeSend {
        std::future::ready(PolicyDispatch::dispatch(self, source))
    }
    /// [`PolicyDispatch::translate_refusal`]'s answer, ready.
    fn dispatch_refusal(
        &self,
        refusal: &pb::Refusal,
        ctx: &crate::context::CommandContext,
    ) -> impl Future<Output = Result<Option<prost_types::Any>, CodedError>> + MaybeSend {
        std::future::ready(PolicyDispatch::translate_refusal(self, refusal, ctx))
    }
}

/// A projector as the router sees it, whatever its projection type.
///
/// Kernel-facing, as [`CommandHandler`] is: its dispatch is awaited, a
/// [`ProjectorDispatch`] answering at once. Held boxed as
/// `Box<dyn DynProjectorHandler>`.
pub trait ProjectorHandler: MaybeSend + MaybeSync {
    /// The component's name.
    fn name(&self) -> &str;
    /// `ProjectorService.Handle`.
    fn dispatch(
        &self,
        events: &pb::EventBook,
    ) -> impl Future<Output = Result<pb::Projection, CodedError>> + MaybeSend;
}

/// [`ProjectorHandler`], object-safe: its future boxed. Every
/// `ProjectorHandler` is one, and `dyn DynProjectorHandler` is a
/// `ProjectorHandler` again.
pub trait DynProjectorHandler: MaybeSend + MaybeSync {
    /// [`ProjectorHandler::name`].
    fn name(&self) -> &str;
    /// [`ProjectorHandler::dispatch`], boxed.
    fn dispatch<'a>(
        &'a self,
        events: &'a pb::EventBook,
    ) -> BoxFuture<'a, Result<pb::Projection, CodedError>>;
}

impl<T: ProjectorHandler> DynProjectorHandler for T {
    fn name(&self) -> &str {
        ProjectorHandler::name(self)
    }
    fn dispatch<'a>(
        &'a self,
        events: &'a pb::EventBook,
    ) -> BoxFuture<'a, Result<pb::Projection, CodedError>> {
        Box::pin(ProjectorHandler::dispatch(self, events))
    }
}

impl ProjectorHandler for dyn DynProjectorHandler + '_ {
    fn name(&self) -> &str {
        DynProjectorHandler::name(self)
    }
    async fn dispatch(&self, events: &pb::EventBook) -> Result<pb::Projection, CodedError> {
        DynProjectorHandler::dispatch(self, events).await
    }
}

impl<T: ProjectorHandler + ?Sized> ProjectorHandler for Box<T> {
    fn name(&self) -> &str {
        (**self).name()
    }
    fn dispatch(
        &self,
        events: &pb::EventBook,
    ) -> impl Future<Output = Result<pb::Projection, CodedError>> + MaybeSend {
        (**self).dispatch(events)
    }
}

impl<P> ProjectorHandler for ProjectorDispatch<P> {
    fn name(&self) -> &str {
        ProjectorDispatch::name(self)
    }
    /// [`ProjectorDispatch::dispatch`]'s answer, ready.
    fn dispatch(
        &self,
        events: &pb::EventBook,
    ) -> impl Future<Output = Result<pb::Projection, CodedError>> + MaybeSend {
        std::future::ready(ProjectorDispatch::dispatch(self, events))
    }
}

/// The one component a router hosts.
enum Component {
    Aggregate(Box<dyn DynCommandHandler>),
    Policy(PolicyDispatch),
    Projector(Box<dyn DynProjectorHandler>),
}

impl Component {
    fn name(&self) -> &str {
        match self {
            Component::Aggregate(c) => DynCommandHandler::name(&**c),
            Component::Policy(c) => c.name(),
            Component::Projector(c) => DynProjectorHandler::name(&**c),
        }
    }

    fn kind(&self) -> pb::ComponentKind {
        match self {
            Component::Aggregate(c) => DynCommandHandler::kind(&**c),
            Component::Policy(_) => pb::ComponentKind::Policy,
            Component::Projector(_) => pb::ComponentKind::Projector,
        }
    }

    /// The refusal of a second component (ROUTER_REASON_DUPLICATE_COMPONENT) beside this
    /// one, naming this component and its kind (C-0580, C-0670): a
    /// misconfiguration, INTERNAL.
    fn refuse_another(&self, reason: RouterReason) -> CodedError {
        CodedError::internal(
            reason,
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

/// Collects the one component of a [`Router`], and the descriptor it
/// answers Describe with; [`RouterBuilder::build`] validates the
/// configuration.
#[derive(Default)]
pub struct RouterBuilder {
    components: Vec<Component>,
    descriptor: Option<pb::ComponentDescriptor>,
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

    /// The component's descriptor (types.proto, ComponentDescriptor), as
    /// angzarr-cli generated it at build time: what the business process
    /// answers `ComponentService.Describe` with, unchanged. The coordinator
    /// validates it (decision 14.42); the router builds and checks none.
    pub fn descriptor(mut self, descriptor: pb::ComponentDescriptor) -> Self {
        self.descriptor = Some(descriptor);
        self
    }

    /// Validates the configuration and builds the router.
    pub fn build(mut self) -> Result<Router, CodedError> {
        if self.components.is_empty() {
            return Err(CodedError::internal(RouterReason::NoHandlersRegistered, []));
        }
        let component = self.components.remove(0);
        if !self.components.is_empty() {
            return Err(component.refuse_another(RouterReason::DuplicateComponent));
        }
        Ok(Router {
            component,
            descriptor: self.descriptor,
        })
    }
}

/// The one component of one business process, ready to dispatch. Dispatching
/// a kind the router does not host is ROUTER_REASON_NO_COMPONENT_OF_KIND (UNIMPLEMENTED).
pub struct Router {
    component: Component,
    descriptor: Option<pb::ComponentDescriptor>,
}

fn wrong_kind() -> CodedError {
    CodedError::unimplemented(RouterReason::NoComponentOfKind, [])
}

impl Router {
    /// The refusal of another component registered beside this one
    /// (ROUTER_REASON_DUPLICATE_COMPONENT naming this one; C-0580), for hosts that
    /// register components one at a time (the C ABI).
    pub fn refuse_another_component(&self) -> CodedError {
        self.component
            .refuse_another(RouterReason::DuplicateComponent)
    }

    /// The descriptor `ComponentService.Describe` answers: the one the
    /// router was given ([`RouterBuilder::descriptor`]), unchanged; `None`
    /// when it was given none.
    pub fn component_descriptor(&self) -> Option<&pb::ComponentDescriptor> {
        self.descriptor.as_ref()
    }

    /// The policy's OnRefused (policy.proto): the refusal fact for its own
    /// source domain, passed through or translated, the callback handed the
    /// refused command's context `ctx`
    /// ([`PolicyDispatch::dispatch_refusal`]).
    pub async fn dispatch_refusal(
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
    pub async fn dispatch_command(
        &self,
        req: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError> {
        match &self.component {
            Component::Aggregate(aggregate) => DynCommandHandler::dispatch(&**aggregate, req).await,
            _ => Err(wrong_kind()),
        }
    }

    /// Runs the policy over a source. The coordinator delivers only its
    /// input domain's events; the router guards no envelope (decision
    /// 14.42).
    pub async fn dispatch_policy(
        &self,
        source: &pb::EventBook,
    ) -> Result<pb::PolicyResponse, CodedError> {
        match &self.component {
            Component::Policy(policy) => PolicyComponent::dispatch(policy, source).await,
            _ => Err(wrong_kind()),
        }
    }

    /// Runs the projector over the book: one Projection.
    pub async fn dispatch_projector(
        &self,
        events: &pb::EventBook,
    ) -> Result<pb::Projection, CodedError> {
        match &self.component {
            Component::Projector(projector) => {
                DynProjectorHandler::dispatch(&**projector, events).await
            }
            _ => Err(wrong_kind()),
        }
    }
}

#[cfg(test)]
#[path = "router.test.rs"]
mod router_tests;
