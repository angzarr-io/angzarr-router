//! Destinations — the output domains a saga or process manager declares
//! (its command targets). Emitted commands are deferred: they carry no
//! destination sequence, and the router stamps their `angzarr_deferred`
//! provenance (see [`crate::stamp_deferred`]), so a component never needs
//! destination state or sequences. [`Destinations::stamp_command`] stamps one
//! command the same way, for code that builds deferred commands itself.

use crate::error::{codes, extras, messages, CodedError};
use crate::pb;

/// The declared output domains of one saga or process manager, in
/// declaration order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Destinations {
    domains: Vec<String>,
}

impl Destinations {
    /// The declared output domains.
    pub fn new(domains: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Destinations {
            domains: domains.into_iter().map(Into::into).collect(),
        }
    }

    /// True when `domain` is a declared output domain.
    pub fn has_domain(&self, domain: &str) -> bool {
        self.domains.iter().any(|d| d == domain)
    }

    /// The declared output domains, in declaration order.
    pub fn domains(&self) -> &[String] {
        &self.domains
    }

    /// The `PageHeader` of a deferred command: `angzarr_deferred` recording
    /// the triggering event (`source_cover`, `source_seq`) and the command's
    /// position in the invocation's output. `source_component` is left for
    /// the coordinator, which stamps the registered component name.
    pub fn deferred_header(
        source_cover: &pb::Cover,
        source_seq: u32,
        command_index: u32,
    ) -> pb::PageHeader {
        pb::PageHeader {
            sequence_type: Some(deferred(Some(source_cover), source_seq, command_index)),
            sync_mode: None,
        }
    }

    /// Makes `cmd` a deferred command for `domain`: every page header
    /// becomes [`Self::deferred_header`]; any explicit sequence is replaced
    /// and a page's `sync_mode` is kept.
    ///
    /// # Errors
    ///
    /// INVALID_ARGUMENT `UNDECLARED_OUTPUT_DOMAIN` (with `domain`) when
    /// `domain` is not a declared output domain; `cmd` is left unchanged.
    pub fn stamp_command(
        &self,
        cmd: &mut pb::CommandBook,
        domain: &str,
        source_cover: &pb::Cover,
        source_seq: u32,
        command_index: u32,
    ) -> Result<(), CodedError> {
        if !self.has_domain(domain) {
            return Err(CodedError::invalid_argument(
                codes::UNDECLARED_OUTPUT_DOMAIN,
                messages::UNDECLARED_OUTPUT_DOMAIN,
                [(extras::DOMAIN.to_string(), domain.to_string())],
            ));
        }
        stamp_pages(cmd, Some(source_cover), source_seq, command_index);
        Ok(())
    }
}

fn deferred(
    source_cover: Option<&pb::Cover>,
    source_seq: u32,
    command_index: u32,
) -> pb::page_header::SequenceType {
    pb::page_header::SequenceType::AngzarrDeferred(pb::AngzarrDeferredSequence {
        source: source_cover.cloned(),
        source_seq,
        source_component: String::new(),
        command_index,
    })
}

/// Sets every page of `cmd` to `angzarr_deferred` provenance, replacing any
/// explicit sequence (a deferred command never carries an expected version)
/// and keeping a page's `sync_mode`.
pub(crate) fn stamp_pages(
    cmd: &mut pb::CommandBook,
    source_cover: Option<&pb::Cover>,
    source_seq: u32,
    command_index: u32,
) {
    for page in &mut cmd.pages {
        let header = page.header.get_or_insert_with(pb::PageHeader::default);
        header.sequence_type = Some(deferred(source_cover, source_seq, command_index));
    }
}

#[cfg(test)]
#[path = "destinations.test.rs"]
mod destinations_tests;
