//! The coded error model — SCREAMING_SNAKE codes, static cross-language
//! messages, and the single handler-error → gRPC mapping table. Codes and
//! the framework's own message text are byte-identical across language
//! clients; assertions key off codes, never message substrings.

use std::collections::BTreeMap;

/// google.rpc.ErrorInfo domain for all angzarr errors.
pub const ERROR_INFO_DOMAIN: &str = "angzarr.io";

/// gRPC status codes the router emits (numeric values are the wire codes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum GrpcCode {
    InvalidArgument = 3,
    NotFound = 5,
    FailedPrecondition = 9,
    Aborted = 10,
    Unimplemented = 12,
    Internal = 13,
    DataLoss = 15,
}

/// Cross-language error codes (the slice's subset of the 47-code inventory).
pub mod codes {
    pub const ANY_DECODE_FAILED: &str = "ANY_DECODE_FAILED";
    pub const NO_HANDLER_REGISTERED: &str = "NO_HANDLER_REGISTERED";
    pub const MISSING_COMMAND_BOOK: &str = "MISSING_COMMAND_BOOK";
    pub const MISSING_COMMAND_PAGE: &str = "MISSING_COMMAND_PAGE";
    pub const MISSING_COMMAND_PAYLOAD: &str = "MISSING_COMMAND_PAYLOAD";
    pub const PERSISTED_EVENT_CORRUPT: &str = "PERSISTED_EVENT_CORRUPT";
    pub const UNHANDLED_HANDLER_ERROR: &str = "UNHANDLED_HANDLER_ERROR";
    pub const MISSING_EVENT_BOOK_COVER: &str = "MISSING_EVENT_BOOK_COVER";
    pub const EMPTY_POLICY_SOURCE: &str = "EMPTY_POLICY_SOURCE";
    pub const DUPLICATE_REGISTRATION: &str = "DUPLICATE_REGISTRATION";
    pub const NO_HANDLERS_REGISTERED: &str = "NO_HANDLERS_REGISTERED";
    pub const DUPLICATE_COMMAND_HANDLER: &str = "DUPLICATE_COMMAND_HANDLER";
    pub const NO_FACT_HANDLER: &str = "NO_FACT_HANDLER";
    pub const CONFLICT_UNRESOLVED: &str = "CONFLICT_UNRESOLVED";
    pub const CONFLICT_OVERLAP: &str = "CONFLICT_OVERLAP";
    pub const CONFLICT_RESOLUTION_UNSUPPORTED: &str = "CONFLICT_RESOLUTION_UNSUPPORTED";
    pub const MISSING_RULES_VERSION: &str = "MISSING_RULES_VERSION";
    pub const MISSING_STATE_TYPE: &str = "MISSING_STATE_TYPE";
    pub const POLICY_COMMAND_OUTSIDE_DESTINATION: &str = "POLICY_COMMAND_OUTSIDE_DESTINATION";
    /// A policy command naming an edition other than its trigger's (owner
    /// ruling, 2026-10-28; types.proto Cover.edition: a policy stays in the
    /// edition of the event it handles).
    pub const POLICY_COMMAND_OUTSIDE_EDITION: &str = "POLICY_COMMAND_OUTSIDE_EDITION";
    /// A policy's OnRefused answer that is not a bare Fact (no cover, no
    /// identity, no Handling), or a refusal lacking what addressing it needs
    /// (policy.proto, OnRefusedResponse): a policy bug.
    pub const MALFORMED_REFUSAL_FACT: &str = "MALFORMED_REFUSAL_FACT";
    /// An input from outside (not a policy's) addressed in an edition to a
    /// stream with no fork there (types.proto, Cover.edition; decision 14.32).
    pub const EDITION_NOT_FORKED: &str = "EDITION_NOT_FORKED";
    /// A command for a process manager's correlation with no instance, of a
    /// type the PM does not declare as starting (process_manager.proto;
    /// decision 14.31).
    pub const PM_NO_INSTANCE: &str = "PM_NO_INSTANCE";
    /// An input addressed to a process manager whose set correlation_id
    /// differs from its cover's root (process_manager.proto, Stream).
    pub const PM_CORRELATION_NOT_ROOT: &str = "PM_CORRELATION_NOT_ROOT";
    /// A refusal fact addressed to any stream but the exact one its
    /// causation_id names (types.proto, the command_identity region).
    pub const REFUSAL_FACT_WRONG_STREAM: &str = "REFUSAL_FACT_WRONG_STREAM";
    /// A component other than a process manager declaring starting types
    /// (types.proto, ComponentDescriptor.starting_types).
    pub const STARTING_TYPES_ON_NON_PM: &str = "STARTING_TYPES_ON_NON_PM";
}

