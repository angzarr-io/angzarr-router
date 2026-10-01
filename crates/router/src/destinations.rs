//! Destinations — the output domains a saga or process manager declares
//! (its command targets). Emitted commands are deferred: they carry no
//! destination sequence, and the router stamps their `angzarr_deferred`
//! provenance (see [`crate::stamp_deferred`]), so a component never needs
//! destination state or sequences.

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
    pub fn has(&self, domain: &str) -> bool {
        self.domains.iter().any(|d| d == domain)
    }

    /// The declared output domains, in declaration order.
    pub fn domains(&self) -> &[String] {
        &self.domains
    }
}

#[cfg(test)]
#[path = "destinations.test.rs"]
mod destinations_tests;
