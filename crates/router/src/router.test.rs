//! RouterBuilder / Router: build-time validation (C-0060, C-0062..C-0064)
//! and dispatch across composed components of one kind.

use prost::Message;
use prost_types::Any;

use crate::aggregate::AggregateDispatch;
use crate::error::{codes, CodedError};
use crate::pb;
use crate::process_manager::{ProcessManagerDispatch, ProcessManagerRoute};
use crate::projector::ProjectorDispatch;
use crate::rebuild::Rebuilder;
use crate::router::{
    CommandHandler, CommandHandlers, ProcessManagerHandler, ProjectorHandler, Router, RouterBuilder,
};
use crate::saga::SagaDispatch;
use crate::test_support::{
    any_of, cover_any, cover_applier, event_page, fresh_rebuilder, notification_page_for,
    sequenced_event_page, TestState,
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

#[test]
fn an_ambiguous_process_manager_fails_the_build() {
    let pm = ProcessManagerDispatch::new("pm", "pm", ["inventory"], fresh_rebuilder())
        .on_rejected("test.Reserve", |_, _, _| {
            Ok(pb::ProcessManagerHandleResponse::default())
        })
        .on_rejected("inventory:test.Reserve", |_, _, _| {
            Ok(pb::ProcessManagerHandleResponse::default())
        });
    assert_eq!(
        build_err(RouterBuilder::new().process_manager(pm)),
        codes::AMBIGUOUS_COMPENSATION
    );
}

// --- every claimant of a notification runs (C-0042) -------------------------

fn compensating(label: &'static str, events: usize) -> AggregateDispatch<TestState> {
    AggregateDispatch::new(label, "payment", fresh_rebuilder()).on_rejected(
        "test.ReserveStock",
        move |_, _, _, _| {
            Ok(pb::BusinessResponse {
                result: Some(pb::business_response::Result::Events(pb::EventBook {
                    pages: (0..events)
                        .map(|_| event_page(any_of_type(label)))
                        .collect(),
                    ..Default::default()
                })),
            })
        },
    )
}

fn rejection_delivery(domain: &str, prior: u32) -> pb::ContextualCommand {
    let notification = match notification_page_for("test.ReserveStock").payload {
        Some(pb::event_page::Payload::Event(any)) => any,
        _ => unreachable!(),
    };
    let mut req = command(domain, "test.Unused");
    req.command.as_mut().unwrap().pages[0].payload =
        Some(pb::command_page::Payload::Command(notification));
    if prior > 0 {
        req.events = Some(pb::EventBook {
            pages: (0..prior)
                .map(|seq| sequenced_event_page(seq, any_of_type("test.Prior")))
                .collect(),
            next_sequence: prior,
            ..Default::default()
        });
    }
    req
}

fn labels_and_sequences(resp: &pb::BusinessResponse) -> Vec<(String, u32)> {
    let Some(pb::business_response::Result::Events(book)) = &resp.result else {
        panic!("expected events, got {:?}", resp.result);
    };
    book.pages
        .iter()
        .map(|page| {
            let label = match page.payload.as_ref() {
                Some(pb::event_page::Payload::Event(any)) => {
                    crate::type_name_from_url(&any.type_url).to_string()
                }
                _ => String::new(),
            };
            let seq = match page.header.as_ref().and_then(|h| h.sequence_type.as_ref()) {
                Some(pb::page_header::SequenceType::Sequence(s)) => *s,
                other => panic!("no sequence: {other:?}"),
            };
            (label, seq)
        })
        .collect()
}

#[test]
fn every_aggregate_claiming_a_rejection_runs_in_registration_order() {
    let router = RouterBuilder::new()
        .aggregate(compensating("first", 1))
        .aggregate(aggregate("bystander", "payment", &["test.Pay"]))
        .aggregate(compensating("second", 2))
        .aggregate(compensating("elsewhere", 1).on_command("test.Other", |_, _, _| Ok(None)))
        .build()
        .expect("build");
    let resp = router
        .dispatch_command(&rejection_delivery("payment", 7))
        .expect("dispatch");
    assert_eq!(
        labels_and_sequences(&resp),
        vec![
            ("first".to_string(), 7),
            ("second".to_string(), 8),
            ("second".to_string(), 9),
            ("elsewhere".to_string(), 10),
        ],
        "claimants run in registration order; sequences continue across them"
    );
}

#[test]
fn a_claimant_escalation_wins_over_merged_events() {
    let escalating = AggregateDispatch::new("escalates", "payment", fresh_rebuilder()).on_rejected(
        "test.ReserveStock",
        |_, _, _, _| {
            Ok(pb::BusinessResponse {
                result: Some(pb::business_response::Result::Revocation(
                    pb::RevocationResponse {
                        reason: "escalated".to_string(),
                        ..Default::default()
                    },
                )),
            })
        },
    );
    let router = RouterBuilder::new()
        .aggregate(compensating("first", 1))
        .aggregate(escalating)
        .aggregate(compensating("third", 1))
        .build()
        .expect("build");
    let resp = router
        .dispatch_command(&rejection_delivery("payment", 0))
        .expect("dispatch");
    match resp.result {
        Some(pb::business_response::Result::Revocation(r)) => assert_eq!(r.reason, "escalated"),
        other => panic!("expected the escalation, got {other:?}"),
    }
}

#[test]
fn a_claimant_failure_fails_the_delivery() {
    let failing = AggregateDispatch::new("fails", "payment", fresh_rebuilder())
        .on_rejected("test.ReserveStock", |_, _, _, _| {
            Err(crate::error::HandlerError::Other("boom".to_string()))
        });
    let router = RouterBuilder::new()
        .aggregate(compensating("first", 1))
        .aggregate(failing)
        .build()
        .expect("build");
    assert_eq!(
        router
            .dispatch_command(&rejection_delivery("payment", 0))
            .unwrap_err()
            .code,
        codes::UNHANDLED_HANDLER_ERROR
    );
}

#[test]
fn a_rejection_no_aggregate_claims_is_left_to_the_framework() {
    let router = RouterBuilder::new()
        .aggregate(aggregate("only", "payment", &["test.Pay"]))
        .build()
        .expect("build");
    let resp = router
        .dispatch_command(&rejection_delivery("payment", 0))
        .expect("dispatch");
    assert_eq!(resp.result, None, "DelegateToFramework");
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
            |_, _, _, _| {
                Ok(pb::ProcessManagerHandleResponse {
                    facts: vec![pb::EventBook::default()],
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
    assert_eq!(resp.facts.len(), 1);

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
    let boxed: Box<dyn CommandHandler> = Box::new(
        aggregate("boxed", "order", &["test.Create"])
            .on_rejected("test.ReserveStock", |_, _, _, _| {
                Ok(pb::BusinessResponse::default())
            }),
    );
    assert_eq!(boxed.domain(), "order");
    assert_eq!(boxed.command_types(), vec!["test.Create".to_string()]);
    let notification = match notification_page_for("test.ReserveStock").payload {
        Some(pb::event_page::Payload::Event(any)) => any,
        _ => unreachable!(),
    };
    assert!(boxed.claims_notification(&notification));
}

// --- Replay ------------------------------------------------------------------

fn cover_replay(domains: &[&str]) -> pb::ReplayRequest {
    pb::ReplayRequest {
        events: domains.iter().map(|d| event_page(cover_any(d))).collect(),
        ..Default::default()
    }
}

fn applied_domains(packed: &pb::ReplayResponse) -> String {
    String::from_utf8(packed.state.as_ref().expect("state").value.clone()).unwrap()
}

fn pack_applied(state: &TestState) -> Result<Any, crate::error::HandlerError> {
    Ok(Any {
        type_url: type_url("test.Applied"),
        value: state.applied.join(",").into_bytes(),
    })
}

#[test]
fn replay_routes_to_the_aggregate_of_the_domain_and_packs_its_state() {
    let router = RouterBuilder::new()
        .aggregate(aggregate("other", "billing", &["test.Bill"]))
        .aggregate(
            AggregateDispatch::new("order", "order", cover_applier(fresh_rebuilder()))
                .with_state_packer(pack_applied),
        )
        .build()
        .expect("build");
    let resp = router
        .replay("order", &cover_replay(&["a", "b"]))
        .expect("replay");
    assert_eq!(
        resp.state.as_ref().unwrap().type_url,
        type_url("test.Applied")
    );
    assert_eq!(applied_domains(&resp), "a,b");
}

#[test]
fn replay_folds_the_base_snapshot_first() {
    let rebuilder = cover_applier(fresh_rebuilder()).with_snapshot(|s: &mut TestState, any| {
        s.applied.push(format!("snap:{}", any.type_url));
        Ok(())
    });
    let router = RouterBuilder::new()
        .aggregate(
            AggregateDispatch::new("order", "order", rebuilder).with_state_packer(pack_applied),
        )
        .build()
        .expect("build");
    let mut req = cover_replay(&["a"]);
    req.base_snapshot = Some(pb::Snapshot {
        state: Some(Any {
            type_url: "/s".to_string(),
            value: vec![1],
        }),
        ..Default::default()
    });
    assert_eq!(
        applied_domains(&router.replay("order", &req).unwrap()),
        "snap:/s,a"
    );
}

#[test]
fn a_message_state_packs_under_its_full_name() {
    let router = RouterBuilder::new()
        .aggregate(
            AggregateDispatch::new(
                "order",
                "order",
                Rebuilder::new(pb::Cover::default).apply(
                    &crate::test_support::cover_full_name(),
                    |s: &mut pb::Cover, any| {
                        let c = pb::Cover::decode(any.value.as_slice())?;
                        s.domain.push_str(&c.domain);
                        Ok(())
                    },
                ),
            )
            .with_message_state(),
        )
        .build()
        .expect("build");
    let resp = router.replay("order", &cover_replay(&["x", "y"])).unwrap();
    assert_eq!(
        resp.state,
        Some(any_of(&pb::Cover {
            domain: "xy".to_string(),
            ..Default::default()
        }))
    );
}

#[test]
fn replay_without_a_state_packer_is_unsupported() {
    let router = RouterBuilder::new()
        .aggregate(aggregate("order", "order", &["test.Create"]))
        .build()
        .expect("build");
    let err = router.replay("order", &cover_replay(&[])).unwrap_err();
    assert_eq!(err.code, codes::NO_HANDLER_REGISTERED);
    assert_eq!(err.extras.get("domain").map(String::as_str), Some("order"));
}

#[test]
fn replay_of_an_unknown_domain_is_no_handler_registered() {
    let router = RouterBuilder::new()
        .aggregate(
            AggregateDispatch::new("order", "order", fresh_rebuilder())
                .with_state_packer(pack_applied),
        )
        .build()
        .expect("build");
    let err = router.replay("billing", &cover_replay(&[])).unwrap_err();
    assert_eq!(err.code, codes::NO_HANDLER_REGISTERED);
    assert_eq!(
        err.extras.get("domain").map(String::as_str),
        Some("billing")
    );
}

#[test]
fn replay_routes_to_the_process_manager_owning_the_domain() {
    let pm = |name: &'static str| {
        ProcessManagerDispatch::new(
            name,
            format!("{name}-pm"),
            ["inventory"],
            cover_applier(fresh_rebuilder()),
        )
        .on_event("order", "test.Created", |_, _, _, _| {
            Ok(pb::ProcessManagerHandleResponse::default())
        })
        .with_state_packer(move |s: &TestState| {
            Ok(Any {
                type_url: type_url(name),
                value: s.applied.join(",").into_bytes(),
            })
        })
    };
    let router = RouterBuilder::new()
        .process_manager(pm("a"))
        .process_manager(pm("b"))
        .build()
        .expect("build");
    let resp = router.replay("b-pm", &cover_replay(&["p", "q"])).unwrap();
    assert_eq!(resp.state.as_ref().unwrap().type_url, type_url("b"));
    assert_eq!(applied_domains(&resp), "p,q");
    assert_eq!(
        router.replay("c-pm", &cover_replay(&[])).unwrap_err().code,
        codes::NO_HANDLER_REGISTERED
    );
}

#[test]
fn replay_on_a_router_without_stateful_components_is_refused() {
    let router = RouterBuilder::new()
        .saga(SagaDispatch::new("s", "order", ["inventory"]))
        .build()
        .expect("build");
    assert_eq!(
        router.replay("order", &cover_replay(&[])).unwrap_err().code,
        codes::NO_HANDLER_REGISTERED
    );
}

// --- Compensate delivery across a shared domain ------------------------------

fn compensate_delivery(domain: &str, command_type: &str) -> pb::ContextualCommand {
    let compensate = pb::Compensate {
        command_type: command_type.to_string(),
        ..Default::default()
    };
    let notification = pb::Notification {
        payload: Some(any_of(&compensate)),
        ..Default::default()
    };
    let mut req = command(domain, "test.Unused");
    req.command.as_mut().unwrap().pages[0].payload =
        Some(pb::command_page::Payload::Command(any_of(&notification)));
    req
}

#[test]
fn a_compensate_reaches_only_the_aggregate_undoing_its_command() {
    let counter: Box<dyn CommandHandler> =
        Box::new(aggregate("counter", "inventory", &["test.Count"]));
    let undoer: Box<dyn CommandHandler> = Box::new(
        AggregateDispatch::new("undoer", "inventory", fresh_rebuilder()).on_undo(
            "test.Reserve",
            |_, _, _, _| {
                Ok(pb::BusinessResponse {
                    result: Some(pb::business_response::Result::Events(pb::EventBook {
                        pages: vec![event_page(any_of_type("released"))],
                        ..Default::default()
                    })),
                })
            },
        ),
    );
    let router = RouterBuilder::new()
        .aggregate(counter)
        .aggregate(undoer)
        .build()
        .expect("build");
    let resp = router
        .dispatch_command(&compensate_delivery("inventory", "test.Reserve"))
        .expect("the undoing aggregate answers; the other never runs");
    assert_eq!(emitted_label(&resp), "released");
}

// --- boxed and hand-written components ---------------------------------------

#[test]
fn a_boxed_aggregate_handles_facts_and_replay() {
    let boxed: Box<dyn CommandHandler> = Box::new(
        AggregateDispatch::new("order", "order", cover_applier(fresh_rebuilder()))
            .with_state_packer(pack_applied),
    );
    let facts = pb::EventBook {
        cover: Some(pb::Cover {
            domain: "order".to_string(),
            ..Default::default()
        }),
        pages: vec![event_page(cover_any("fact"))],
        ..Default::default()
    };
    let recorded = boxed
        .handle_fact(&pb::FactRequest {
            facts: Some(facts.clone()),
            prior_events: None,
        })
        .expect("fact");
    assert_eq!(recorded, facts);
    assert_eq!(
        applied_domains(&boxed.replay(&cover_replay(&["a"])).expect("replay")),
        "a"
    );
}

#[test]
fn a_boxed_process_manager_keeps_its_routing_view_and_replays() {
    let boxed: Box<dyn ProcessManagerHandler> = Box::new(
        ProcessManagerDispatch::new(
            "fulfil",
            "fulfil-pm",
            ["inventory"],
            cover_applier(fresh_rebuilder()),
        )
        .on_event("order", "test.Created", |_, _, _, _| {
            Ok(pb::ProcessManagerHandleResponse::default())
        })
        .with_state_packer(pack_applied),
    );
    assert_eq!(boxed.name(), "fulfil");
    assert_eq!(boxed.pm_domain(), "fulfil-pm");
    assert!(boxed.consumes("order"));
    assert!(!boxed.consumes("billing"));
    assert_eq!(
        applied_domains(&boxed.replay(&cover_replay(&["p"])).expect("replay")),
        "p"
    );
}

/// A hand-written aggregate that does not implement Replay.
struct Plain;

impl CommandHandler for Plain {
    fn domain(&self) -> &str {
        "plain"
    }
    fn command_types(&self) -> Vec<String> {
        vec!["test.Plain".to_string()]
    }
    fn claims_notification(&self, _: &Any) -> bool {
        false
    }
    fn validate(&self) -> Result<(), CodedError> {
        Ok(())
    }
    fn dispatch(&self, _: &pb::ContextualCommand) -> Result<pb::BusinessResponse, CodedError> {
        Ok(pb::BusinessResponse::default())
    }
    fn handle_fact(&self, _: &pb::FactRequest) -> Result<pb::EventBook, CodedError> {
        Ok(pb::EventBook::default())
    }
}

/// A hand-written process manager that does not implement Replay.
struct PlainPm;

impl ProcessManagerRoute for PlainPm {
    fn name(&self) -> &str {
        "plain"
    }
    fn pm_domain(&self) -> &str {
        "plain-pm"
    }
    fn consumes(&self, _: &str) -> bool {
        false
    }
}

impl ProcessManagerHandler for PlainPm {
    fn validate(&self) -> Result<(), CodedError> {
        Ok(())
    }
    fn dispatch(
        &self,
        _: &pb::ProcessManagerHandleRequest,
    ) -> Result<pb::ProcessManagerHandleResponse, CodedError> {
        Ok(pb::ProcessManagerHandleResponse::default())
    }
}

#[test]
fn a_component_without_replay_refuses_it_with_its_domain() {
    let router = RouterBuilder::new()
        .aggregate(Plain)
        .build()
        .expect("build");
    let err = router.replay("plain", &cover_replay(&[])).unwrap_err();
    assert_eq!(err.code, codes::NO_HANDLER_REGISTERED);
    assert_eq!(err.extras.get("domain").map(String::as_str), Some("plain"));

    let router = RouterBuilder::new()
        .process_manager(PlainPm)
        .build()
        .expect("build");
    let err = router.replay("plain-pm", &cover_replay(&[])).unwrap_err();
    assert_eq!(err.code, codes::NO_HANDLER_REGISTERED);
    assert_eq!(
        err.extras.get("domain").map(String::as_str),
        Some("plain-pm")
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