/// Canonical static message text (byte-equal across languages).
pub mod messages {
    pub const UNKNOWN_COMMAND: &str = "Unknown command type";
    pub const NO_COMMAND_PAGES: &str = "No command pages";
    pub const PERSISTED_EVENT_CORRUPT: &str = "persisted event payload corrupt";
    pub const MISSING_EVENT_BOOK_COVER: &str = "missing event book cover";
    pub const EMPTY_POLICY_SOURCE: &str = "empty policy source";
    pub const NO_HANDLERS_REGISTERED: &str = "no handlers registered";
    pub const DUPLICATE_COMMAND_HANDLER: &str =
        "an old type the aggregate's upcaster chain accepts is also one of its command types";
    pub const NO_COMPONENT_OF_KIND: &str = "the router hosts no component of this kind";
    pub const CONFLICT_UNRESOLVED: &str =
        "the handler did not resolve the conflict window with the delivered nonce";
    pub const CONFLICT_OVERLAP: &str = "the command's changes overlap the changes it did not see";
    pub const CONFLICT_RESOLUTION_UNSUPPORTED: &str =
        "the component does not support COMMUTATIVE conflict resolution";
    pub const MISSING_RULES_VERSION: &str = "a component registers with its rules version";
    pub const MISSING_STATE_TYPE: &str =
        "a stateful component registers with its state type (Rebuilder::of_message)";
    pub const POLICY_COMMAND_OUTSIDE_DESTINATION: &str =
        "a policy answered a command for a domain other than its one destination";
    pub const FOLD_FAILED: &str = "the state fold failed on an event it had accepted";
    pub const FOLD_FAILED_EMITTED: &str = "the state fold failed on an event this command emitted";
    pub const POLICY_COMMAND_OUTSIDE_EDITION: &str =
        "a policy answered a command for an edition other than its trigger's";
    pub const MALFORMED_REFUSAL_FACT: &str =
        "a policy's refusal fact must be a bare Fact, addressed and identified by its runtime";
    pub const EDITION_NOT_FORKED: &str =
        "the stream has no fork in the edition; fork it, or let a policy's command reach it";
    pub const PM_NO_INSTANCE: &str =
        "no process manager instance for the correlation, and the command does not start one";
    pub const PM_CORRELATION_NOT_ROOT: &str =
        "an input to a process manager sets a correlation_id other than its cover's root";
    pub const REFUSAL_FACT_WRONG_STREAM: &str =
        "a refusal fact is accepted only by the exact stream its causation_id names";
    pub const STARTING_TYPES_ON_NON_PM: &str =
        "only a process manager declares starting input types";
}

/// Cross-language detail-key constants.
pub mod extras {
    pub const DOMAIN: &str = "domain";
    pub const TYPE_URL: &str = "type_url";
    pub const COMMAND_TYPE: &str = "command_type";
    pub const UPCAST_TYPE_URL: &str = "upcast_type_url";
    pub const WINDOW_UNITS: &str = "window_units";
    pub const COMMAND_UNITS: &str = "command_units";
    pub const SEQUENCE: &str = "sequence";
    pub const STATE_TYPE_URL: &str = "state_type_url";
    pub const COMPONENT: &str = "component";
    pub const COMPONENT_KIND: &str = "component_kind";
    pub const EXPECTED_DOMAIN: &str = "expected_domain";
    pub const ACTUAL_DOMAIN: &str = "actual_domain";
    pub const EXPECTED_EDITION: &str = "expected_edition";
    pub const ACTUAL_EDITION: &str = "actual_edition";
    pub const REASON: &str = "reason";
    pub const FIRST_SEQUENCE: &str = "first_sequence";
    pub const LAST_SEQUENCE: &str = "last_sequence";
    pub const FOLD_MESSAGE: &str = "fold_message";
}

