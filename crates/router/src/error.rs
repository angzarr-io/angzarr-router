//! The coded error model: a SCREAMING_SNAKE key and an argument map, and
//! the single handler-error → gRPC mapping table (decision 14.37). No
//! English crosses a boundary: the gRPC status message IS the key, the
//! `google.rpc.ErrorInfo` (domain [`ERROR_INFO_DOMAIN`]) carries the same
//! key as its reason and the args as its metadata, and text is rendered only
//! where it is shown, from a catalog keyed by the code.

use std::collections::BTreeMap;

/// google.rpc.ErrorInfo domain for all angzarr errors.
pub const ERROR_INFO_DOMAIN: &str = "angzarr.io";

/// gRPC status codes the router emits (numeric values are the wire codes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum GrpcCode {
    Unknown = 2,
    InvalidArgument = 3,
    NotFound = 5,
    AlreadyExists = 6,
    PermissionDenied = 7,
    FailedPrecondition = 9,
    Aborted = 10,
    OutOfRange = 11,
    Unimplemented = 12,
    Internal = 13,
    Unavailable = 14,
    DataLoss = 15,
}

/// The class an error is, decided by its gRPC status alone (owner ruling,
/// 2026-10-29): a business refusal reaches the issuing policy's OnRefused;
/// a bug is alerted and dead-lettered; anything else is retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// FAILED_PRECONDITION, INVALID_ARGUMENT, NOT_FOUND, ALREADY_EXISTS,
    /// OUT_OF_RANGE, PERMISSION_DENIED.
    BusinessRefusal,
    /// INTERNAL, UNKNOWN, DATA_LOSS.
    Bug,
    /// UNAVAILABLE, ABORTED and the like.
    Retryable,
}

impl GrpcCode {
    /// The class an error with this status is (owner ruling, 2026-10-29).
    ///
    /// RED STUB (ruling 2026-10-29): the superseded "only
    /// FAILED_PRECONDITION is a business refusal".
    pub fn class(self) -> ErrorClass {
        match self {
            GrpcCode::FailedPrecondition => ErrorClass::BusinessRefusal,
            GrpcCode::Internal | GrpcCode::DataLoss => ErrorClass::Bug,
            _ => ErrorClass::Retryable,
        }
    }

    /// True when an error with this status is a business refusal.
    pub fn is_business_refusal(self) -> bool {
        self.class() == ErrorClass::BusinessRefusal
    }
}

/// The statuses a business refusal may carry, and only those: a handler's
/// [`CodedError::rejection`] takes one, so a bug's or a retryable status
/// is refused by type (owner ruling, 2026-10-29).
///
/// ```compile_fail
/// use angzarr_router::error::{CodedError, GrpcCode};
/// let _ = CodedError::rejection(GrpcCode::Internal, "BROKEN", []);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusinessStatus {
    FailedPrecondition,
    InvalidArgument,
    NotFound,
    AlreadyExists,
    OutOfRange,
    PermissionDenied,
}

impl From<BusinessStatus> for GrpcCode {
    fn from(status: BusinessStatus) -> Self {
        match status {
            BusinessStatus::FailedPrecondition => GrpcCode::FailedPrecondition,
            BusinessStatus::InvalidArgument => GrpcCode::InvalidArgument,
            BusinessStatus::NotFound => GrpcCode::NotFound,
            BusinessStatus::AlreadyExists => GrpcCode::AlreadyExists,
            BusinessStatus::OutOfRange => GrpcCode::OutOfRange,
            BusinessStatus::PermissionDenied => GrpcCode::PermissionDenied,
        }
    }
}

