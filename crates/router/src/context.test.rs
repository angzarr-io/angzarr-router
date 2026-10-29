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
