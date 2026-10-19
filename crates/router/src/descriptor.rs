//! The registration hello: the spec's ComponentDescriptor (types.proto),
//! built once here from a component's dispatch table for every language.
//! Hosts only send it to their coordinator.

use std::collections::BTreeSet;

use crate::error::{codes, messages, CodedError};
use crate::pb;

/// A component's ComponentDescriptor. `inputs` are the domains whose events
/// it consumes (deduplicated and sorted); `state` is set for aggregates and
/// process managers only. An empty `rules_version` is refused
/// MISSING_RULES_VERSION (INVALID_ARGUMENT), as the coordinator refuses
/// such a registration.
pub(crate) fn component_descriptor(
    name: &str,
    kind: pb::ComponentKind,
    inputs: impl IntoIterator<Item = String>,
    rules_version: &str,
    state: Option<pb::StateDescriptor>,
) -> Result<pb::ComponentDescriptor, CodedError> {
    if rules_version.is_empty() {
        return Err(CodedError::invalid_argument(
            codes::MISSING_RULES_VERSION,
            messages::MISSING_RULES_VERSION,
            [],
        ));
    }
    let inputs: BTreeSet<String> = inputs.into_iter().collect();
    Ok(pb::ComponentDescriptor {
        name: name.to_string(),
        kind: kind as i32,
        inputs: inputs
            .into_iter()
            .map(|domain| pb::Target { domain })
            .collect(),
        rules_version: rules_version.to_string(),
        state,
    })
}

/// The StateDescriptor of the state message named `full_name`: its type URL
/// and the full resolved list of supported resolutions, in enum-number
/// order. The base is STRICT and AGGREGATE_HANDLES; COMMUTATIVE (number 1)
/// leads when the state has a `changed()`.
pub(crate) fn state_descriptor(full_name: &str, commutative: bool) -> pb::StateDescriptor {
    let mut resolutions = Vec::new();
    if commutative {
        resolutions.push(pb::ConflictResolution::Commutative as i32);
    }
    resolutions.push(pb::ConflictResolution::Strict as i32);
    resolutions.push(pb::ConflictResolution::AggregateHandles as i32);
    pb::StateDescriptor {
        type_url: crate::type_url(full_name),
        options: Some(pb::StateOptions {
            conflict_resolutions: resolutions,
        }),
    }
}

#[cfg(test)]
#[path = "descriptor.test.rs"]
mod descriptor_tests;
