//! RouterBuilder / Router: build-time validation (C-0060, C-0062..C-0064)
//! and dispatch across composed components of one kind.

use prost_types::Any;

use crate::aggregate::AggregateDispatch;
use crate::error::{codes, CodedError};
use crate::pb;
use crate::process_manager::{ProcessManagerDispatch, ProcessManagerRoute};
use crate::projector::ProjectorDispatch;
use crate::router::{
    CommandHandler, CommandHandlers, ProcessManagerHandler, ProjectorHandler, Router, RouterBuilder,
};
use crate::saga::SagaDispatch;
use crate::test_support::{
    command_page, cover_applier, event_page, fact_page, fresh_rebuilder, TestState,
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
fn aggregate(label: &'static str, domain: &str, commands: &[&str]) -> AggregateDispatch<TestState> {
    let mut agg = AggregateDispatch::new(label, domain, fresh_rebuilder());
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

#[test]
fn mixed_handler_kinds_are_rejected() {
    let builder = RouterBuilder::new()
        .aggregate(aggregate("order", "order", &["test.Create"]))
        .saga(SagaDispatch::new(
            "OrderFulfillment",
            "order",
            ["inventory"],
        ));
    assert_eq!(build_err(builder), codes::MIXED_HANDLER_KINDS, "C-0063");
}

#[test]
fn duplicate_domain_command_pairs_are_rejected() {
    let builder = RouterBuilder::new()
        .aggregate(aggregate("alpha", "order", &["test.Create"]))
        .aggregate(aggregate("beta", "order", &["test.Create"]));
    assert_eq!(
        build_err(builder),
        codes::DUPLICATE_COMMAND_HANDLER,
        "C-0064"
    );
}

#[test]
fn the_same_command_type_in_two_domains_is_accepted() {
    let router = RouterBuilder::new()
        .aggregate(aggregate("a", "order-a", &["test.Create"]))
        .aggregate(aggregate("b", "order-b", &["test.Create"]))
        .build()
        .expect("C-0011");
    let resp = router
        .dispatch_command(&command("order-b", "test.Create"))
        .expect("dispatch");
    assert_eq!(emitted_label(&resp), "b");
}

#[test]
fn commands_route_by_domain_and_type() {
    let router = RouterBuilder::new()
        .aggregate(aggregate("order-create", "order", &["test.Create"]))
        .aggregate(aggregate("order-cancel", "order", &["test.Cancel"]))
        .aggregate(aggregate("payment", "payment", &["test.Pay"]))
        .build()
        .expect("C-0062");
    for (domain, fq, label) in [
        ("order", "test.Create", "order-create"),
        ("order", "test.Cancel", "order-cancel"),
        ("payment", "test.Pay", "payment"),
    ] {
        let resp = router
            .dispatch_command(&command(domain, fq))
            .expect("dispatch");
        assert_eq!(emitted_label(&resp), label);
    }
    let err = router
        .dispatch_command(&command("payment", "test.Create"))
        .expect_err("no handler for (payment, Create)");
    assert_eq!(err.code, codes::NO_HANDLER_REGISTERED);
    assert_eq!(
        err.extras.get("upcast_type_url").map(String::as_str),
        Some(type_url("test.Create").as_str()),
        "no chain ran: the sent type"
    );
    // command_handler.proto, Handle step 2: INVALID_ARGUMENT naming the type
    // URL (decision 12.11, backlog VC-1).
    assert_eq!(err.grpc, crate::error::GrpcCode::InvalidArgument);
    assert_eq!(
        err.message,
        format!("no handler for type URL {}", type_url("test.Create"))
    );
    let err = router
        .dispatch_command(&fact("payment", "test.Paid"))
        .expect_err("no aggregate records Paid");
    assert_eq!(err.code, codes::NO_FACT_HANDLER);
    assert_eq!(err.grpc, crate::error::GrpcCode::InvalidArgument);
}

/// An aggregate in `domain` recording `fq` facts as `label` events.
fn recorder(label: &'static str, domain: &str, fq: &str) -> AggregateDispatch<TestState> {
    AggregateDispatch::new(label, domain, fresh_rebuilder()).on_fact(fq, move |_, _| {
        Ok(crate::aggregate::FactRecord::from(any_of_type(label)))
    })
}

#[test]
fn facts_route_by_domain_and_fact_type() {
    let router = RouterBuilder::new()
        .aggregate(aggregate("order", "order", &["test.Create"]))
        .aggregate(recorder("paid", "order", "test.Paid"))
        .aggregate(recorder("shipped", "order", "test.Shipped"))
        .build()
        .expect("build");
    for (fq, label) in [("test.Paid", "paid"), ("test.Shipped", "shipped")] {
        let resp = router.dispatch_command(&fact("order", fq)).expect("fact");
        assert_eq!(emitted_label(&resp), label, "the aggregate recording {fq}");
    }
    let err = router
        .dispatch_command(&fact("order", "test.Unknown"))
        .expect_err("no aggregate records Unknown");
    assert_eq!(
        err.code,
        codes::NO_FACT_HANDLER,
        "the domain's first refuses it"
    );
    let err = router
        .dispatch_command(&fact("billing", "test.Paid"))
        .expect_err("no aggregate for billing");
    assert_eq!(err.code, codes::NO_FACT_HANDLER);
    assert_eq!(err.grpc, crate::error::GrpcCode::InvalidArgument);
}

#[test]
fn sagas_fan_out_and_merge_in_registration_order() {
    let saga = |label: &'static str| {
        SagaDispatch::new(label, "order", ["inventory"]).on_event("test.Created", move |_, _| {
            Ok(vec![pb::CommandBook {
                cover: Some(pb::Cover {
                    domain: label.to_string(),
                    ..Default::default()
                }),
                page: Some(pb::CommandPage::default()),
                ..Default::default()
            }])
        })
    };
    let router = RouterBuilder::new()
        .saga(saga("first"))
        .saga(saga("second"))
        .build()
        .expect("build");
    let req = pb::EventBook {
        cover: Some(pb::Cover {
            domain: "order".to_string(),
            ..Default::default()
        }),
        pages: vec![event_page(any_of_type("test.Created"))],
        ..Default::default()
    };
    let resp = router.dispatch_saga(&req).expect("dispatch");
    let order: Vec<_> = resp
        .commands
        .iter()
        .map(|c| c.cover.as_ref().unwrap().domain.clone())
        .collect();
    assert_eq!(order, vec!["first", "second"], "C-0013");
    let mut other = req.clone();
    other.cover.as_mut().unwrap().domain = "billing".to_string();
    assert_eq!(
        router.dispatch_saga(&other).unwrap_err().code,
        codes::NO_HANDLER_REGISTERED
    );
}

#[test]
fn process_managers_route_through_selection() {
    let pm = |name: &'static str| {
        ProcessManagerDispatch::new(name, format!("{name}-pm"), ["inventory"], fresh_rebuilder())
            .on_event("order", "test.Created", move |_, _, _| {
                Ok(pb::ProcessManagerHandleResponse {
                    process_events: vec![pb::EventBook {
                        cover: Some(pb::Cover {
                            domain: name.to_string(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }],
                    ..Default::default()
                })
            })
    };
    let router = RouterBuilder::new()
        .process_manager(pm("a"))
        .process_manager(pm("b"))
        .build()
        .expect("build");
    let mut req = pb::ProcessManagerHandleRequest {
        trigger: Some(pb::EventBook {
            cover: Some(pb::Cover {
                domain: "order".to_string(),
                ..Default::default()
            }),
            pages: vec![event_page(any_of_type("test.Created"))],
            ..Default::default()
        }),
        process_state: None,
    };
    let reactions = |resp: pb::ProcessManagerHandleResponse| -> Vec<String> {
        resp.process_events
            .into_iter()
            .map(|f| f.cover.unwrap().domain)
            .collect()
    };
    assert_eq!(
        reactions(router.dispatch_process_manager(&req).unwrap()),
        vec!["a", "b"]
    );
    req.process_state = Some(pb::EventBook {
        cover: Some(pb::Cover {
            domain: "b-pm".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    });
    assert_eq!(
        reactions(router.dispatch_process_manager(&req).unwrap()),
        vec!["b"]
    );
}

#[test]
fn projectors_each_produce_a_projection() {
    let router = RouterBuilder::new()
        .projector(ProjectorDispatch::new("p1", || ()))
        .projector(ProjectorDispatch::new("p2", || ()))
        .build()
        .expect("build");
    let book = pb::EventBook {
        cover: Some(pb::Cover {
            domain: "order".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let names: Vec<_> = router
        .dispatch_projectors(&book)
        .expect("dispatch")
        .into_iter()
        .map(|p| p.projector)
        .collect();
    assert_eq!(names, vec!["p1", "p2"], "C-0015");
}

#[test]
fn upcasters_chain_across_components_for_their_domain() {
    let router = RouterBuilder::new()
        .upcaster(
            UpcasterDispatch::new("v1-v2", "order")
                .on_event("test.V1", |_| Ok(any_of_type("test.V2"))),
        )
        .upcaster(
            UpcasterDispatch::new("v2-v3", "order")
                .on_event("test.V2", |_| Ok(any_of_type("test.V3"))),
        )
        .upcaster(
            UpcasterDispatch::new("billing", "billing")
                .on_event("test.V3", |_| Ok(any_of_type("test.X"))),
        )
        .build()
        .expect("build");
    let resp = router
        .upcast(&pb::UpcastRequest {
            domain: "order".to_string(),
            events: vec![event_page(any_of_type("test.V1"))],
        })
        .expect("upcast");
    let Some(any) = resp.events[0].payload.as_ref() else {
        panic!("no event");
    };
    assert_eq!(
        crate::type_name_from_url(&any.type_url),
        "test.V3",
        "C-0136"
    );
}

#[test]
fn a_router_refuses_dispatch_of_another_kind() {
    let router: Router = RouterBuilder::new()
        .saga(SagaDispatch::new("s", "order", ["inventory"]))
        .build()
        .expect("build");
    assert_eq!(
        router
            .dispatch_command(&command("order", "test.Create"))
            .unwrap_err()
            .code,
        codes::NO_HANDLER_REGISTERED
    );
    assert_eq!(
        router
            .upcast(&pb::UpcastRequest::default())
            .unwrap_err()
            .code,
        codes::NO_HANDLER_REGISTERED
    );
}

// --- boxed components ------------------------------------------------------

#[test]
fn boxed_components_build_routers() {
    let boxed: Box<dyn CommandHandler> = Box::new(aggregate("boxed", "order", &["test.Create"]));
    let router = RouterBuilder::new()
        .aggregate(boxed)
        .build()
        .expect("build");
    let resp = router
        .dispatch_command(&command("order", "test.Create"))
        .expect("dispatch");
    assert_eq!(emitted_label(&resp), "boxed");

    let pm: Box<dyn ProcessManagerHandler> = Box::new(
        ProcessManagerDispatch::new("pm", "pm", ["inventory"], fresh_rebuilder()).on_event(
            "order",
            "test.Created",
            |_, _, _| {
                Ok(pb::ProcessManagerHandleResponse {
                    process_events: vec![pb::EventBook::default()],
                    ..Default::default()
                })
            },
        ),
    );
    let router = RouterBuilder::new()
        .process_manager(pm)
        .build()
        .expect("build");
    let resp = router
        .dispatch_process_manager(&pb::ProcessManagerHandleRequest {
            trigger: Some(pb::EventBook {
                cover: Some(pb::Cover {
                    domain: "order".to_string(),
                    ..Default::default()
                }),
                pages: vec![event_page(any_of_type("test.Created"))],
                ..Default::default()
            }),
            process_state: None,
        })
        .expect("dispatch");
    assert_eq!(resp.process_events.len(), 1);

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
    let projections = router
        .dispatch_projectors(&pb::EventBook {
            cover: Some(pb::Cover::default()),
            ..Default::default()
        })
        .expect("dispatch");
    assert_eq!(projections[0].projector, "boxed");
}

#[test]
fn a_boxed_aggregate_keeps_its_routing_view() {
    let boxed: Box<dyn CommandHandler> = Box::new(aggregate("boxed", "order", &["test.Create"]));
    assert_eq!(boxed.domain(), "order");
    assert_eq!(boxed.command_types(), vec!["test.Create".to_string()]);
}

// --- Compensate delivery across a shared domain ------------------------------

// --- boxed and hand-written components ---------------------------------------

#[test]
fn a_boxed_aggregate_handles_facts() {
    let boxed: Box<dyn CommandHandler> = Box::new(recorder("paid", "order", "test.Paid"));
    assert_eq!(boxed.fact_types(), vec!["test.Paid".to_string()]);
    let resp = boxed.dispatch(&fact("order", "test.Paid")).expect("fact");
    assert_eq!(emitted_label(&resp), "paid");
}

#[test]
fn a_boxed_process_manager_keeps_its_routing_view() {
    let boxed: Box<dyn ProcessManagerHandler> = Box::new(
        ProcessManagerDispatch::new(
            "fulfil",
            "fulfil-pm",
            ["inventory"],
            cover_applier(fresh_rebuilder()),
        )
        .on_event("order", "test.Created", |_, _, _| {
            Ok(pb::ProcessManagerHandleResponse::default())
        }),
    );
    assert_eq!(boxed.name(), "fulfil");
    assert_eq!(boxed.pm_domain(), "fulfil-pm");
    assert!(boxed.consumes("order"));
    assert!(!boxed.consumes("billing"));
}

/// A hand-written aggregate that records no facts.
struct Plain;

impl CommandHandler for Plain {
    fn domain(&self) -> &str {
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
}

#[test]
fn a_hand_written_aggregate_records_no_facts_by_default() {
    assert!(Plain.fact_types().is_empty());
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
}

// --- CommandHandlers ---------------------------------------------------------

#[test]
fn command_handlers_report_the_domains_they_serve_and_a_sole_handler() {
    let mut handlers = CommandHandlers::new();
    assert!(handlers.sole().is_none(), "no handler");
    assert!(!handlers.serves("order"));
    handlers
        .push(Box::new(aggregate("order", "order", &["test.Create"])))
        .expect("push");
    assert_eq!(handlers.sole().map(|h| h.domain()), Some("order"));
    assert!(handlers.serves("order"));
    assert!(!handlers.serves("billing"));
    handlers
        .push(Box::new(aggregate("billing", "billing", &["test.Bill"])))
        .expect("push");
    assert!(handlers.sole().is_none(), "two handlers");
    assert!(handlers.serves("billing"));
}

#[test]
fn a_refused_push_leaves_the_table_unchanged() {
    let mut handlers = CommandHandlers::new();
    handlers
        .push(Box::new(aggregate("first", "order", &["test.Create"])))
        .expect("push");
    let err = handlers
        .push(Box::new(aggregate(
            "rival",
            "order",
            &["test.Ship", "test.Create"],
        )))
        .unwrap_err();
    assert_eq!(err.code, codes::DUPLICATE_COMMAND_HANDLER);
    assert_eq!(
        err.extras.get("command_type").map(String::as_str),
        Some("test.Create")
    );
    assert!(handlers.sole().is_some(), "the rival was not added");
    let err = handlers
        .dispatch(&command("order", "test.Ship"))
        .unwrap_err();
    assert_eq!(
        err.code,
        codes::NO_HANDLER_REGISTERED,
        "no claim of Ship survived"
    );
}

// An old type no aggregate claims as sent routes to the aggregate whose
// upcaster chain brings it to a type it claims (upcaster.proto; VC-1).
#[test]
fn an_old_command_type_routes_to_the_aggregate_its_upcaster_chain_serves() {
    let upcaster = UpcasterDispatch::new("rename", "order").on_event("test.ShipV1", |any| {
        Ok(Any {
            type_url: type_url("test.Ship"),
            value: any.value.clone(),
        })
    });
    let router = RouterBuilder::new()
        .aggregate(aggregate("create", "order", &["test.Create"]))
        .aggregate(aggregate("ship", "order", &["test.Ship"]).with_command_upcaster(upcaster))
        .build()
        .expect("build");
    let resp = router
        .dispatch_command(&command("order", "test.ShipV1"))
        .expect("upcast and routed");
    assert_eq!(emitted_label(&resp), "ship");
    let err = router
        .dispatch_command(&command("order", "test.Unknown"))
        .expect_err("no path to a handler");
    assert_eq!(err.code, codes::NO_HANDLER_REGISTERED);
    assert_eq!(err.grpc, crate::error::GrpcCode::InvalidArgument);
}

/// A chain renaming `from` to `to`, for the domain "order".
fn renaming(from: &str, to: &'static str) -> UpcasterDispatch {
    UpcasterDispatch::new("rename", "order").on_event(from, move |any| {
        Ok(Any {
            type_url: type_url(to),
            value: any.value.clone(),
        })
    })
}

// Upcaster source types are claimed at registration like command types
// (owner ruling): two aggregates accepting one old type, or an old type
// colliding with a current one, fail the build.
#[test]
fn upcaster_source_types_are_claimed_at_registration() {
    let err = build_err(
        RouterBuilder::new()
            .aggregate(
                aggregate("a", "order", &["test.Ship"])
                    .with_command_upcaster(renaming("test.ShipV1", "test.Ship")),
            )
            .aggregate(
                aggregate("b", "order", &["test.Send"])
                    .with_command_upcaster(renaming("test.ShipV1", "test.Send")),
            ),
    );
    assert_eq!(
        err,
        codes::DUPLICATE_COMMAND_HANDLER,
        "one old type, two claimants"
    );

    let err = build_err(
        RouterBuilder::new()
            .aggregate(aggregate("a", "order", &["test.Create"]))
            .aggregate(
                aggregate("b", "order", &["test.Ship"])
                    .with_command_upcaster(renaming("test.Create", "test.Ship")),
            ),
    );
    assert_eq!(
        err,
        codes::DUPLICATE_COMMAND_HANDLER,
        "an old type that is a current one"
    );

    let err = build_err(
        RouterBuilder::new().aggregate(
            aggregate("a", "order", &["test.Ship", "test.ShipV1"])
                .with_command_upcaster(renaming("test.ShipV1", "test.Ship")),
        ),
    );
    assert_eq!(
        err,
        codes::DUPLICATE_COMMAND_HANDLER,
        "within one aggregate too"
    );
}

#[test]
fn a_claimed_old_type_that_upcasts_to_nothing_is_refused_by_its_claimant() {
    let router = RouterBuilder::new()
        .aggregate(aggregate("create", "order", &["test.Create"]))
        .aggregate(
            aggregate("ship", "order", &["test.Ship"])
                .with_command_upcaster(renaming("test.ShipOld", "test.ShipLost")),
        )
        .build()
        .expect("build");
    let err = router
        .dispatch_command(&command("order", "test.ShipOld"))
        .expect_err("ShipLost reaches no handler");
    assert_eq!(err.code, codes::NO_HANDLER_REGISTERED);
    assert_eq!(
        err.extras.get("upcast_type_url").map(String::as_str),
        Some(type_url("test.ShipLost").as_str()),
        "the claimant's chain ran"
    );
}

#[test]
fn an_old_fact_type_routes_to_the_aggregate_whose_chain_claims_it() {
    let router = RouterBuilder::new()
        .aggregate(recorder("paid", "order", "test.Paid"))
        .aggregate(
            recorder("shipped", "order", "test.Shipped")
                .with_command_upcaster(renaming("test.ShippedV1", "test.Shipped")),
        )
        .build()
        .expect("build");
    let resp = router
        .dispatch_command(&fact("order", "test.ShippedV1"))
        .expect("upcast and routed");
    assert_eq!(emitted_label(&resp), "shipped");
}

// One aggregate's chains may name a source type more than once (rules on
// one type compose in order, C-0136): its own claims never collide.
#[test]
fn an_aggregate_naming_a_source_type_twice_claims_it_once() {
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
