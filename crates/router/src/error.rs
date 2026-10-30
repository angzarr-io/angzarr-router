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

/// The class an error is, decided by its gRPC status alone (decision
/// 14.37). Four classes (owner's rulings, 2026-10-29, latest). Acting on
/// the class (the retry, its backoff and cap, the dead letter, the alert,
/// re-running a policy step) is the runtime's configuration and work; the
/// router only classifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// Business, fatal: FAILED_PRECONDITION, INVALID_ARGUMENT, NOT_FOUND,
    /// ALREADY_EXISTS, OUT_OF_RANGE, PERMISSION_DENIED. Final: to the
    /// caller, or the issuing policy's OnRefused; never retried.
    BusinessFatal,
    /// Business, immediate return: ABORTED, "re-read and re-decide". The
    /// framework's conflicts (a stale or mismatched sequence, an overlap)
    /// or a handler's own choice ([`CodedError::immediate`]). Returned at
    /// once to an outside caller; for a policy step the runtime is the
    /// caller, reloading and re-running the step under the capped backoff,
    /// then dead-lettering it. Never OnRefused.
    BusinessImmediate,
    /// Bug, fatal: INTERNAL, UNKNOWN, DATA_LOSS. Dead-lettered and alerted
    /// at once.
    BugFatal,
    /// Bug, retryable: UNAVAILABLE, DEADLINE_EXCEEDED, RESOURCE_EXHAUSTED
    /// (a handler's [`RetryableStatus`]), UNIMPLEMENTED and the remaining
    /// framework statuses. Capped exponential backoff with jitter, then
    /// dead-lettered and alerted. The statuses are not interchangeable:
    /// see [`RetryableStatus`].
    BugRetryable,
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

impl GrpcCode {
    /// The class an error with this status is (decision 14.37, owner's
    /// rulings 2026-10-29, latest). UNIMPLEMENTED (a request this router
    /// hosts no component for: a misroute or a stale deployment) is
    /// [`ErrorClass::BugRetryable`].
    pub fn class(self) -> ErrorClass {
        match self {
            GrpcCode::FailedPrecondition
            | GrpcCode::InvalidArgument
            | GrpcCode::NotFound
            | GrpcCode::AlreadyExists
            | GrpcCode::OutOfRange
            | GrpcCode::PermissionDenied => ErrorClass::BusinessFatal,
            GrpcCode::Aborted => ErrorClass::BusinessImmediate,
            GrpcCode::Internal | GrpcCode::Unknown | GrpcCode::DataLoss => ErrorClass::BugFatal,
            GrpcCode::Unavailable
            | GrpcCode::DeadlineExceeded
            | GrpcCode::ResourceExhausted
            | GrpcCode::Unimplemented => ErrorClass::BugRetryable,
        }
    }

