//! Destinations — the per-dispatch sequence table sagas and process managers
//! stamp emitted commands from. The coordinator supplies one next-sequence
//! per output domain (config-driven); the component is a translator, not a
//! decision maker, so it never rebuilds destination state — it only stamps.
//!
//! Transliterated from client-go's `destinations.go`.

use std::collections::HashMap;

use crate::error::{codes, extras, messages, CodedError};
use crate::pb;

/// Destination next-sequences keyed by domain, as supplied by the
/// coordinator in the dispatch request.
pub struct Destinations {
    sequences: HashMap<String, u32>,
}

impl Destinations {
    /// Wraps the coordinator-supplied sequence map.
    pub fn new(sequences: HashMap<String, u32>) -> Self {
        Destinations { sequences }
    }

    /// The next sequence for a domain, or None when the coordinator
    /// supplied none.
    pub fn sequence_for(&self, domain: &str) -> Option<u32> {
        self.sequences.get(domain).copied()
    }

    /// True when a sequence exists for the domain.
    pub fn has(&self, domain: &str) -> bool {
        self.sequences.contains_key(domain)
    }

    /// The domains carrying a sequence (unordered).
    pub fn domains(&self) -> Vec<String> {
        self.sequences.keys().cloned().collect()
    }

    /// Stamps every page of `cmd` with the next sequence for `domain`.
    /// A domain with no supplied sequence is the coded
    /// MISSING_DESTINATION_SEQUENCE (check output_domains config).
    pub fn stamp_command(&self, cmd: &mut pb::CommandBook, domain: &str) -> Result<(), CodedError> {
        let Some(seq) = self.sequences.get(domain).copied() else {
            return Err(CodedError::invalid_argument(
                codes::MISSING_DESTINATION_SEQUENCE,
                messages::MISSING_DESTINATION_SEQUENCE,
                [(extras::DOMAIN.to_string(), domain.to_string())],
            ));
        };
        for page in &mut cmd.pages {
            page.header = Some(pb::PageHeader {
                sequence_type: Some(pb::page_header::SequenceType::Sequence(seq)),
                ..Default::default()
            });
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "destinations.test.rs"]
mod destinations_tests;
