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
    Unimplemented = 12,
    Internal = 13,
    DataLoss = 15,
}

/// Cross-language error codes (the slice's subset of the 47-code inventory).
pub mod codes {
    pub const ANY_DECODE_FAILED: &str = "ANY_DECODE_FAILED";
    pub const VALUE_NOT_POSITIVE: &str = "VALUE_NOT_POSITIVE";
    pub const NO_HANDLER_REGISTERED: &str = "NO_HANDLER_REGISTERED";
    pub const MISSING_COMMAND_BOOK: &str = "MISSING_COMMAND_BOOK";
    pub const MISSING_COMMAND_PAGE: &str = "MISSING_COMMAND_PAGE";
    pub const MISSING_COMMAND_PAYLOAD: &str = "MISSING_COMMAND_PAYLOAD";
    pub const NOTIFICATION_DECODE_FAILED: &str = "NOTIFICATION_DECODE_FAILED";
    pub const REJECTION_NOTIFICATION_DECODE_FAILED: &str = "REJECTION_NOTIFICATION_DECODE_FAILED";
    pub const PERSISTED_EVENT_CORRUPT: &str = "PERSISTED_EVENT_CORRUPT";
    pub const UNHANDLED_HANDLER_ERROR: &str = "UNHANDLED_HANDLER_ERROR";
    pub const MISSING_EVENT_BOOK_COVER: &str = "MISSING_EVENT_BOOK_COVER";
    pub const MISSING_SAGA_SOURCE: &str = "MISSING_SAGA_SOURCE";
    pub const EMPTY_SAGA_SOURCE: &str = "EMPTY_SAGA_SOURCE";
    pub const MISSING_PM_TRIGGER: &str = "MISSING_PM_TRIGGER";
    pub const DUPLICATE_REGISTRATION: &str = "DUPLICATE_REGISTRATION";
    pub const AMBIGUOUS_COMPENSATION: &str = "AMBIGUOUS_COMPENSATION";
    pub const NO_HANDLERS_REGISTERED: &str = "NO_HANDLERS_REGISTERED";
    pub const MIXED_HANDLER_KINDS: &str = "MIXED_HANDLER_KINDS";
    pub const DUPLICATE_COMMAND_HANDLER: &str = "DUPLICATE_COMMAND_HANDLER";
    pub const NO_UNDO_HANDLER: &str = "NO_UNDO_HANDLER";
    pub const UNKNOWN_NOTIFICATION_PAYLOAD: &str = "UNKNOWN_NOTIFICATION_PAYLOAD";
    pub const COMPENSATE_DECODE_FAILED: &str = "COMPENSATE_DECODE_FAILED";
    pub const EMPTY_PM_TRIGGER: &str = "EMPTY_PM_TRIGGER";
    pub const MISSING_PM_EVENT_PAYLOAD: &str = "MISSING_PM_EVENT_PAYLOAD";
    pub const UNDECLARED_OUTPUT_DOMAIN: &str = "UNDECLARED_OUTPUT_DOMAIN";
    pub const NO_FACT_HANDLER: &str = "NO_FACT_HANDLER";
    pub const SAGA_COMPENSATES: &str = "SAGA_COMPENSATES";
}

/// Canonical static message text (byte-equal across languages).
pub mod messages {
    pub const UNKNOWN_COMMAND: &str = "Unknown command type";
    pub const NO_COMMAND_PAGES: &str = "No command pages";
    pub const NOTIFICATION_DECODE_FAILED: &str = "failed to decode Notification payload";
    pub const REJECTION_NOTIFICATION_DECODE_FAILED: &str =
        "failed to decode RejectionNotification payload";
    pub const PERSISTED_EVENT_CORRUPT: &str = "persisted event payload corrupt";
    pub const MISSING_EVENT_BOOK_COVER: &str = "missing event book cover";
    pub const MISSING_SAGA_SOURCE: &str = "missing saga source";
    pub const EMPTY_SAGA_SOURCE: &str = "empty saga source";
    pub const MISSING_PM_TRIGGER: &str = "missing PM trigger";
    pub const EMPTY_PM_TRIGGER: &str = "empty PM trigger";
    pub const MISSING_PM_EVENT_PAYLOAD: &str = "missing event payload on PM trigger";
    pub const AMBIGUOUS_COMPENSATION: &str =
        "a command type is listed both unqualified and domain-qualified";
    pub const NO_HANDLERS_REGISTERED: &str = "no handlers registered";
    pub const MIXED_HANDLER_KINDS: &str = "a router hosts components of one kind";
    pub const DUPLICATE_COMMAND_HANDLER: &str =
        "two command handlers claim the same (domain, command type)";
    pub const NO_COMPONENT_OF_KIND: &str = "the router hosts no component of this kind";
    pub const NO_UNDO_HANDLER: &str = "no undo handler for the compensated command type";
    pub const UNKNOWN_NOTIFICATION_PAYLOAD: &str =
        "notification payload is neither a RejectionNotification nor a Compensate";
    pub const COMPENSATE_DECODE_FAILED: &str = "failed to decode Compensate payload";
    pub const UNDECLARED_OUTPUT_DOMAIN: &str = "domain is not a declared output domain";
    pub const REPLAY_UNSUPPORTED: &str = "the component does not support Replay";
    pub const NO_FACT_HANDLER: &str = "no fact handler for the fact type";
    pub const SAGA_COMPENSATES: &str =
        "a saga never receives rejections, so it cannot declare compensation handlers";
}

/// Cross-language detail-key constants.
pub mod extras {
    pub const DOMAIN: &str = "domain";
    pub const TYPE_URL: &str = "type_url";
    pub const COMMAND_TYPE: &str = "command_type";
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
        }
    }

    /// A coded client error. The gRPC code is resolved from the single
    /// mapping table: NO_HANDLER_REGISTERED and NO_UNDO_HANDLER →
    /// UNIMPLEMENTED (misrouted / stale deployment; the coordinator
    /// dead-letters an undeliverable Compensate on UNIMPLEMENTED),
    /// PERSISTED_EVENT_CORRUPT → DATA_LOSS
    /// (store-sourced, unrecoverable by retry), everything else →
    /// INVALID_ARGUMENT.
    pub fn invalid_argument(
        code: &str,
        message: &str,
        extras: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        let grpc = match code {
            codes::NO_HANDLER_REGISTERED | codes::NO_UNDO_HANDLER => GrpcCode::Unimplemented,
            codes::PERSISTED_EVENT_CORRUPT => GrpcCode::DataLoss,
            _ => GrpcCode::InvalidArgument,
        };
        Self::coded(code, message, extras, grpc)
    }

    /// Classifies a store-sourced decode failure (DATA_LOSS on the wire).
    pub fn persisted_corrupt(type_url: &str) -> Self {
        Self::invalid_argument(
            codes::PERSISTED_EVENT_CORRUPT,
            messages::PERSISTED_EVENT_CORRUPT,
            [(extras::TYPE_URL.to_string(), type_url.to_string())],
        )
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