/// A status a host or a binding hands over at run time is checked: a
/// non-business status is refused with itself.
///
/// RED STUB (ruling 2026-10-29): refuses everything.
impl TryFrom<GrpcCode> for BusinessStatus {
    type Error = GrpcCode;
    fn try_from(code: GrpcCode) -> Result<Self, GrpcCode> {
        Err(code)
    }
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
    /// A policy's command whose cover names no root, whatever its target
    /// (decision 14.36, amended: a process manager is not differentiated;
    /// policy.proto PolicyResponse): a policy bug.
    pub const POLICY_COMMAND_NO_ROOT: &str = "POLICY_COMMAND_NO_ROOT";
    /// A policy's command carrying a correlation_id other than its
    /// trigger's (decision 14.36; policy.proto PolicyResponse): a policy bug.
    pub const POLICY_COMMAND_OTHER_CORRELATION: &str = "POLICY_COMMAND_OTHER_CORRELATION";
    /// A policy's OnRefused answer that is not a bare Fact (no cover, no
    /// identity, no Handling), or a refusal lacking what addressing it needs
    /// (policy.proto, OnRefusedResponse): a policy bug.
    pub const MALFORMED_REFUSAL_FACT: &str = "MALFORMED_REFUSAL_FACT";
    /// A command for a process manager's correlation with no instance, of a
    /// type the PM does not declare as starting (process_manager.proto;
    /// decision 14.31).
    pub const PM_NO_INSTANCE: &str = "PM_NO_INSTANCE";
    /// A refusal fact addressed to any stream but the exact one its
    /// causation_id names (types.proto, the command_identity region).
    pub const REFUSAL_FACT_WRONG_STREAM: &str = "REFUSAL_FACT_WRONG_STREAM";
    /// A component other than a process manager declaring starting types
    /// (types.proto, ComponentDescriptor.starting_types).
    pub const STARTING_TYPES_ON_NON_PM: &str = "STARTING_TYPES_ON_NON_PM";

    /// The router's complete reason inventory (decision 14.37): every key
    /// the router, or the C ABI over it, answers with. A catalog that
    /// renders the router's errors carries a message for each.
    ///
    /// RED STUB (14.37): the pre-14.37 codes, unsplit.
    pub const ALL: &[&str] = &[
        ANY_DECODE_FAILED,
        NO_HANDLER_REGISTERED,
        MISSING_COMMAND_BOOK,
        MISSING_COMMAND_PAGE,
        MISSING_COMMAND_PAYLOAD,
        PERSISTED_EVENT_CORRUPT,
        UNHANDLED_HANDLER_ERROR,
        MISSING_EVENT_BOOK_COVER,
        EMPTY_POLICY_SOURCE,
        DUPLICATE_REGISTRATION,
        NO_HANDLERS_REGISTERED,
        DUPLICATE_COMMAND_HANDLER,
        NO_FACT_HANDLER,
        CONFLICT_UNRESOLVED,
        CONFLICT_OVERLAP,
        CONFLICT_RESOLUTION_UNSUPPORTED,
        MISSING_RULES_VERSION,
        MISSING_STATE_TYPE,
        POLICY_COMMAND_OUTSIDE_DESTINATION,
        POLICY_COMMAND_OUTSIDE_EDITION,
        POLICY_COMMAND_NO_ROOT,
        POLICY_COMMAND_OTHER_CORRELATION,
        MALFORMED_REFUSAL_FACT,
        PM_NO_INSTANCE,
        REFUSAL_FACT_WRONG_STREAM,
        STARTING_TYPES_ON_NON_PM,
    ];
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
    /// A handler's bug's own code, under UNHANDLED_HANDLER_ERROR; its args
    /// follow as `cause.<arg>` (decision 14.37).
    pub const CAUSE: &str = "cause";
}

/// Where a failing fold failed: the page, when known, else the batch's
/// sequence range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldAt<'a> {
    Page { sequence: u64, type_url: &'a str },
    Range { first: u64, last: u64 },
}

