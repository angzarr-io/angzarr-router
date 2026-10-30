//! RouterBuilder / Router: one router hosts ONE component (owner ruling: one
//! coordinator, one component, one business process). Build-time validation
//! (C-0060) and dispatch to that component.

use prost_types::Any;

use crate::domain::DomainDispatch;
use crate::error::{codes, CodedError};
use crate::pb;
use crate::policy::PolicyDispatch;
use crate::projector::ProjectorDispatch;
use crate::rebuild::Rebuilder;
use crate::router::{CommandHandler, PolicyComponent, ProjectorHandler, Router, RouterBuilder};
use crate::test_support::{
    command_page, event_page, fact_page, fresh_rebuilder, LedgerState, TestState,
};
use crate::type_url;

fn any_of_type(fq: &str) -> Any {
    Any {
        type_url: type_url(fq),
        value: Vec::new(),
    }
}

/// An aggregate in `domain` handling `commands`, each emitting one event
/// page tagged (type URL) with `label`.
fn aggregate(label: &'static str, domain: &str, commands: &[&str]) -> DomainDispatch<TestState> {
    let mut agg = DomainDispatch::new(label, domain, fresh_rebuilder());
    for c in commands {
        agg = agg.on_command(c, move |_, _, _| {
            Ok(Some(pb::EventBook {
                pages: vec![event_page(any_of_type(label))],
                ..Default::default()
            }))
        });
    }
    agg
}

fn command(domain: &str, fq: &str) -> pb::ContextualCommand {
    pb::ContextualCommand {
        command: Some(pb::CommandBook {
            cover: Some(pb::Cover {
                domain: domain.to_string(),
                ..Default::default()
            }),
            page: Some(command_page(any_of_type(fq))),
            ..Default::default()
        }),
        events: None,
        ..Default::default()
    }
}

/// A Fact-arm delivery of `fq` to `domain`.
fn fact(domain: &str, fq: &str) -> pb::ContextualCommand {
    let mut req = command(domain, fq);
    req.command.as_mut().unwrap().page = Some(fact_page(any_of_type(fq)));
    req
}

fn emitted_label(resp: &pb::CommandHandlerResponse) -> String {
    resp.events
        .as_ref()
        .and_then(|book| book.pages.first())
        .and_then(|page| page.payload.as_ref())
        .map(|any| crate::type_name_from_url(&any.type_url).to_string())
        .unwrap_or_default()
}

fn build_err(builder: RouterBuilder) -> String {
    match builder.build() {
        Ok(_) => panic!("expected the build to fail"),
        Err(err) => err.code,
    }
}

#[test]
fn an_empty_configuration_fails_to_build() {
    assert_eq!(
        build_err(RouterBuilder::new()),
        codes::NO_HANDLERS_REGISTERED,
        "C-0060"
    );
}

/// A builder holding one component of `kind`.
fn with_one(builder: RouterBuilder, kind: &str) -> RouterBuilder {
    match kind {
        "aggregate" => builder.aggregate(aggregate("a", "order", &["test.Create"])),
        "policy" => builder.policy(PolicyDispatch::new("s", "order", "inventory")),
        "process manager" => builder.process_manager(
            DomainDispatch::process_manager("pm", "pm", fresh_rebuilder())
                .on_command("test.Start", |_, _, _| Ok(None)),
        ),
        _ => builder.projector(ProjectorDispatch::new("p", || ())),
    }
}

const KINDS: [&str; 4] = ["aggregate", "policy", "process manager", "projector"];

#[test]
fn a_router_hosts_one_component_and_refuses_a_second_of_any_kind() {
    for first in KINDS {
        assert!(
            with_one(RouterBuilder::new(), first).build().is_ok(),
            "one {first} builds"
        );
        for second in KINDS {
            let err = match with_one(with_one(RouterBuilder::new(), first), second).build() {
                Ok(_) => panic!("a {second} after a {first} must be refused"),
                Err(err) => err,
            };
            assert_eq!(err.code, "DUPLICATE_COMPONENT");
            // C-0580: the refusal names the component already registered.
            let (name, kind) = match first {
                "aggregate" => ("a", "AGGREGATE"),
                "policy" => ("s", "POLICY"),
                "process manager" => ("pm", "PROCESS_MANAGER"),
                _ => ("p", "PROJECTOR"),
            };
            let extra = |key: &str| err.extras.get(key).map(String::as_str);
            assert_eq!(extra(crate::error::extras::COMPONENT), Some(name));
            assert_eq!(extra(crate::error::extras::COMPONENT_KIND), Some(kind));
        }
    }
}