/// Where a failing fold failed: the page, when known, else the batch's
/// sequence range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldAt<'a> {
    Page { sequence: u64, type_url: &'a str },
    Range { first: u64, last: u64 },
}

/// A coded business/framework error: the form every failure takes before
/// crossing a boundary. `code` is the cross-language SCREAMING_SNAKE
/// identifier (travels as ErrorInfo.reason); `message` is human-readable
/// text (travels as Status.message): the framework's own codes carry the
/// static [`messages`] text, while a business rejection carries the
/// handler's message and UNHANDLED_HANDLER_ERROR carries the escaped
/// error's (or panic's) text; `grpc` is resolved by the single mapping table
/// at construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodedError {
    pub code: String,
    pub message: String,
    pub extras: BTreeMap<String, String>,
    pub grpc: GrpcCode,
    /// A business rejection's TYPED refusal object, packed: one of the
    /// command's `CommandOptions.refuses` types (options.proto; decision
    /// 14.31). Carried in the rejection's status details; the refused
    /// policy receives it as `RefusalResponse.refusal`. Boxed, so the
    /// common error stays small.
    pub refusal: Option<Box<prost_types::Any>>,
}

/// What a business handler may fail with: a coded error (rejections and
/// classified failures keep their code and gRPC code) or an unclassified
/// error, which the boundary maps to UNHANDLED_HANDLER_ERROR.
#[derive(Debug)]
pub enum HandlerError {
    Coded(CodedError),
    Other(String),
}

impl From<CodedError> for HandlerError {
    fn from(e: CodedError) -> Self {
        HandlerError::Coded(e)
    }
}

impl CodedError {
    fn coded(
        code: &str,
        message: &str,
        extras: impl IntoIterator<Item = (String, String)>,
        grpc: GrpcCode,
    ) -> Self {
        CodedError {
            code: code.to_string(),
            message: message.to_string(),
            extras: extras.into_iter().collect(),
            grpc,
            refusal: None,
        }
    }

    /// A coded client error. The gRPC code is resolved from the single
    /// mapping table: NO_HANDLER_REGISTERED → UNIMPLEMENTED (a request this
    /// router hosts no component for; an unknown type URL is
    /// [`Self::unknown_type`] instead), PERSISTED_EVENT_CORRUPT → DATA_LOSS
    /// (store-sourced, unrecoverable by retry), everything else →
    /// INVALID_ARGUMENT.
    pub fn invalid_argument(
        code: &str,
        message: &str,
        extras: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        let grpc = match code {
            codes::NO_HANDLER_REGISTERED => GrpcCode::Unimplemented,
            codes::PERSISTED_EVENT_CORRUPT => GrpcCode::DataLoss,
            _ => GrpcCode::InvalidArgument,
        };
        Self::coded(code, message, extras, grpc)
    }

    /// A command or fact whose type URL reaches no registered handler
    /// (`code` NO_HANDLER_REGISTERED or NO_FACT_HANDLER): INVALID_ARGUMENT,
    /// the message naming the type URL as sent (command_handler.proto,
    /// CommandHandlerService.Handle step 2; decision 12.11; the one framework
    /// message that is not static), the extras — rendered as the status's
    /// google.rpc.ErrorInfo metadata — carrying it as `type_url` and the type
    /// the upcaster chain ended at as `upcast_type_url` (upcaster.proto, VC-1:
    /// the sent one when no upcaster applies).
    pub fn unknown_type(
        code: &str,
        type_url: &str,
        upcast_type_url: &str,
        extras: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self::coded(
            code,
            &format!("no handler for type URL {type_url}"),
            [
                (extras::TYPE_URL.to_string(), type_url.to_string()),
                (
                    extras::UPCAST_TYPE_URL.to_string(),
                    upcast_type_url.to_string(),
                ),
            ]
            .into_iter()
            .chain(extras),
            GrpcCode::InvalidArgument,
        )
    }

    /// A stale command (ABORTED, as STRICT would answer it): its conflict
    /// window was not resolved, or COMMUTATIVE found an overlap.
    pub fn stale(
        code: &str,
        message: &str,
        extras: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self::coded(code, message, extras, GrpcCode::Aborted)
    }