    /// True when an error with this status is a business refusal that
    /// reaches the issuing policy's OnRefused: [`ErrorClass::BusinessFatal`]
    /// only.
    pub fn is_business_refusal(self) -> bool {
        self.class() == ErrorClass::BusinessFatal
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

/// The router's reason keys (decisions 14.37, 14.42): one per condition,
/// each its own status message and `ErrorInfo` reason, exactly the keys
/// angzarr-project's `catalog/reasons.toml` lists with `owner = "router"`.
/// The coordinator's keys (admission, descriptor validation, policy output
/// checks, refusal facts, conflict judgement) are its own; the retired ones
/// are never answered again.
pub mod codes {
    // --- handler failures ---------------------------------------------------
    /// An error escaped a business handler, or it failed as a bug: its own
    /// code and args, when it has them, are the `cause`.
    pub const UNHANDLED_HANDLER_ERROR: &str = "UNHANDLED_HANDLER_ERROR";
    /// The cause of a handler that panicked.
    pub const HANDLER_PANICKED: &str = "HANDLER_PANICKED";
    /// A typed business refusal: its `refusal_type` names the object.
    pub const COMMAND_REFUSED: &str = "COMMAND_REFUSED";
    /// A fold failed on persisted events.
    pub const FOLD_FAILED: &str = "FOLD_FAILED";
    /// A fold failed on an event the command emitted.
    pub const FOLD_FAILED_EMITTED: &str = "FOLD_FAILED_EMITTED";
    /// A starting state the state's codec cannot decode.
    pub const PERSISTED_EVENT_CORRUPT: &str = "PERSISTED_EVENT_CORRUPT";

    // --- routing ------------------------------------------------------------
    /// A payload that does not decode as the message it claims.
    pub const ANY_DECODE_FAILED: &str = "ANY_DECODE_FAILED";
    /// A command type no handler claims.
    pub const UNKNOWN_COMMAND_TYPE: &str = "UNKNOWN_COMMAND_TYPE";
    /// A fact type no handler claims.
    pub const NO_FACT_HANDLER: &str = "NO_FACT_HANDLER";
    /// A pass-through CommandRefused the domain has no handler for.
    pub const NO_REFUSAL_HANDLER: &str = "NO_REFUSAL_HANDLER";
    /// A COMMUTATIVE delivery to a state with no resolver.
    pub const CONFLICT_RESOLUTION_UNSUPPORTED: &str = "CONFLICT_RESOLUTION_UNSUPPORTED";
    /// A dispatch to a kind of component the router does not host.
    pub const NO_COMPONENT_OF_KIND: &str = "NO_COMPONENT_OF_KIND";

    // --- registration -------------------------------------------------------
    pub const NO_HANDLERS_REGISTERED: &str = "NO_HANDLERS_REGISTERED";
    /// A second component beside the one a router (a business process)
    /// hosts.
    pub const DUPLICATE_COMPONENT: &str = "DUPLICATE_COMPONENT";

    /// The router's complete reason inventory: every key the router answers
    /// with, and the catalog's router keys (decision 14.42).
    pub const ALL: &[&str] = &[
        UNHANDLED_HANDLER_ERROR,
        HANDLER_PANICKED,
        COMMAND_REFUSED,
        FOLD_FAILED,
        FOLD_FAILED_EMITTED,
        PERSISTED_EVENT_CORRUPT,
        ANY_DECODE_FAILED,
        UNKNOWN_COMMAND_TYPE,
        NO_FACT_HANDLER,
        NO_REFUSAL_HANDLER,
        CONFLICT_RESOLUTION_UNSUPPORTED,
        NO_COMPONENT_OF_KIND,
        NO_HANDLERS_REGISTERED,
        DUPLICATE_COMPONENT,
    ];
}

/// Cross-language arg names: the `ErrorInfo` metadata keys of the router's
/// reasons (the catalog's `args`).
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
    /// A typed refusal's object's fully-qualified type (COMMAND_REFUSED).
    pub const REFUSAL_TYPE: &str = "refusal_type";
    /// The message bytes failed to decode as (ANY_DECODE_FAILED).
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
/// decides the error's [`ErrorClass`]. No constructor takes text.
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
    /// (`code` UNKNOWN_COMMAND_TYPE or NO_FACT_HANDLER): a bug (INTERNAL;
    /// owner ruling, 2026-10-29), its one arg `type_url` the type it was
    /// given (C-0682). The coordinator upcast it, and relays the type as sent
    /// beside it (decision 14.39).
    pub fn unknown_type(code: &str, type_url: &str) -> Self {
        Self::coded(
            code,
            [(extras::TYPE_URL.to_string(), type_url.to_string())],
            GrpcCode::Internal,
        )
    }

    /// Bytes that do not decode as the message `message_type` (its
    /// fully-qualified name): ANY_DECODE_FAILED, a bug (INTERNAL), one
    /// condition whatever the message, `message_type` its arg. prost's
    /// `error` is a third-party library's, so its own text travels
    /// verbatim as `ext_err_txt` (decision 14.37, owner's rulings
    /// 2026-10-29, late); prost gives it no numeric code.
    pub fn undecodable(message_type: &str, error: &prost::DecodeError) -> Self {
        Self::internal(
            codes::ANY_DECODE_FAILED,
            [
                (extras::MESSAGE_TYPE.to_string(), message_type.to_string()),
                (extras::EXT_ERR_TXT.to_string(), error.to_string()),
            ],
        )
    }

    /// A request the router hosts no component for, or a policy source
    /// from outside its input domain (UNIMPLEMENTED): a misroute or a
    /// stale deployment.
    pub(crate) fn unimplemented(
        code: &str,
        extras: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self::coded(code, extras, GrpcCode::Unimplemented)
    }

    /// Classifies a store-sourced decode failure (DATA_LOSS on the wire).
    pub fn persisted_corrupt(type_url: &str) -> Self {
        Self::coded(
            codes::PERSISTED_EVENT_CORRUPT,
            [(extras::TYPE_URL.to_string(), type_url.to_string())],
            GrpcCode::DataLoss,
        )
    }

    /// A fold of events that failed (INTERNAL), under `code`: FOLD_FAILED
    /// for persisted events, FOLD_FAILED_EMITTED for the events the command
    /// emitted. Those events were accepted (or emitted) by the same apply
    /// logic, so the failure is a handler regression, never corrupt data.
    /// `at` names the failing page or, when it is unknown, the batch's
    /// range; `cause`, the fold's own failure as a key and args, is
    /// flattened in as `cause` and `cause.<arg>`.
    pub fn fold_failed(
        code: &str,
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
    pub fn panicked() -> Self {
        Self::bug(codes::HANDLER_PANICKED, [])
    }

    /// A business rejection under `status`, one of the business set: the
    /// command is refused, a business "no", never a fault. One of the two
    /// classes a handler fails with (the other is [`Self::bug`]). `code` is
    /// its key and `details` its args; a typed refusal is
    /// [`Self::refused`] instead. The refusal reaches the issuing policy's
    /// OnRefused ([`Self::refusal_response`]).
    pub fn rejection(
        status: BusinessStatus,
        code: &str,
        details: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self::coded(code, details, status.into())
    }

    /// A typed business refusal (FAILED_PRECONDITION): `refusal` is one of
    /// the command's `refuses` types, packed. Its wire reason is
    /// COMMAND_REFUSED, its `refusal_type` arg the object's fully-qualified
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
            codes::COMMAND_REFUSED,
            [(extras::REFUSAL_TYPE.to_string(), full_name)],
        )
        .with_refusal(refusal)
    }

