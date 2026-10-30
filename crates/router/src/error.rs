//! The coded error model: a SCREAMING_SNAKE key and an argument map, and
//! the single handler-error → gRPC mapping table (decision 14.37). No
//! English crosses a boundary: the gRPC status message IS the key, the
//! `google.rpc.ErrorInfo` (domain [`ERROR_INFO_DOMAIN`]) carries the same
//! key as its reason and the args as its metadata, and text is rendered only
//! where it is shown, from a catalog keyed by the code.
//!
//! The router answers a status and a key, and never classifies (decision
//! 14.42): which of decision 14.37's four classes a status is, and what
//! reaches a policy's OnRefused, are the coordinator's. No class,
//!
//! ```compile_fail
//! use angzarr_router::error::ErrorClass;
//! ```
//!
//! no status's class,
//!
//! ```compile_fail
//! let _ = angzarr_router::error::GrpcCode::Internal.class();
//! ```
//!
//! no business-refusal test,
//!
//! ```compile_fail
//! let _ = angzarr_router::error::GrpcCode::Internal.is_business_refusal();
//! ```
//!
//! and no refused policy's view of an answer:
//!
//! ```compile_fail
//! let _ = angzarr_router::error::CodedError::unhandled().refusal_response();
//! ```

use std::collections::BTreeMap;

/// google.rpc.ErrorInfo domain for all angzarr errors.
pub const ERROR_INFO_DOMAIN: &str = "angzarr.io";

/// gRPC status codes the router emits (numeric values are the wire codes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum GrpcCode {
    Unknown = 2,
    InvalidArgument = 3,
    DeadlineExceeded = 4,
    NotFound = 5,
    AlreadyExists = 6,
    PermissionDenied = 7,
    ResourceExhausted = 8,
    FailedPrecondition = 9,
    Aborted = 10,
    OutOfRange = 11,
    Unimplemented = 12,
    Internal = 13,
    Unavailable = 14,
    DataLoss = 15,
}

/// The status a handler picks for a bug it
/// marks retryable ([`CodedError::retryable_bug`]; decision 14.37, owner's
/// rulings 2026-10-29, latest). The statuses are not interchangeable, and
/// a retry loop honours the difference:
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryableStatus {
    /// UNAVAILABLE: resend. The dependency is down for now.
    Unavailable,
    /// DEADLINE_EXCEEDED: resend only if the call is idempotent; the work
    /// may have happened. (A command is idempotent by its `command_id`.)
    DeadlineExceeded,
    /// RESOURCE_EXHAUSTED: wait longer before resending; a quota or a
    /// capacity limit was hit.
    ResourceExhausted,
}

