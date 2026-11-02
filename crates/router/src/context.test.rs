//! The context a handler is handed ([`command_context`]), read from the
//! `ContextualCommand` alone.

use super::*;

// Decision 14.37 (C-0671): the input's acceptance time, stamped by the
// coordinator or the angzarr-embedded kernel, reaches the handler as
// delivered, so business code reads no clock of its own.
#[test]
fn the_context_carries_when_the_input_was_accepted() {
    let accepted = prost_types::Timestamp {
        seconds: 1_793_188_800,
        nanos: 5,
    };
    let req = pb::ContextualCommand {
        recorded_at: Some(accepted),
        ..Default::default()
    };
    assert_eq!(
        command_context(&req).recorded_at,
        Some(accepted),
        "CommandContext.recorded_at is ContextualCommand.recorded_at"
    );
}

#[test]
fn no_acceptance_time_delivered_is_none() {
    assert_eq!(
        command_context(&pb::ContextualCommand::default()).recorded_at,
        None
    );
}

// The full-context rule: the meta a decision's events are stamped and
// folded with is the context's, field for field, under the rules version
// given; a context with no command id names no causation.
#[test]
fn event_meta_is_the_context_s_identity_under_the_rules_version() {
    let id = pb::Uuid {
        value: vec![1; 16].into(),
    };
    let ctx = CommandContext {
        command_id: Some(id.clone()),
        correlation_id: Some(pb::Uuid {
            value: vec![2; 16].into(),
        }),
        fact_origin: Some(pb::FactOrigin {
            external_id: "pi_1".to_string(),
            ..Default::default()
        }),
        recorded_at: Some(prost_types::Timestamp {
            seconds: 3,
            nanos: 4,
        }),
        ..Default::default()
    };
    assert_eq!(
        ctx.event_meta("v9"),
        pb::EventMeta {
            causation: Some(pb::event_meta::Causation::CommandId(id)),
            fact: ctx.fact_origin.clone(),
            correlation_id: ctx.correlation_id.clone(),
            recorded_at: ctx.recorded_at,
            rules_version: "v9".to_string(),
        }
    );
    assert_eq!(CommandContext::default().event_meta("").causation, None);
}
