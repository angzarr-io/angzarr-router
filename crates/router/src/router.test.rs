//! RouterBuilder / Router: one router hosts ONE component (owner ruling: one
//! coordinator, one component, one business process), whose upcaster chain is
//! part of it. Build-time validation (C-0060) and dispatch to that component.

use prost_types::Any;

use crate::domain::DomainDispatch;
use crate::error::{codes, CodedError};
use crate::pb;
use crate::policy::PolicyDispatch;
use crate::projector::ProjectorDispatch;
use crate::rebuild::Rebuilder;
use crate::router::{CommandHandler, ProjectorHandler, Router, RouterBuilder};
use crate::test_support::{
    command_page, event_page, fact_page, fresh_rebuilder, LedgerState, TestState,
};
use crate::type_url;
use crate::upcaster::UpcasterDispatch;

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
fn an_upcaster_chain_is_part_of_its_component() {
    let chain = UpcasterDispatch::new("v1-v3", "order")
        .on_event("test.V1", |_| Ok(any_of_type("test.V2")))
        .on_event("test.V2", |_| Ok(any_of_type("test.V3")));
    let router = with_one(RouterBuilder::new(), "projector")
        .upcaster(chain)
        .build()
        .expect("build");
    let upcast = |domain: &str| {
        let resp = router
            .upcast(&pb::UpcastRequest {
                domain: domain.to_string(),
                events: vec![event_page(any_of_type("test.V1"))],
            })
            .expect("upcast");
        let any = resp.events[0].payload.clone().expect("an event");
        crate::type_name_from_url(&any.type_url).to_string()
    };
    assert_eq!(upcast("order"), "test.V3", "C-0136");
    assert_eq!(
        upcast("billing"),
        "test.V1",
        "another domain passes through"
    );
}

#[test]
fn a_component_without_an_upcaster_chain_passes_events_through() {
    let router = with_one(RouterBuilder::new(), "policy")
        .build()
        .expect("build");
    let events = vec![event_page(any_of_type("test.V1"))];
    let resp = router
        .upcast(&pb::UpcastRequest {
            domain: "order".to_string(),
            events: events.clone(),
        })
        .expect("upcast");
    assert_eq!(resp.events, events);
}