impl From<RetryableStatus> for GrpcCode {
    fn from(status: RetryableStatus) -> Self {
        match status {
            RetryableStatus::Unavailable => GrpcCode::Unavailable,
            RetryableStatus::DeadlineExceeded => GrpcCode::DeadlineExceeded,
            RetryableStatus::ResourceExhausted => GrpcCode::ResourceExhausted,
        }
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
impl TryFrom<GrpcCode> for BusinessStatus {
    type Error = GrpcCode;
    fn try_from(code: GrpcCode) -> Result<Self, GrpcCode> {
        match code {
            GrpcCode::FailedPrecondition => Ok(BusinessStatus::FailedPrecondition),
            GrpcCode::InvalidArgument => Ok(BusinessStatus::InvalidArgument),
            GrpcCode::NotFound => Ok(BusinessStatus::NotFound),
            GrpcCode::AlreadyExists => Ok(BusinessStatus::AlreadyExists),
            GrpcCode::OutOfRange => Ok(BusinessStatus::OutOfRange),
            GrpcCode::PermissionDenied => Ok(BusinessStatus::PermissionDenied),
            other => Err(other),
        }
    }
}

/// The router's reason keys (decisions 14.37, 14.42, 14.44) are
/// [`RouterReason`], reasons.proto's enum: one value per condition, each
/// answered as its full value name ([`RouterReason::as_str_name`],
/// prefix included), its own status message and `ErrorInfo` reason. The
/// coordinator's keys (admission, descriptor validation, policy output
/// checks, refusal facts, conflict judgement) are its own; the retired ones
/// are reserved there and never answered again.
pub use angzarr_proto::v1::RouterReason;

/// Cross-language arg names: the `ErrorInfo` metadata keys of the router's
/// reasons (the args each `RouterReason` value's comment in reasons.proto
/// names).
pub mod extras {
    pub const TYPE_URL: &str = "type_url";
    pub const COMMAND_TYPE: &str = "command_type";
    /// The type an upcaster sidecar returned: never the router's, which
    /// upcasts nothing; the coordinator relays it beside `type_url` on an
    /// unknown type (decision 14.39).
    pub const UPCAST_TYPE_URL: &str = "upcast_type_url";
    pub const SEQUENCE: &str = "sequence";
    pub const STATE_TYPE_URL: &str = "state_type_url";
    pub const COMPONENT: &str = "component";
    pub const COMPONENT_KIND: &str = "component_kind";
    pub const FIRST_SEQUENCE: &str = "first_sequence";
    pub const LAST_SEQUENCE: &str = "last_sequence";
    /// A typed refusal's object's fully-qualified type (ROUTER_REASON_COMMAND_REFUSED).
    pub const REFUSAL_TYPE: &str = "refusal_type";
    /// The message bytes failed to decode as (ROUTER_REASON_ANY_DECODE_FAILED).
    pub const MESSAGE_TYPE: &str = "message_type";
    /// A wrapped cause's code; its args follow as `cause.<arg>` (decision
    /// 14.37).
    pub const CAUSE: &str = "cause";
    /// A third-party library's own error text, verbatim (decision 14.37,
    /// owner's rulings 2026-10-29, late): the one text an arg may hold.
    pub const EXT_ERR_TXT: &str = "ext_err_txt";
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
/// `cause` and `cause.<arg>`; `grpc` is resolved at construction, and
/// is the status it travels with. No constructor takes text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodedError {
    pub code: String,
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
/// error, which the boundary maps to ROUTER_REASON_UNHANDLED_HANDLER_ERROR with no cause.
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

/// `cause`'s code under `name`, each of its args under `<name>.<arg>`
/// (decision 14.37).
fn flatten(
    name: &str,
    code: &str,
    args: impl IntoIterator<Item = (String, String)>,
) -> impl Iterator<Item = (String, String)> {
    let name = name.to_string();
    std::iter::once((name.clone(), code.to_string())).chain(
        args.into_iter()
            .map(move |(arg, value)| (format!("{name}.{arg}"), value)),
    )
}

impl CodedError {
    fn coded(
        code: &str,
        extras: impl IntoIterator<Item = (String, String)>,
        grpc: GrpcCode,
    ) -> Self {
        CodedError {
            code: code.to_string(),
            extras: extras.into_iter().collect(),
            grpc,
            refusal: None,
        }
    }

    /// The gRPC status message: the reason key itself (decision 14.37).
    pub fn status_message(&self) -> &str {
        &self.code
    }

    /// This error as a key and args (types.proto, the diagnostic region).
    pub fn diagnostic(&self) -> angzarr_proto::v1::Diagnostic {
        angzarr_proto::v1::Diagnostic {
            code: self.code.clone(),
            args: self.extras.clone().into_iter().collect(),
        }
    }

    /// A command or fact whose type URL reaches no registered handler
    /// (`code` ROUTER_REASON_UNKNOWN_COMMAND_TYPE or ROUTER_REASON_NO_FACT_HANDLER): a bug (INTERNAL;
    /// owner ruling, 2026-10-29), its one arg `type_url` the type it was
    /// given (C-0682). The coordinator upcast it, and relays the type as sent
    /// beside it (decision 14.39).
    pub fn unknown_type(reason: RouterReason, type_url: &str) -> Self {
        Self::coded(
            reason.as_str_name(),
            [(extras::TYPE_URL.to_string(), type_url.to_string())],
            GrpcCode::Internal,
        )
    }

    /// Bytes that do not decode as the message `message_type` (its
    /// fully-qualified name): ROUTER_REASON_ANY_DECODE_FAILED, a bug (INTERNAL), one
    /// condition whatever the message, `message_type` its arg. prost's
    /// `error` is a third-party library's, so its own text travels
    /// verbatim as `ext_err_txt` (decision 14.37, owner's rulings
    /// 2026-10-29, late); prost gives it no numeric code.
    pub fn undecodable(message_type: &str, error: &prost::DecodeError) -> Self {
        Self::internal(
            RouterReason::AnyDecodeFailed,
            [
                (extras::MESSAGE_TYPE.to_string(), message_type.to_string()),
                (extras::EXT_ERR_TXT.to_string(), error.to_string()),
            ],
        )
    }

    /// A request the router hosts no component for (UNIMPLEMENTED): a
    /// misroute or a stale deployment.
    pub(crate) fn unimplemented(
        reason: RouterReason,
        extras: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self::coded(reason.as_str_name(), extras, GrpcCode::Unimplemented)
    }

    /// Classifies a store-sourced decode failure (DATA_LOSS on the wire).
    pub fn persisted_corrupt(type_url: &str) -> Self {
        Self::coded(
            RouterReason::PersistedEventCorrupt.as_str_name(),
            [(extras::TYPE_URL.to_string(), type_url.to_string())],
            GrpcCode::DataLoss,
        )
    }

    /// A fold of events that failed (INTERNAL), under `code`: ROUTER_REASON_FOLD_FAILED
    /// for persisted events, ROUTER_REASON_FOLD_FAILED_EMITTED for the events the command
    /// emitted. Those events were accepted (or emitted) by the same apply
    /// logic, so the failure is a handler regression, never corrupt data.
    /// `at` names the failing page or, when it is unknown, the batch's
    /// range; `cause`, the fold's own failure as a key and args, is
    /// flattened in as `cause` and `cause.<arg>`.
    pub fn fold_failed(
        reason: RouterReason,
        at: FoldAt<'_>,
        state_type_url: &str,
        cause: Option<&angzarr_proto::v1::Diagnostic>,
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
        if let Some(cause) = cause {
            details.extend(flatten(extras::CAUSE, &cause.code, cause.args.clone()));
        }
        Self::coded(reason.as_str_name(), details, GrpcCode::Internal)
    }

    /// An unclassified error that escaped a business handler (INTERNAL):
    /// ROUTER_REASON_UNHANDLED_HANDLER_ERROR with no cause. What escaped stays in the
    /// handler's own log.
    pub fn unhandled() -> Self {
        Self::coded(
            RouterReason::UnhandledHandlerError.as_str_name(),
            [],
            GrpcCode::Internal,
        )
    }

    /// A business handler that panicked (INTERNAL): ROUTER_REASON_UNHANDLED_HANDLER_ERROR
    /// with cause ROUTER_REASON_HANDLER_PANICKED. The panic's message stays in the
    /// handler's own log.
    pub fn panicked() -> Self {
        Self::bug(RouterReason::HandlerPanicked.as_str_name(), [])
    }

    /// A business rejection under `status`, one of the business set: the
    /// command is refused, a business "no", never a fault. One of the two
    /// classes a handler fails with (the other is [`Self::bug`]). `code` is
    /// its key and `details` its args; a typed refusal is
    /// [`Self::refused`] instead. The coordinator hands it to the issuing
    /// policy's OnRefused.
    pub fn rejection(
        status: BusinessStatus,
        code: &str,
        details: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self::coded(code, details, status.into())
    }

    /// A typed business refusal (FAILED_PRECONDITION): `refusal` is one of
    /// the command's `refuses` types, packed. Its wire reason is
    /// ROUTER_REASON_COMMAND_REFUSED, its `refusal_type` arg the object's fully-qualified
    /// type, and the object itself rides the status details (owner's
    /// ruling on decision 14.37).
    pub fn refused(refusal: prost_types::Any) -> Self {
        let full_name = refusal
            .type_url
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string();
        Self::rejection(
            BusinessStatus::FailedPrecondition,
            RouterReason::CommandRefused.as_str_name(),
            [(extras::REFUSAL_TYPE.to_string(), full_name)],
        )
        .with_refusal(refusal)
    }

    /// A bug: a defect, never a business answer. The other of the two
    /// classes a handler fails with (the first is [`Self::rejection`]). It
    /// goes out as ROUTER_REASON_UNHANDLED_HANDLER_ERROR (INTERNAL), with `code` and
    /// `details` as its cause, flattened as `cause=<code>` and
    /// `cause.<arg>=<value>` (decision 14.37); the runtime alerts it and
    /// dead-letters the input, and it never reaches a policy's OnRefused.
    pub fn bug(code: &str, details: impl IntoIterator<Item = (String, String)>) -> Self {
        Self::coded(
            RouterReason::UnhandledHandlerError.as_str_name(),
            flatten(extras::CAUSE, code, details),
            GrpcCode::Internal,
        )
    }

    /// A bug the handler marks retryable (an external dependency that is
    /// down), under the status it picks (decision 14.37, owner's rulings
    /// 2026-10-29, latest): that status, ROUTER_REASON_UNHANDLED_HANDLER_ERROR, `code`
    /// and `details` as its cause, flattened as [`Self::bug`]'s are. The
    /// runtime retries it with capped backoff, then dead-letters it; it
    /// never reaches a policy's OnRefused.
    pub fn retryable_bug(
        status: RetryableStatus,
        code: &str,
        details: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self::coded(
            RouterReason::UnhandledHandlerError.as_str_name(),
            flatten(extras::CAUSE, code, details),
            status.into(),
        )
    }

    /// A handler's immediate-return business error, "re-read and
    /// re-decide" (decision 14.37, owner's rulings 2026-10-29, latest):
    /// ABORTED under the handler's own `code`, its args `details`.
    /// Returned at once to the caller, never a policy's OnRefused; for a
    /// policy step the runtime reloads and re-runs the step.
    pub fn immediate(code: &str, details: impl IntoIterator<Item = (String, String)>) -> Self {
        Self::coded(code, details, GrpcCode::Aborted)
    }

    /// A bug the router itself finds (INTERNAL), under its own code: a
    /// type no handler claims, a component's misconfiguration. Never a
    /// handler's, and never a business refusal (owner ruling, 2026-10-29).
    pub(crate) fn internal(
        reason: RouterReason,
        extras: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self::coded(reason.as_str_name(), extras, GrpcCode::Internal)
    }

    /// This business rejection carrying its typed refusal object (one of
    /// the command's `refuses` types, packed).
    pub fn with_refusal(self, refusal: prost_types::Any) -> Self {
        CodedError {
            refusal: Some(Box::new(refusal)),
            ..self
        }
    }
}

/// In process too, an error shows as its code then its args, ` name=value`
/// each, a wrapped cause flattened as on the wire, a value quoted where it
/// would not parse back ([`write_value`]); never English (decision 14.37,
/// owner's clarification of 2026-10-29).
impl std::fmt::Display for CodedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.code)?;
        for (name, value) in &self.extras {
            write!(f, " {name}=")?;
            write_value(f, value)?;
        }
        Ok(())
    }
}