#[test]
fn a_router_dispatches_commands_and_facts_to_its_aggregate() {
    let router = RouterBuilder::new()
        .aggregate(
            aggregate("created", "order", &["test.Create"]).on_fact("test.Paid", |_, _| {
                Ok(crate::domain::FactRecord::from(any_of_type("paid")))
            }),
        )
        .build()
        .expect("build");
    let resp = router
        .dispatch_command(&command("order", "test.Create"))
        .expect("command");
    assert_eq!(emitted_label(&resp), "created");
    let resp = router
        .dispatch_command(&fact("order", "test.Paid"))
        .expect("fact");
    assert_eq!(emitted_label(&resp), "paid");
    let err = router
        .dispatch_command(&command("order", "test.Unknown"))
        .unwrap_err();
    assert_eq!(err.code, "UNKNOWN_COMMAND_TYPE");
    crate::test_support::assert_bug(&err);
    assert_eq!(err.grpc, crate::error::GrpcCode::Internal);
    let err = router
        .dispatch_command(&fact("order", "test.Unrecorded"))
        .unwrap_err();
    assert_eq!(err.code, codes::NO_FACT_HANDLER);
    crate::test_support::assert_bug(&err);
}

fn source(domain: &str) -> pb::EventBook {
    pb::EventBook {
        cover: Some(pb::Cover {
            domain: domain.to_string(),
            ..Default::default()
        }),
        pages: vec![event_page(any_of_type("test.Created"))],
        ..Default::default()
    }
}

#[test]
fn a_router_runs_its_policy() {
    let router = RouterBuilder::new()
        .policy(
            PolicyDispatch::new("s", "order", "inventory").on_event("test.Created", |_, _| {
                Ok(vec![pb::CommandBook {
                    cover: Some(pb::Cover {
                        domain: "inventory".to_string(),
                        root: Some(crate::test_support::uuid_of(1)),
                        ..Default::default()
                    }),
                    ..Default::default()
                }])
            }),
        )
        .build()
        .expect("build");
    assert_eq!(
        crate::test_support::commands_of(
            &router.dispatch_policy(&source("order")).expect("policy")
        )
        .len(),
        1
    );
    assert_eq!(
        router.dispatch_policy(&source("billing")).unwrap_err().code,
        "POLICY_SOURCE_OUTSIDE_INPUT_DOMAIN",
        "a source outside the policy's input domain"
    );
}

// A process manager is a domain (decision 14.31): the router hosts it as it
// hosts an aggregate, and a command to it answers its own events only.
#[test]
fn a_router_runs_its_process_manager_as_a_domain() {
    let router = RouterBuilder::new()
        .process_manager(
            DomainDispatch::process_manager("pm", "pm", Rebuilder::<LedgerState>::of_message())
                .with_rules_version("9")
                .on_command("test.Start", |_, _, _| {
                    Ok(Some(pb::EventBook {
                        pages: vec![event_page(any_of_type("test.Started"))],
                        ..Default::default()
                    }))
                })
                .starts("test.Start"),
        )
        .build()
        .expect("build");
    let resp = router
        .dispatch_command(&command("pm", "test.Start"))
        .expect("pm");
    assert_eq!(emitted_label(&resp), "test.Started");
    assert_eq!(resp.rules_version, "9");
    assert_eq!(
        router.component_descriptor().expect("hello").kind(),
        pb::ComponentKind::ProcessManager
    );
}

#[test]
fn a_router_answers_its_projectors_projection() {
    let router = RouterBuilder::new()
        .projector(ProjectorDispatch::new("p1", || ()))
        .build()
        .expect("build");
    let projection = router
        .dispatch_projector(&source("order"))
        .expect("projection");
    assert_eq!(projection.projector, "p1");
}

#[test]
fn a_router_refuses_dispatch_of_another_kind() {
    let router: Router = with_one(RouterBuilder::new(), "policy")
        .build()
        .expect("build");
    assert_eq!(
        router
            .dispatch_command(&command("order", "test.Create"))
            .unwrap_err()
            .code,
        "NO_COMPONENT_OF_KIND"
    );
    assert_eq!(
        router
            .dispatch_projector(&source("order"))
            .unwrap_err()
            .code,
        "NO_COMPONENT_OF_KIND"
    );
    let aggregate_router = with_one(RouterBuilder::new(), "aggregate")
        .build()
        .expect("build");
    assert_eq!(
        aggregate_router
            .dispatch_policy(&source("order"))
            .unwrap_err()
            .code,
        "NO_COMPONENT_OF_KIND"
    );
}

// --- boxed and hand-written components ---------------------------------------