    /// Classifies a store-sourced decode failure (DATA_LOSS on the wire).
    pub fn persisted_corrupt(type_url: &str) -> Self {
        Self::invalid_argument(
            codes::PERSISTED_EVENT_CORRUPT,
            messages::PERSISTED_EVENT_CORRUPT,
            [(extras::TYPE_URL.to_string(), type_url.to_string())],
        )
    }

    /// A fold of events that failed (INTERNAL): those events were accepted
    /// by the same apply logic (or, for [`messages::FOLD_FAILED_EMITTED`],
    /// emitted by the command), so the failure is a handler regression,
    /// never corrupt data. `message` says which; `at` names the failing page
    /// or, when it is unknown, the batch's range; `fold_message` is the
    /// fold's own text (omitted when empty).
    pub fn fold_failed(
        message: &str,
        at: FoldAt<'_>,
        state_type_url: &str,
        fold_message: &str,
    ) -> Self {
        let mut details = match at {
            FoldAt::Page { sequence, type_url } => vec![
                (extras::SEQUENCE.to_string(), sequence.to_string()),
                (extras::TYPE_URL.to_string(), type_url.to_string()),
            ],
            FoldAt::Range { first, last } => vec![
                (extras::FIRST_SEQUENCE.to_string(), first.to_string()),
                (extras::LAST_SEQUENCE.to_string(), last.to_string()),
            ],
        };
        details.push((
            extras::STATE_TYPE_URL.to_string(),
            state_type_url.to_string(),
        ));
        if !fold_message.is_empty() {
            details.push((extras::FOLD_MESSAGE.to_string(), fold_message.to_string()));
        }
        Self::coded(
            codes::UNHANDLED_HANDLER_ERROR,
            message,
            details,
            GrpcCode::Internal,
        )
    }

    /// A component bug the client library refuses before it answers
    /// (INTERNAL).
    pub fn internal(
        code: &str,
        message: &str,
        extras: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self::coded(code, message, extras, GrpcCode::Internal)
    }

    /// An unclassified error that escaped a business handler (INTERNAL).
    pub fn unhandled(message: &str) -> Self {
        Self::coded(
            codes::UNHANDLED_HANDLER_ERROR,
            message,
            [],
            GrpcCode::Internal,
        )
    }

    /// Business rejection: retry after refetching state.
    pub fn rejection_precondition_failed(
        code: &str,
        message: &str,
        details: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self::coded(code, message, details, GrpcCode::FailedPrecondition)
    }

    /// Business rejection: bad input, don't retry.
    pub fn rejection_invalid_argument(
        code: &str,
        message: &str,
        details: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self::coded(code, message, details, GrpcCode::InvalidArgument)
    }

    /// Business rejection: referenced entity absent.
    pub fn rejection_not_found(
        code: &str,
        message: &str,
        details: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self::coded(code, message, details, GrpcCode::NotFound)
    }
}

impl CodedError {
    /// This business rejection carrying its typed refusal object (one of
    /// the command's `refuses` types, packed).
    pub fn with_refusal(self, refusal: prost_types::Any) -> Self {
        CodedError {
            refusal: Some(Box::new(refusal)),
            ..self
        }
    }

    /// The refused policy's view of this answer (types.proto,
    /// RefusalResponse): `Some` for a business rejection
    /// (FAILED_PRECONDITION) only, carrying the typed refusal object and the
    /// status message; `None` for every other failure, which never reaches
    /// a policy's OnRefused (retried, or dead-lettered as a bug).
    pub fn refusal_response(&self) -> Option<angzarr_proto::v1::RefusalResponse> {
        (self.grpc == GrpcCode::FailedPrecondition).then(|| angzarr_proto::v1::RefusalResponse {
            refusal: self.refusal.as_deref().cloned(),
            message: self.message.clone(),
        })
    }
}

/// The single handler-error mapping: coded errors pass through carrying
/// their resolved gRPC code; unclassified errors become
/// UNHANDLED_HANDLER_ERROR (INTERNAL).
pub fn map_handler_error(err: HandlerError) -> CodedError {
    match err {
        HandlerError::Coded(coded) => coded,
        HandlerError::Other(message) => CodedError::unhandled(&message),
    }
}

#[cfg(test)]
#[path = "error.test.rs"]
mod error_tests;