/// `value` as a display writes it (decision 14.37, owner ruling
/// 2026-10-29): bare, unless it would not parse back (empty, or holding
/// whitespace, `=`, `"` or `\`), then double-quoted with its `"` and `\`
/// backslash-escaped.
fn write_value(f: &mut std::fmt::Formatter<'_>, value: &str) -> std::fmt::Result {
    let ambiguous = value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '=' | '"' | '\\'));
    if !ambiguous {
        return f.write_str(value);
    }
    f.write_str("\"")?;
    for c in value.chars() {
        if matches!(c, '"' | '\\') {
            f.write_str("\\")?;
        }
        write!(f, "{c}")?;
    }
    f.write_str("\"")
}

/// A CodedError is a std error, so an applier may fail with one: a failing
/// fold then carries it as its `cause`.
impl std::error::Error for CodedError {}

/// The single handler-error mapping: coded errors pass through carrying
/// their resolved gRPC code; unclassified errors become
/// ROUTER_REASON_UNHANDLED_HANDLER_ERROR (INTERNAL) with no cause, their text left to the
/// handler's own log.
pub fn map_handler_error(err: HandlerError) -> CodedError {
    match err {
        HandlerError::Coded(coded) => coded,
        HandlerError::Other(escaped) => {
            tracing::error!(
                code = RouterReason::UnhandledHandlerError.as_str_name(),
                escaped = %escaped,
                "a handler's unclassified error"
            );
            CodedError::unhandled()
        }
    }
}

/// Runs a business handler, answering its panic as a bug
/// (ROUTER_REASON_UNHANDLED_HANDLER_ERROR, cause ROUTER_REASON_HANDLER_PANICKED; decision 14.37) and
/// mapping its error ([`map_handler_error`]). The panic's message stays in
/// the handler's own log; the router answers rather than unwinding into its
/// host.
pub(crate) fn call_handler<T>(
    handler: impl FnOnce() -> Result<T, HandlerError>,
) -> Result<T, CodedError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(handler)) {
        Ok(answer) => answer.map_err(map_handler_error),
        Err(_) => Err(CodedError::panicked()),
    }
}

#[cfg(test)]
#[path = "error.test.rs"]
mod error_tests;