/// A coded business/framework error: the form every failure takes before
/// crossing a boundary, a KEY and an ARGUMENT MAP (decision 14.37). `code`
/// is the cross-language SCREAMING_SNAKE key: the `ErrorInfo.reason` and
/// the status message alike ([`Self::status_message`]); `extras` are the
/// args, the `ErrorInfo` metadata, a wrapped cause flattened into them as
/// `cause` and `cause.<arg>`; `grpc` is resolved by the single mapping
/// table at construction. No constructor takes text.
///
/// RED (14.37): `message` is what the wire still reads as the status
/// message. No constructor fills it any more; [`map_handler_error`] still
/// puts an escaped error's text there. The green deletes the field.
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
/// error, which the boundary maps to UNHANDLED_HANDLER_ERROR with no cause.
/// `Other`'s text is for the handler's own log; it never crosses a
/// boundary (decision 14.37).
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
        extras: impl IntoIterator<Item = (String, String)>,
        grpc: GrpcCode,
    ) -> Self {
        CodedError {
            code: code.to_string(),
            message: String::new(),
            extras: extras.into_iter().collect(),
            grpc,
            refusal: None,
        }
    }

    /// The gRPC status message: the reason key itself (decision 14.37).
    ///
    /// RED STUB (14.37): still the legacy `message` field.
    pub fn status_message(&self) -> &str {
        &self.message
    }

    /// A coded client error. The gRPC code is resolved from the single
    /// mapping table: a request this router hosts no component for →
    /// UNIMPLEMENTED (an unknown type URL is [`Self::unknown_type`]
    /// instead), PERSISTED_EVENT_CORRUPT → DATA_LOSS (store-sourced,
    /// unrecoverable by retry), everything else → INVALID_ARGUMENT.
    pub fn invalid_argument(
        code: &str,
        extras: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        let grpc = match code {
            codes::NO_HANDLER_REGISTERED => GrpcCode::Unimplemented,
            codes::PERSISTED_EVENT_CORRUPT => GrpcCode::DataLoss,
            _ => GrpcCode::InvalidArgument,
        };
        Self::coded(code, extras, grpc)
    }

    /// A command or fact whose type URL reaches no registered handler
    /// (`code` UNKNOWN_COMMAND_TYPE or NO_FACT_HANDLER): INVALID_ARGUMENT,
    /// its args carrying the type URL as sent as `type_url` and the type the
    /// upcaster chain ended at as `upcast_type_url` (upcaster.proto, VC-1:
    /// the sent one when no upcaster applies).
    pub fn unknown_type(
        code: &str,
        type_url: &str,
        upcast_type_url: &str,
        extras: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self::coded(
            code,
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

    /// Bytes that do not decode as the message `message_type` (its
    /// fully-qualified name): ANY_DECODE_FAILED (INVALID_ARGUMENT), one
    /// condition whatever the message, `message_type` its arg.
    ///
    /// RED STUB (14.37): drops `message_type`.
    pub fn undecodable(message_type: &str) -> Self {
        let _ = message_type;
        Self::invalid_argument(codes::ANY_DECODE_FAILED, [])
    }

    /// A stale command (ABORTED, as STRICT would answer it): its conflict
    /// window was not resolved, or COMMUTATIVE found an overlap.
    pub fn stale(code: &str, extras: impl IntoIterator<Item = (String, String)>) -> Self {
        Self::coded(code, extras, GrpcCode::Aborted)
    }

    /// Classifies a store-sourced decode failure (DATA_LOSS on the wire).
    pub fn persisted_corrupt(type_url: &str) -> Self {
        Self::invalid_argument(
            codes::PERSISTED_EVENT_CORRUPT,
            [(extras::TYPE_URL.to_string(), type_url.to_string())],
        )
    }

    /// A fold of events that failed (INTERNAL), under `code`: FOLD_FAILED
    /// for persisted events, FOLD_FAILED_EMITTED for the events the command
    /// emitted. Those events were accepted (or emitted) by the same apply
    /// logic, so the failure is a handler regression, never corrupt data.
    /// `at` names the failing page or, when it is unknown, the batch's
    /// range; `cause`, the fold's own failure as a key and args, is
    /// flattened in as `cause` and `cause.<arg>`.
    ///
    /// RED STUB (14.37): `cause` is dropped.
    pub fn fold_failed(
        code: &str,
        at: FoldAt<'_>,
        state_type_url: &str,
        cause: Option<&angzarr_proto::v1::Diagnostic>,
    ) -> Self {
        let _ = cause;
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
        Self::coded(code, details, GrpcCode::Internal)
    }

    /// An unclassified error that escaped a business handler (INTERNAL):
    /// UNHANDLED_HANDLER_ERROR with no cause. What escaped stays in the
    /// handler's own log.
    pub fn unhandled() -> Self {
        Self::coded(codes::UNHANDLED_HANDLER_ERROR, [], GrpcCode::Internal)
    }

    /// A business handler that panicked (INTERNAL): UNHANDLED_HANDLER_ERROR
    /// with cause HANDLER_PANICKED. The panic's message stays in the
    /// handler's own log.
    ///
    /// RED STUB (14.37): no cause.
    pub fn panicked() -> Self {
        Self::unhandled()
    }

    /// A business rejection under `status`, one of the business set: the
    /// command is refused, a business "no", never a fault. One of the two
    /// classes a handler fails with (the other is [`Self::bug`]). `code` is
    /// its key and `details` its args; a typed refusal is
    /// [`Self::refused`] instead. The refusal reaches the issuing policy's
    /// OnRefused ([`Self::refusal_response`]).
    ///
    /// RED STUB (ruling 2026-10-29): `status` is ignored, always
    /// FAILED_PRECONDITION.
    pub fn rejection(
        status: BusinessStatus,
        code: &str,
        details: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        let _ = status;
        Self::coded(code, details, GrpcCode::FailedPrecondition)
    }

    /// A typed business refusal (FAILED_PRECONDITION): `refusal` is one of
    /// the command's `refuses` types, packed. Its wire reason is
    /// COMMAND_REFUSED, its `refusal_type` arg the object's fully-qualified
    /// type, and the object itself rides the status details (owner's
    /// ruling on decision 14.37).
    ///
    /// RED STUB (14.37): no key, no `refusal_type`.
    pub fn refused(refusal: prost_types::Any) -> Self {
        Self::rejection(BusinessStatus::FailedPrecondition, "", []).with_refusal(refusal)
    }

    /// A bug: a defect, never a business answer. The other of the two
    /// classes a handler fails with (the first is [`Self::rejection`]). It
    /// goes out as UNHANDLED_HANDLER_ERROR (INTERNAL), with `code` and
    /// `details` as its cause, flattened as `cause=<code>` and
    /// `cause.<arg>=<value>` (decision 14.37); the runtime alerts it and
    /// dead-letters the input, and it never reaches a policy's OnRefused.
    pub fn bug(code: &str, details: impl IntoIterator<Item = (String, String)>) -> Self {
        let cause = std::iter::once((extras::CAUSE.to_string(), code.to_string())).chain(
            details
                .into_iter()
                .map(|(name, value)| (format!("{}.{name}", extras::CAUSE), value)),
        );
        Self::coded(codes::UNHANDLED_HANDLER_ERROR, cause, GrpcCode::Internal)
    }

    /// A component bug the router itself finds (INTERNAL), under its own
    /// code: a policy's malformed answer, say. Never a handler's.
    pub(crate) fn internal(code: &str, extras: impl IntoIterator<Item = (String, String)>) -> Self {
        Self::coded(code, extras, GrpcCode::Internal)
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
    /// RefusalResponse): `Some` for a business refusal only (its status in
    /// the business set, [`GrpcCode::is_business_refusal`]), carrying the typed refusal object, or,
    /// for an untyped rejection, its key and args as a `Diagnostic`; `None`
    /// for every other failure, which never reaches a policy's OnRefused
    /// (retried, or dead-lettered as a bug).
    pub fn refusal_response(&self) -> Option<angzarr_proto::v1::RefusalResponse> {
        use angzarr_proto::v1::refusal_response::Reason;
        self.grpc
            .is_business_refusal()
            .then(|| angzarr_proto::v1::RefusalResponse {
                reason: Some(match self.refusal.as_deref() {
                    Some(refusal) => Reason::Refusal(refusal.clone()),
                    None => Reason::Diagnostic(angzarr_proto::v1::Diagnostic {
                        code: self.code.clone(),
                        args: self.extras.clone().into_iter().collect(),
                    }),
                }),
            })
    }
}

/// In process too, an error shows as its code then its args, ` name=value`
/// each, a wrapped cause flattened as on the wire; never English (decision
/// 14.37, owner's clarification of 2026-10-29).
///
/// RED STUB (14.37): the legacy `message`.
impl std::fmt::Display for CodedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The single handler-error mapping: coded errors pass through carrying
/// their resolved gRPC code; unclassified errors become
/// UNHANDLED_HANDLER_ERROR (INTERNAL) with no cause, their text left to the
/// handler's own log.
///
/// RED STUB (14.37): an escaped error's text still rides `message`.
pub fn map_handler_error(err: HandlerError) -> CodedError {
    match err {
        HandlerError::Coded(coded) => coded,
        HandlerError::Other(message) => CodedError {
            message,
            ..CodedError::unhandled()
        },
    }
}

#[cfg(test)]
#[path = "error.test.rs"]
mod error_tests;