#[test]
fn boxed_components_build_routers() {
    let boxed: Box<dyn CommandHandler> = Box::new(aggregate("boxed", "order", &["test.Create"]));
    assert_eq!(boxed.command_types(), vec!["test.Create".to_string()]);
    let router = RouterBuilder::new()
        .aggregate(boxed)
        .build()
        .expect("build");
    let resp = router
        .dispatch_command(&command("order", "test.Create"))
        .expect("dispatch");
    assert_eq!(emitted_label(&resp), "boxed");

    let projector: Box<dyn ProjectorHandler> =
        Box::new(ProjectorDispatch::new("p", || ()).finish(|_, _| {
            Ok(pb::Projection {
                projector: "boxed".to_string(),
                ..Default::default()
            })
        }));
    let router = RouterBuilder::new()
        .projector(projector)
        .build()
        .expect("build");
    let projection = router
        .dispatch_projector(&pb::EventBook {
            cover: Some(pb::Cover::default()),
            ..Default::default()
        })
        .expect("dispatch");
    assert_eq!(projection.projector, "boxed");
}

/// A hand-written aggregate.
struct Plain;

impl CommandHandler for Plain {
    fn name(&self) -> &str {
        "plain"
    }
    fn command_types(&self) -> Vec<String> {
        vec!["test.Plain".to_string()]
    }
    fn dispatch(
        &self,
        _: &pb::ContextualCommand,
    ) -> Result<pb::CommandHandlerResponse, CodedError> {
        Ok(pb::CommandHandlerResponse::default())
    }
    fn component_descriptor(&self) -> Result<pb::ComponentDescriptor, CodedError> {
        Ok(pb::ComponentDescriptor {
            name: "plain".to_string(),
            ..Default::default()
        })
    }
}

#[test]
fn a_hand_written_aggregate_builds_a_router() {
    let router = RouterBuilder::new()
        .aggregate(Plain)
        .build()
        .expect("build");
    assert_eq!(
        router
            .dispatch_command(&command("plain", "test.Plain"))
            .expect("dispatch"),
        pb::CommandHandlerResponse::default()
    );
    assert_eq!(router.component_descriptor().expect("hello").name, "plain");
}

// A hand-written domain is an aggregate unless it says otherwise: the
// refusal of a second component names it as one (C-0580).
#[test]
fn a_hand_written_domain_is_an_aggregate_by_default() {
    let router = RouterBuilder::new()
        .aggregate(Plain)
        .build()
        .expect("build");
    let err = router.refuse_another_component();
    assert_eq!(
        err.extras
            .get(crate::error::extras::COMPONENT_KIND)
            .map(String::as_str),
        Some("AGGREGATE")
    );
}

// --- the registration hello (types.proto ComponentDescriptor) ----------------

fn ledger(version: &str) -> DomainDispatch<LedgerState> {
    DomainDispatch::new("Ledger", "ledger", Rebuilder::of_message())
        .with_rules_version(version)
        .on_command("test.Ledger", |_, _, _| Ok(None))
}

// Decision 14.42: angzarr-cli generates the descriptor at build time; the
// business process answers Describe with it unchanged, and the router builds
// and checks none. One the router's own descriptor logic would have refused
// (no rules version, starting types on an aggregate, no state, an output
// domain) or rebuilt (its inputs) is answered as given, and a router given
// none answers none.
#[test]
fn describe_answers_the_descriptor_it_was_given_unchanged() {
    let given = pb::ComponentDescriptor {
        name: "Ledger".to_string(),
        kind: pb::ComponentKind::Aggregate as i32,
        inputs: vec![pb::Target {
            domain: "elsewhere".to_string(),
        }],
        rules_version: String::new(),
        state: None,
        starting_types: vec!["test.Ledger".to_string()],
        output_domain: "inventory".to_string(),
    };
    let router = RouterBuilder::new()
        .aggregate(ledger(""))
        .descriptor(given.clone())
        .build()
        .expect("build");
    assert_eq!(router.component_descriptor(), Some(&given));
    let undescribed = RouterBuilder::new()
        .aggregate(ledger("1"))
        .build()
        .expect("build");
    assert_eq!(undescribed.component_descriptor(), None);
}

#[test]
fn a_router_answers_its_components_hello() {
    let kind = |router: Router| router.component_descriptor().expect("hello").kind();
    assert_eq!(
        kind(RouterBuilder::new().aggregate(ledger("1")).build().unwrap()),
        pb::ComponentKind::Aggregate
    );
    assert_eq!(
        kind(
            RouterBuilder::new()
                .policy(PolicyDispatch::new("s", "order", "x").with_rules_version("1"))
                .build()
                .unwrap()
        ),
        pb::ComponentKind::Policy
    );
    assert_eq!(
        kind(
            RouterBuilder::new()
                .process_manager(
                    DomainDispatch::process_manager(
                        "p",
                        "pm",
                        Rebuilder::<LedgerState>::of_message()
                    )
                    .with_rules_version("1")
                )
                .build()
                .unwrap()
        ),
        pb::ComponentKind::ProcessManager
    );
    assert_eq!(
        kind(
            RouterBuilder::new()
                .projector(ProjectorDispatch::new("v", || ()).with_rules_version("1"))
                .build()
                .unwrap()
        ),
        pb::ComponentKind::Projector
    );
}

