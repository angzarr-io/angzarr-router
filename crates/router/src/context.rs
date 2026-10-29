//! The context every handler is handed beside its input and rebuilt state
//! ([`CommandContext`]), the conflict it acknowledges under
//! AGGREGATE_HANDLES ([`Conflict`]), and the context a `ContextualCommand`
//! hands its handler ([`command_context`]), which a runtime passes on with a
//! refusal to the refused policy's translating callback. Kernel: every
//! component dispatcher reads it.

use std::sync::{Arc, Mutex};

use crate::pb;

/// Per-dispatch facts the business method may need beyond its typed
/// command and rebuilt state.
#[derive(Debug, Clone, Default)]
pub struct CommandContext {
    /// The aggregate's next event sequence, derived from the prior-events
    /// book the coordinator supplied.
    pub next_sequence: u64,
    /// The "does this aggregate exist" signal — true when the prior-events
    /// book carried any history (pages or snapshot). Exposed because state
    /// factories produce non-default zero states, so business code cannot
    /// infer existence from state.
    pub had_prior_events: bool,
    /// The cover of the command being handled: the aggregate's own domain
    /// and root.
    pub cover: Option<pb::Cover>,
    /// The resolved `Handling.expected_sequence`: the head the command was
    /// issued against (stamped by the coordinator when the caller sent none).
    pub expected_sequence: u64,
    /// The resolved `Handling.conflict_resolution`.
    pub conflict_resolution: pb::ConflictResolution,
    /// The conflict to resolve: `Some` only under AGGREGATE_HANDLES. Under
    /// STRICT there is no window, and under COMMUTATIVE the router resolves
    /// the window itself, so the handler never sees the nonce.
    pub conflict: Option<Conflict>,
    /// The input's command id (types.proto, CommandPage.command_id): a
    /// client's, a policy's derived one, or a fact's derived from its
    /// origin; as delivered.
    pub command_id: Option<pb::Uuid>,
    /// The input's correlation (CommandPage.correlation_id), as delivered.
    pub correlation_id: Option<pb::Uuid>,
    /// A fact's origin (CommandPage.fact_origin); `None` for a command.
    pub fact_origin: Option<pb::FactOrigin>,
}

/// A stale command's conflict under AGGREGATE_HANDLES (types.proto,
/// ContextualCommand.conflict_nonce; backlog NC-1): the window of events the
/// command did not see, and the nonce the handler must hand back through
/// [`Conflict::resolve_conflicts`] before returning. A handler that does not
/// (or hands back another value) fails the command stale: its output is
/// discarded and the command answered CONFLICT_UNRESOLVED (ABORTED).
#[derive(Debug, Clone, Default)]
pub struct Conflict {
    window: Vec<pb::EventPage>,
    nonce: Vec<u8>,
    resolved: Arc<Mutex<Option<Vec<u8>>>>,
}

impl Conflict {
    /// A conflict over `window` with the delivered `nonce`.
    pub fn new(window: Vec<pb::EventPage>, nonce: Vec<u8>) -> Self {
        Conflict {
            window,
            nonce,
            resolved: Arc::default(),
        }
    }

    /// The events the command did not see: the pages at sequences
    /// `expected_sequence..head`, in order.
    pub fn window(&self) -> &[pb::EventPage] {
        &self.window
    }

    /// The nonce delivered with the command.
    pub fn nonce(&self) -> &[u8] {
        &self.nonce
    }

    /// The handler's declaration that it examined the window (the spec's
    /// `resolve_conflicts(window, nonce)`). Only `nonce` is checked: it must
    /// be exactly the delivered value.
    pub fn resolve_conflicts(&self, window: &[pb::EventPage], nonce: &[u8]) {
        let _ = window;
        *self.resolved.lock().unwrap_or_else(|e| e.into_inner()) = Some(nonce.to_vec());
    }

    /// True when the handler resolved with exactly the delivered nonce.
    pub fn is_resolved(&self) -> bool {
        self.resolved
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_deref()
            == Some(self.nonce.as_slice())
    }
}

/// The context a handler of `req` is handed, from the `ContextualCommand`
/// alone, whole: the next sequence and whether the stream has history
/// (pages or a snapshot) from its prior-events book; its cover, resolved
/// Handling and identity as delivered; and, under AGGREGATE_HANDLES only,
/// the conflict (the window from the expected sequence on, and the
/// delivered nonce). What a runtime passes on with a refusal of `req` to
/// its policy's translating callback
/// ([`crate::policy::PolicyDispatch::dispatch_refusal`]).
pub fn command_context(req: &pb::ContextualCommand) -> CommandContext {
    let events = req.events.as_ref();
    let next_sequence = crate::next_sequence(events);
    let book = req.command.as_ref();
    let handling = book.and_then(|b| b.handling).unwrap_or_default();
    let page = book.and_then(|b| b.page.as_ref());
    let expected_sequence = handling.expected_sequence.unwrap_or(next_sequence);
    let conflict_resolution = pb::ConflictResolution::try_from(handling.conflict_resolution)
        .unwrap_or(pb::ConflictResolution::Unspecified);
    let conflict = (conflict_resolution == pb::ConflictResolution::AggregateHandles).then(|| {
        Conflict::new(
            window_pages(events, expected_sequence),
            req.conflict_nonce.to_vec(),
        )
    });
    CommandContext {
        next_sequence,
        had_prior_events: events.is_some_and(|b| !b.pages.is_empty() || b.snapshot.is_some()),
        cover: book.and_then(|b| b.cover.clone()),
        expected_sequence,
        conflict_resolution,
        conflict,
        command_id: page.and_then(|p| p.command_id.clone()),
        correlation_id: page.and_then(|p| p.correlation_id.clone()),
        fact_origin: page.and_then(|p| p.fact_origin.clone()),
    }
}

/// The window under AGGREGATE_HANDLES: the prior-events pages at sequences
/// `expected..`.
pub(crate) fn window_pages(events: Option<&pb::EventBook>, expected: u64) -> Vec<pb::EventPage> {
    events
        .map(|b| {
            b.pages
                .iter()
                .filter(|p| p.sequence >= expected)
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}
