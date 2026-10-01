//! RouterBuilder / Router: build-time validation (C-0060, C-0062..C-0064)
//! and dispatch across composed components of one kind.

use prost_types::Any;

use crate::aggregate::AggregateDispatch;
use crate::error::codes;
use crate::pb;
use crate::process_manager::ProcessManagerDispatch;
use crate::projector::ProjectorDispatch;
use crate::router::{Router, RouterBuilder};
use crate::saga::SagaDispatch;
use crate::test_support::{event_page, fresh_rebuilder, notification_page_for, TestState};
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
            pages: vec![pb::CommandPage {
                payload: Some(pb::command_page::Payload::Command(any_of_type(fq))),
                ..Default::default()
            }],
        }),
        events: None,
    }
}

fn emitted_label(resp: &pb::BusinessResponse) -> String {
    match &resp.result {
        Some(pb::business_response::Result::Events(book)) => match book.pages[0].payload.as_ref() {
            Some(pb::event_page::Payload::Event(any)) => {
                crate::type_name_from_url(&any.type_url).to_string()
            }
            _ => String::new(),
        },
        _ => String::new(),
    }
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
fn ambiguous_compensates_entries_fail_the_build() {
    let agg = aggregate("a", "order", &["test.Create"])
        .on_rejected("test.Reserve", |_, _, _, _| {
            Ok(pb::BusinessResponse::default())
        })
        .on_rejected("inventory:test.Reserve", |_, _, _, _| {
            Ok(pb::BusinessResponse::default())
        });
    assert_eq!(
        build_err(RouterBuilder::new().aggregate(agg)),
        codes::AMBIGUOUS_COMPENSATION
    );
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
}

#[test]
fn a_notification_reaches_the_aggregate_declaring_its_compensation() {
    let declares = aggregate("declares", "order", &["test.Cancel"]).on_rejected(
        "test.ReserveStock",
        |_, _, _, _| {
            Ok(pb::BusinessResponse {
                result: Some(pb::business_response::Result::Events(pb::EventBook {
                    pages: vec![event_page(any_of_type("compensated"))],
                    ..Default::default()
                })),
            })
        },
    );
    let router = RouterBuilder::new()
        .aggregate(aggregate("first", "order", &["test.Create"]))
        .aggregate(declares)
        .build()
        .expect("build");
    let notification = match notification_page_for("test.ReserveStock").payload {
        Some(pb::event_page::Payload::Event(any)) => any,
        _ => unreachable!(),
    };
    let mut req = command("order", "test.Unused");
    req.command.as_mut().unwrap().pages[0].payload =
        Some(pb::command_page::Payload::Command(notification));
    let resp = router.dispatch_command(&req).expect("dispatch");
    assert_eq!(emitted_label(&resp), "compensated");
}

#[test]
fn facts_route_to_the_domain_aggregate() {
    let router = RouterBuilder::new()
        .aggregate(aggregate("order", "order", &["test.Create"]))
        .build()
        .expect("build");
    let facts = pb::EventBook {
        cover: Some(pb::Cover {
            domain: "order".to_string(),
            ..Default::default()
        }),
        pages: vec![event_page(any_of_type("test.Fact"))],
        ..Default::default()
    };
    let out = router
        .handle_fact(&pb::FactRequest {
            facts: Some(facts.clone()),
            prior_events: None,
        })
        .expect("fact");
    assert_eq!(out, facts);
    let err = router
        .handle_fact(&pb::FactRequest {
            facts: Some(pb::EventBook {
                cover: Some(pb::Cover {
                    domain: "billing".to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            prior_events: None,
        })
        .expect_err("no aggregate for billing");
    assert_eq!(err.code, codes::NO_HANDLER_REGISTERED);
}

#[test]
fn sagas_fan_out_and_merge_in_registration_order() {
    let saga = |label: &'static str| {
        SagaDispatch::new(label, "order", ["inventory"]).on_event("test.Created", move |_, _, _| {
            Ok((
                vec![pb::CommandBook {
                    cover: Some(pb::Cover {
                        domain: label.to_string(),
                        ..Default::default()
                    }),
                    pages: vec![pb::CommandPage::default()],
                }],
                Vec::new(),
            ))
        })
    };
    let router = RouterBuilder::new()
        .saga(saga("first"))
        .saga(saga("second"))
        .build()
        .expect("build");
    let req = pb::SagaHandleRequest {
        source: Some(pb::EventBook {
            cover: Some(pb::Cover {
                domain: "order".to_string(),
                ..Default::default()
            }),
            pages: vec![event_page(any_of_type("test.Created"))],
            ..Default::default()
        }),
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
    other
        .source
        .as_mut()
        .unwrap()
        .cover
        .as_mut()
        .unwrap()
        .domain = "billing".to_string();
    assert_eq!(
        router.dispatch_saga(&other).unwrap_err().code,
        codes::NO_HANDLER_REGISTERED
    );
}

#[test]
fn process_managers_route_through_selection() {
    let pm = |name: &'static str| {
        ProcessManagerDispatch::new(name, format!("{name}-pm"), ["inventory"], fresh_rebuilder())
            .on_event("order", "test.Created", move |_, _, _, _| {
                Ok(pb::ProcessManagerHandleResponse {
                    facts: vec![pb::EventBook {
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
    let facts = |resp: pb::ProcessManagerHandleResponse| -> Vec<String> {
        resp.facts
            .into_iter()
            .map(|f| f.cover.unwrap().domain)
            .collect()
    };
    assert_eq!(
        facts(router.dispatch_process_manager(&req).unwrap()),
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
        facts(router.dispatch_process_manager(&req).unwrap()),
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
    let Some(pb::event_page::Payload::Event(any)) = resp.events[0].payload.as_ref() else {
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