#[test]
fn a_router_refuses_its_components_hello_without_a_rules_version() {
    let err = RouterBuilder::new()
        .aggregate(ledger(""))
        .build()
        .unwrap()
        .component_descriptor()
        .expect_err("refused");
    assert_eq!(err.code, codes::MISSING_RULES_VERSION);
    crate::test_support::assert_bug(&err);
}

/// A hand-written policy (a policy in another process, say): from `order`,
/// answering one command per source and translating every refusal into a
/// fact named for the refused command's type.
struct Relay;

impl PolicyComponent for Relay {
    fn name(&self) -> &str {
        "relay"
    }
    fn input_domain(&self) -> &str {
        "order"
    }
    fn dispatch(&self, _: &pb::EventBook) -> Result<pb::PolicyResponse, CodedError> {
        Ok(pb::PolicyResponse {
            reactions: vec![pb::PolicyReaction {
                trigger_sequence: 0,
                commands: vec![pb::CommandBook::default()],
            }],
        })
    }
    fn dispatch_refusal(
        &self,
        refusal: &pb::Refusal,
        _: &crate::context::CommandContext,
    ) -> Result<Option<Any>, CodedError> {
        let refused = refusal
            .command
            .as_ref()
            .and_then(|c| c.page.as_ref())
            .and_then(crate::command_payload)
            .map_or(String::new(), |a| a.type_url.clone());
        Ok(Some(any_of_type(&format!("relayed{refused}"))))
    }
}

/// A refusal of a `test.Reserve` command, triggered by an `order` event.
fn reserve_refused() -> pb::Refusal {
    pb::Refusal {
        event: Some(source("order")),
        command: Some(pb::CommandBook {
            page: Some(command_page(any_of_type("test.Reserve"))),
            ..Default::default()
        }),
        ..Default::default()
    }
}

// A policy as a runtime holds it, whatever it is (decision 14.41): a
// hand-written one, boxed, answers through the trait.
#[test]
fn a_hand_written_policy_answers_through_policy_component() {
    let boxed: Box<dyn PolicyComponent> = Box::new(Relay);
    assert_eq!(boxed.name(), "relay");
    assert_eq!(boxed.input_domain(), "order");
    assert_eq!(
        crate::test_support::commands_of(&boxed.dispatch(&source("order")).expect("policy")).len(),
        1
    );
    assert_eq!(
        boxed
            .dispatch_refusal(
                &reserve_refused(),
                &crate::context::CommandContext::default()
            )
            .expect("answered"),
        Some(any_of_type("relayed/test.Reserve"))
    );
}

// A PolicyDispatch through the trait: its translating callback's payload
// for a refused command it translates, and no translation (None) for one
// it does not, the runtime then passing the refusal through (decision
// 14.42).
#[test]
fn a_policy_dispatch_answers_through_policy_component() {
    let dispatch = PolicyDispatch::new("s", "order", "inventory")
        .on_event("test.Created", |_, _| {
            Ok(vec![pb::CommandBook {
                cover: Some(pb::Cover {
                    domain: "inventory".to_string(),
                    root: Some(crate::test_support::uuid_of(1)),
                    ..Default::default()
                }),
                ..Default::default()
            }])
        })
        .on_refused("test.Reserve", |_, _| Ok(any_of_type("test.ReserveFailed")));
    let policy: &dyn PolicyComponent = &dispatch;
    assert_eq!(policy.name(), "s");
    assert_eq!(policy.input_domain(), "order");
    assert_eq!(
        crate::test_support::commands_of(&policy.dispatch(&source("order")).expect("policy")).len(),
        1
    );
    let ctx = crate::context::CommandContext::default();
    assert_eq!(
        policy
            .dispatch_refusal(&reserve_refused(), &ctx)
            .expect("translated"),
        Some(any_of_type("test.ReserveFailed"))
    );
    let mut other = reserve_refused();
    other.command = Some(pb::CommandBook {
        page: Some(command_page(any_of_type("test.Release"))),
        ..Default::default()
    });
    assert_eq!(
        policy.dispatch_refusal(&other, &ctx).expect("answered"),
        None,
        "no callback: no translation"
    );
}
