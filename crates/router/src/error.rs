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
/// a bug is alerted and dead-lettered; anything else is retried with
/// backoff up to a configured cap, then dead-lettered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// FAILED_PRECONDITION, INVALID_ARGUMENT, NOT_FOUND, ALREADY_EXISTS,
    /// OUT_OF_RANGE, PERMISSION_DENIED.
    BusinessRefusal,
    /// INTERNAL, UNKNOWN, DATA_LOSS.
    Bug,
    /// UNAVAILABLE, ABORTED, UNIMPLEMENTED and the like. The caller
    /// retries with backoff up to a configured cap, then dead-letters,
    /// which makes it visible ("retry with backoff, cap, then dead letter";
    /// owner ruling, 2026-10-29). The backoff and the cap are the runtime's
    /// configuration; the router only classifies.
    Retryable,
}

impl GrpcCode {
    /// The class an error with this status is (owner ruling, 2026-10-29).
    /// UNIMPLEMENTED (a request this router hosts no component for: a
    /// misroute or a stale deployment) is [`ErrorClass::Retryable`]: retried
    /// with backoff up to a configured cap, then dead-lettered.
    pub fn class(self) -> ErrorClass {
        match self {
            GrpcCode::FailedPrecondition
            | GrpcCode::InvalidArgument
            | GrpcCode::NotFound
            | GrpcCode::AlreadyExists
            | GrpcCode::OutOfRange
            | GrpcCode::PermissionDenied => ErrorClass::BusinessRefusal,
            GrpcCode::Internal | GrpcCode::Unknown | GrpcCode::DataLoss => ErrorClass::Bug,
            GrpcCode::Aborted | GrpcCode::Unavailable | GrpcCode::Unimplemented => {
                ErrorClass::Retryable
            }
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

/// The router's reason keys (decision 14.37): one per condition, each its
/// own status message and `ErrorInfo` reason.
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

    // --- malformed input (bugs: owner ruling, 2026-10-29) -----------------
    pub const ANY_DECODE_FAILED: &str = "ANY_DECODE_FAILED";
    pub const MISSING_COMMAND_BOOK: &str = "MISSING_COMMAND_BOOK";
    pub const MISSING_COMMAND_PAGE: &str = "MISSING_COMMAND_PAGE";
    pub const MISSING_COMMAND_PAYLOAD: &str = "MISSING_COMMAND_PAYLOAD";
    pub const MISSING_EVENT_BOOK_COVER: &str = "MISSING_EVENT_BOOK_COVER";
    pub const EMPTY_POLICY_SOURCE: &str = "EMPTY_POLICY_SOURCE";
    /// A command type no handler claims, after upcasting.
    pub const UNKNOWN_COMMAND_TYPE: &str = "UNKNOWN_COMMAND_TYPE";
    /// A fact type no handler claims, after upcasting.
    pub const NO_FACT_HANDLER: &str = "NO_FACT_HANDLER";
    /// A pass-through CommandRefused the domain has no handler for.
    pub const NO_REFUSAL_HANDLER: &str = "NO_REFUSAL_HANDLER";
    /// A Refusal given to a policy with no command.
    pub const REFUSAL_WITHOUT_COMMAND: &str = "REFUSAL_WITHOUT_COMMAND";
    /// A Refusal given to a policy with no event.
    pub const REFUSAL_WITHOUT_EVENT: &str = "REFUSAL_WITHOUT_EVENT";
    /// A refusal fact addressed to any stream but the exact one its
    /// causation_id names (types.proto, the command_identity region).
    pub const REFUSAL_FACT_WRONG_STREAM: &str = "REFUSAL_FACT_WRONG_STREAM";
    pub const CONFLICT_RESOLUTION_UNSUPPORTED: &str = "CONFLICT_RESOLUTION_UNSUPPORTED";
    /// A host callback's status bytes that do not decode (the C ABI).
    pub const HOST_STATUS_UNDECODABLE: &str = "HOST_STATUS_UNDECODABLE";
    /// A null pointer crossing the C ABI.
    pub const NULL_ABI_POINTER: &str = "NULL_ABI_POINTER";

    // --- the store ------------------------------------------------------------
    pub const PERSISTED_EVENT_CORRUPT: &str = "PERSISTED_EVENT_CORRUPT";

    // --- stale commands ---------------------------------------------------------
    pub const CONFLICT_UNRESOLVED: &str = "CONFLICT_UNRESOLVED";
    pub const CONFLICT_OVERLAP: &str = "CONFLICT_OVERLAP";

    // --- misroutes --------------------------------------------------------------
    /// A dispatch to a kind of component the router does not host.
    pub const NO_COMPONENT_OF_KIND: &str = "NO_COMPONENT_OF_KIND";
    /// A policy given a book of a domain it does not consume.
    pub const POLICY_SOURCE_OUTSIDE_INPUT_DOMAIN: &str = "POLICY_SOURCE_OUTSIDE_INPUT_DOMAIN";

    // --- registration -------------------------------------------------------
    pub const NO_HANDLERS_REGISTERED: &str = "NO_HANDLERS_REGISTERED";
    /// A second component beside the one a router (a business process)
    /// hosts.
    pub const DUPLICATE_COMPONENT: &str = "DUPLICATE_COMPONENT";
    /// A second upcaster chain for a component.
    pub const DUPLICATE_UPCASTER_CHAIN: &str = "DUPLICATE_UPCASTER_CHAIN";
    pub const DUPLICATE_COMMAND_HANDLER: &str = "DUPLICATE_COMMAND_HANDLER";
    pub const MISSING_RULES_VERSION: &str = "MISSING_RULES_VERSION";
    pub const MISSING_STATE_TYPE: &str = "MISSING_STATE_TYPE";
    /// A component other than a process manager declaring starting types
    /// (types.proto, ComponentDescriptor.starting_types).
    pub const STARTING_TYPES_ON_NON_PM: &str = "STARTING_TYPES_ON_NON_PM";

    // --- a policy's bugs ------------------------------------------------------
    pub const POLICY_COMMAND_OUTSIDE_DESTINATION: &str = "POLICY_COMMAND_OUTSIDE_DESTINATION";
    /// A policy command naming an edition other than its trigger's (owner
    /// ruling, 2026-10-28; types.proto Cover.edition).
    pub const POLICY_COMMAND_OUTSIDE_EDITION: &str = "POLICY_COMMAND_OUTSIDE_EDITION";
    /// A policy's command whose cover names no root (decision 14.36).
    pub const POLICY_COMMAND_NO_ROOT: &str = "POLICY_COMMAND_NO_ROOT";
    /// A policy's command carrying a correlation_id other than its
    /// trigger's (decision 14.36).
    pub const POLICY_COMMAND_OTHER_CORRELATION: &str = "POLICY_COMMAND_OTHER_CORRELATION";
    /// OnRefusedResponse holds no fact (policy.proto, OnRefusedResponse).
    pub const REFUSAL_FACT_MISSING: &str = "REFUSAL_FACT_MISSING";
    /// The policy set the refusal fact's cover or handling.
    pub const REFUSAL_FACT_ADDRESSED: &str = "REFUSAL_FACT_ADDRESSED";
    /// The refusal fact has no page.
    pub const REFUSAL_FACT_NO_PAGE: &str = "REFUSAL_FACT_NO_PAGE";
    /// The refusal fact's page is a Command.
    pub const REFUSAL_FACT_NOT_A_FACT: &str = "REFUSAL_FACT_NOT_A_FACT";
    /// The policy set the refusal fact's identity.
    pub const REFUSAL_FACT_IDENTIFIED: &str = "REFUSAL_FACT_IDENTIFIED";
    /// The refusal to address carries no refused command.
    pub const REFUSAL_WITHOUT_REFUSED_COMMAND: &str = "REFUSAL_WITHOUT_REFUSED_COMMAND";
    /// The refused command carries no command_id.
    pub const REFUSED_COMMAND_WITHOUT_ID: &str = "REFUSED_COMMAND_WITHOUT_ID";
    /// The refusal carries no triggering stream.
    pub const REFUSAL_WITHOUT_TRIGGER_STREAM: &str = "REFUSAL_WITHOUT_TRIGGER_STREAM";

    // --- business rejections the router answers ---------------------------
    /// A command for a process manager's correlation with no instance, of a
    /// type the PM does not declare as starting (decision 14.31).
    pub const PM_NO_INSTANCE: &str = "PM_NO_INSTANCE";

    /// The router's complete reason inventory (decision 14.37): every key
    /// the router, or the C ABI over it, answers with. A catalog that
    /// renders the router's errors carries a message for each.
    pub const ALL: &[&str] = &[
        UNHANDLED_HANDLER_ERROR,
        HANDLER_PANICKED,
        COMMAND_REFUSED,
        FOLD_FAILED,
        FOLD_FAILED_EMITTED,
        ANY_DECODE_FAILED,
        MISSING_COMMAND_BOOK,
        MISSING_COMMAND_PAGE,
        MISSING_COMMAND_PAYLOAD,
        MISSING_EVENT_BOOK_COVER,
        EMPTY_POLICY_SOURCE,
        UNKNOWN_COMMAND_TYPE,
        NO_FACT_HANDLER,
        NO_REFUSAL_HANDLER,
        REFUSAL_WITHOUT_COMMAND,
        REFUSAL_WITHOUT_EVENT,
        REFUSAL_FACT_WRONG_STREAM,
        CONFLICT_RESOLUTION_UNSUPPORTED,
        HOST_STATUS_UNDECODABLE,
        NULL_ABI_POINTER,
        PERSISTED_EVENT_CORRUPT,
        CONFLICT_UNRESOLVED,
        CONFLICT_OVERLAP,
        NO_COMPONENT_OF_KIND,
        POLICY_SOURCE_OUTSIDE_INPUT_DOMAIN,
        NO_HANDLERS_REGISTERED,
        DUPLICATE_COMPONENT,
        DUPLICATE_UPCASTER_CHAIN,
        DUPLICATE_COMMAND_HANDLER,
        MISSING_RULES_VERSION,
        MISSING_STATE_TYPE,
        STARTING_TYPES_ON_NON_PM,
        POLICY_COMMAND_OUTSIDE_DESTINATION,
        POLICY_COMMAND_OUTSIDE_EDITION,
        POLICY_COMMAND_NO_ROOT,
        POLICY_COMMAND_OTHER_CORRELATION,
        REFUSAL_FACT_MISSING,
        REFUSAL_FACT_ADDRESSED,
        REFUSAL_FACT_NO_PAGE,
        REFUSAL_FACT_NOT_A_FACT,
        REFUSAL_FACT_IDENTIFIED,
        REFUSAL_WITHOUT_REFUSED_COMMAND,
        REFUSED_COMMAND_WITHOUT_ID,
        REFUSAL_WITHOUT_TRIGGER_STREAM,
        PM_NO_INSTANCE,
    ];
}

/// Cross-language arg names: the `ErrorInfo` metadata keys.
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
    pub const FIRST_SEQUENCE: &str = "first_sequence";
    pub const LAST_SEQUENCE: &str = "last_sequence";
    /// A typed refusal's object's fully-qualified type (COMMAND_REFUSED).
    pub const REFUSAL_TYPE: &str = "refusal_type";
    /// The message bytes failed to decode as (ANY_DECODE_FAILED).
    pub const MESSAGE_TYPE: &str = "message_type";
    /// A wrapped cause's code; its args follow as `cause.<arg>` (decision
    /// 14.37).
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
    /// owner ruling, 2026-10-29), its args carrying the type URL as sent as
    /// `type_url` and the type the upcaster chain ended at as
    /// `upcast_type_url` (upcaster.proto, VC-1: the sent one when no
    /// upcaster applies).
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
            GrpcCode::Internal,
        )
    }

    /// Bytes that do not decode as the message `message_type` (its
    /// fully-qualified name): ANY_DECODE_FAILED, a bug (INTERNAL), one
    /// condition whatever the message, `message_type` its arg.
    pub fn undecodable(message_type: &str) -> Self {
        Self::internal(
            codes::ANY_DECODE_FAILED,
            [(extras::MESSAGE_TYPE.to_string(), message_type.to_string())],
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

    /// A stale command (ABORTED, as STRICT would answer it): its conflict
    /// window was not resolved, or COMMUTATIVE found an overlap.
    pub fn stale(code: &str, extras: impl IntoIterator<Item = (String, String)>) -> Self {
        Self::coded(code, extras, GrpcCode::Aborted)
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
/// each, a wrapped cause flattened as on the wire; never English (decision
/// 14.37, owner's clarification of 2026-10-29).
impl std::fmt::Display for CodedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.code)?;
        for (name, value) in &self.extras {
            write!(f, " {name}={value}")?;
        }
        Ok(())
    }
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