#[test]
fn a_component_has_one_upcaster_chain_and_a_chain_alone_is_no_component() {
    assert_eq!(
        build_err(
            with_one(RouterBuilder::new(), "aggregate")
                .upcaster(UpcasterDispatch::new("one", "order"))
                .upcaster(UpcasterDispatch::new("two", "order"))
        ),
        "DUPLICATE_UPCASTER_CHAIN"
    );
    assert_eq!(
        build_err(RouterBuilder::new().upcaster(UpcasterDispatch::new("one", "order"))),
        codes::NO_HANDLERS_REGISTERED
    );
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
        router
            .dispatch_policy(&source("order"))
            .expect("policy")
            .commands
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
    assert!(
        Plain.upcast_types().is_empty(),
        "no upcaster chain by default"
    );
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

// The router runs the hosted domain's admission rules, and refuses
// admission for a component that is not a domain.
#[test]
fn a_router_admits_through_its_domain() {
    let mut book = command("plain", "test.Plain").command.unwrap();
    book.page.as_mut().unwrap().command_id = Some(crate::test_support::uuid_of(9));
    let recorded = vec![event_page(any_of_type("test.Plained"))];
    let arrival = crate::domain::Arrival {
        command: &book,
        recorded: &recorded,
        stream: crate::domain::StreamStatus { exists: true },
    };
    let router = RouterBuilder::new()
        .aggregate(Plain)
        .build()
        .expect("build");
    assert_eq!(
        router.admit(&arrival).expect("admitted"),
        crate::domain::Admission::Duplicate(recorded.clone())
    );
    let projector = with_one(RouterBuilder::new(), "projector")
        .build()
        .expect("build");
    assert_eq!(
        projector.admit(&arrival).expect_err("not a domain").code,
        "NO_COMPONENT_OF_KIND"
    );
}

// A hand-written domain meets the framework's rules without reimplementing
// them: a command already recorded under its id is a duplicate.
#[test]
fn a_hand_written_aggregate_meets_the_framework_rules() {
    let mut book = command("plain", "test.Plain").command.unwrap();
    book.page.as_mut().unwrap().command_id = Some(crate::test_support::uuid_of(9));
    let recorded = vec![event_page(any_of_type("test.Plained"))];
    let admission = Plain
        .admit(&crate::domain::Arrival {
            command: &book,
            recorded: &recorded,
            stream: crate::domain::StreamStatus { exists: true },
        })
        .expect("admitted");
    assert_eq!(admission, crate::domain::Admission::Duplicate(recorded));
}

// --- the aggregate's own upcaster chain ---------------------------------------

/// A chain renaming `from` to `to`, for the domain "order".
fn renaming(from: &str, to: &'static str) -> UpcasterDispatch {
    UpcasterDispatch::new("rename", "order").on_event(from, move |any| {
        Ok(Any {
            type_url: type_url(to),
            value: any.value.clone(),
        })
    })
}

// An old type its chain accepts must not also be one of the aggregate's
// current types (owner ruling): the build fails.
// A domain, boxed or not, reports the old types its chains accept.
#[test]
fn a_domain_reports_the_types_its_chain_accepts() {
    let domain = aggregate("ship", "order", &["test.Ship"])
        .with_command_upcaster(renaming("test.ShipV1", "test.Ship"));
    assert_eq!(
        CommandHandler::upcast_types(&domain),
        vec!["test.ShipV1".to_string()]
    );
    let boxed: Box<dyn CommandHandler> = Box::new(domain);
    assert_eq!(boxed.upcast_types(), vec!["test.ShipV1".to_string()]);
}

#[test]
fn an_old_type_colliding_with_a_current_one_fails_the_build() {
    let err = build_err(
        RouterBuilder::new().aggregate(
            aggregate("a", "order", &["test.Ship", "test.ShipV1"])
                .with_command_upcaster(renaming("test.ShipV1", "test.Ship")),
        ),
    );
    assert_eq!(err, codes::DUPLICATE_COMMAND_HANDLER);
    let err = RouterBuilder::new()
        .aggregate(
            aggregate("a", "order", &["test.Ship", "test.ShipV1"])
                .with_command_upcaster(renaming("test.ShipV1", "test.Ship")),
        )
        .build()
        .err()
        .expect("refused");
    crate::test_support::assert_bug(&err);
}

#[test]
fn an_old_type_that_upcasts_to_nothing_is_refused_naming_where_it_ended() {
    let router = RouterBuilder::new()
        .aggregate(
            aggregate("ship", "order", &["test.Ship"])
                .with_command_upcaster(renaming("test.ShipOld", "test.ShipLost")),
        )
        .build()
        .expect("build");
    let err = router
        .dispatch_command(&command("order", "test.ShipOld"))
        .expect_err("ShipLost reaches no handler");
    assert_eq!(err.code, "UNKNOWN_COMMAND_TYPE");
    crate::test_support::assert_bug(&err);
    assert_eq!(
        err.extras.get("upcast_type_url").map(String::as_str),
        Some(type_url("test.ShipLost").as_str()),
        "the chain ran"
    );
}

// One aggregate's chains may name a source type more than once (rules on
// one type compose in order, C-0136): its own claims never collide.
#[test]
fn an_aggregate_naming_a_source_type_twice_builds() {
    let chain = renaming("test.ShipV1", "test.ShipV1").on_event("test.ShipV1", |any| {
        Ok(Any {
            type_url: type_url("test.Ship"),
            value: any.value.clone(),
        })
    });
    let router = RouterBuilder::new()
        .aggregate(aggregate("ship", "order", &["test.Ship"]).with_command_upcaster(chain))
        .build()
        .expect("one claimant");
    let resp = router
        .dispatch_command(&command("order", "test.ShipV1"))
        .expect("both rules ran");
    assert_eq!(emitted_label(&resp), "ship");
}

// --- the registration hello (types.proto ComponentDescriptor) ----------------

fn ledger(version: &str) -> DomainDispatch<LedgerState> {
    DomainDispatch::new("Ledger", "ledger", Rebuilder::of_message())
        .with_rules_version(version)
        .on_command("test.Ledger", |_, _, _| Ok(None))
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