    /// A bug: a defect, never a business answer. The other of the two
    /// classes a handler fails with (the first is [`Self::rejection`]). It
    /// goes out as UNHANDLED_HANDLER_ERROR (INTERNAL), with `code` and
    /// `details` as its cause, flattened as `cause=<code>` and
    /// `cause.<arg>=<value>` (decision 14.37); the runtime alerts it and
    /// dead-letters the input, and it never reaches a policy's OnRefused.
    pub fn bug(code: &str, details: impl IntoIterator<Item = (String, String)>) -> Self {
        Self::coded(
            codes::UNHANDLED_HANDLER_ERROR,
            flatten(extras::CAUSE, code, details),
            GrpcCode::Internal,
        )
    }

    /// A bug the handler marks retryable (an external dependency that is
    /// down), under the status it picks (decision 14.37, owner's rulings
    /// 2026-10-29, latest): that status, UNHANDLED_HANDLER_ERROR, `code`
    /// and `details` as its cause, flattened as [`Self::bug`]'s are. The
    /// runtime retries it with capped backoff, then dead-letters it; it
    /// never reaches a policy's OnRefused.
    pub fn retryable_bug(
        status: RetryableStatus,
        code: &str,
        details: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self::coded(
            codes::UNHANDLED_HANDLER_ERROR,
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

    /// A bug the router itself finds (INTERNAL), under its own code:
    /// malformed input, a component's misconfiguration, a policy's
    /// malformed answer. Never a handler's, and never a business refusal
    /// (owner ruling, 2026-10-29).
    pub(crate) fn internal(code: &str, extras: impl IntoIterator<Item = (String, String)>) -> Self {
        Self::coded(code, extras, GrpcCode::Internal)
    }

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
    /// the business set, [`GrpcCode::is_business_refusal`]), carrying the
    /// typed refusal object, or, for an untyped rejection, its key and args
    /// as a `Diagnostic`; `None` for every other failure, which never
    /// reaches a policy's OnRefused (retried, or dead-lettered as a bug).
    pub fn refusal_response(&self) -> Option<angzarr_proto::v1::RefusalResponse> {
        use angzarr_proto::v1::refusal_response::Reason;
        self.grpc
            .is_business_refusal()
            .then(|| angzarr_proto::v1::RefusalResponse {
                reason: Some(match self.refusal.as_deref() {
                    Some(refusal) => Reason::Refusal(refusal.clone()),
                    None => Reason::Diagnostic(self.diagnostic()),
                }),
            })
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
/// UNHANDLED_HANDLER_ERROR (INTERNAL) with no cause, their text left to the
/// handler's own log.
pub fn map_handler_error(err: HandlerError) -> CodedError {
    match err {
        HandlerError::Coded(coded) => coded,
        HandlerError::Other(escaped) => {
            tracing::error!(
                code = codes::UNHANDLED_HANDLER_ERROR,
                escaped = %escaped,
                "a handler's unclassified error"
            );
            CodedError::unhandled()
        }
    }
}

/// Runs a business handler, answering its panic as a bug
/// (UNHANDLED_HANDLER_ERROR, cause HANDLER_PANICKED; decision 14.37) and
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
