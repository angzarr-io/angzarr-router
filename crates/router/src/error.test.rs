//! The single error-mapping table, pinned: these wire codes are the
//! adapter-level contracts client-go's engine_test.go checks through its
//! gRPC handler (DATA_LOSS for corrupt persisted events, UNIMPLEMENTED
//! for a request the router hosts no component for, INTERNAL +
//! ROUTER_REASON_UNHANDLED_HANDLER_ERROR for unclassified failures), asserted here
//! directly on the coded form the FFI boundary will serialize.
//!
//! Decision 14.37: every error is a key and an argument map, never English.
//! The status message IS the key; a wrapped cause is flattened into the
//! args as `cause` and `cause.<arg>`; no constructor takes or carries text.

use super::*;
use angzarr_proto::v1 as pb;

fn arg(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// A prost DecodeError, as undecodable bytes raise it.
fn decode_error() -> prost::DecodeError {
    <pb::ContextualCommand as prost::Message>::decode(&[0xff, 0xff, 0xff][..])
        .expect_err("undecodable")
}

fn typed_refusal() -> prost_types::Any {
    prost_types::Any {
        type_url: "/test.wallet.InsufficientFunds".to_string(),
        value: vec![8, 100, 16, 150],
    }
}

/// Every handler- and router-facing constructor, called as decision 14.37
/// shapes them: a key and args, no text. (`HandlerError::Other`'s text is
/// the handler's own, handed in to prove it goes no further.)
fn every_constructor() -> Vec<(&'static str, CodedError)> {
    vec![
        (
            "internal",
            CodedError::internal(RouterReason::NoHandlersRegistered, []),
        ),
        (
            "internal with args",
            CodedError::internal(
                RouterReason::DuplicateComponent,
                [arg(extras::COMPONENT, "order")],
            ),
        ),
        (
            "unimplemented",
            CodedError::unimplemented(RouterReason::NoComponentOfKind, []),
        ),
        (
            "unknown_type",
            CodedError::unknown_type(RouterReason::UnknownCommandType, "/a.Sent"),
        ),
        (
            "undecodable",
            CodedError::undecodable("io.angzarr.v1.ContextualCommand", &decode_error()),
        ),
        ("persisted_corrupt", CodedError::persisted_corrupt("/a.Evt")),
        (
            "fold_failed",
            CodedError::fold_failed(
                RouterReason::FoldFailed,
                FoldAt::Range { first: 1, last: 2 },
                "/a.State",
                None,
            ),
        ),
        ("unhandled", CodedError::unhandled()),
        ("panicked", CodedError::panicked()),
        (
            "rejection",
            CodedError::rejection(
                BusinessStatus::FailedPrecondition,
                "ORDER_CLOSED",
                [arg("order_state", "closed")],
            ),
        ),
        ("refused", CodedError::refused(typed_refusal())),
        (
            "bug",
            CodedError::bug("LEDGER_OFFLINE", [arg("ledger", "main")]),
        ),
        (
            "retryable_bug",
            CodedError::retryable_bug(
                RetryableStatus::Unavailable,
                "LEDGER_OFFLINE",
                [arg("ledger", "main")],
            ),
        ),
        (
            "immediate",
            CodedError::immediate("PRICE_MOVED", [arg("item", "apple")]),
        ),
        (
            "map_handler_error(Other)",
            map_handler_error(HandlerError::Other("connection reset".to_string())),
        ),
    ]
}

// --- decision 14.37: a key and args, never English ------------------------------

#[test]
fn every_constructors_status_message_is_its_code() {
    let wrong: Vec<String> = every_constructor()
        .into_iter()
        .filter(|(_, err)| err.status_message() != err.code || err.code.is_empty())
        .map(|(name, err)| {
            format!(
                "{name}: status message {:?}, code {:?}",
                err.status_message(),
                err.code
            )
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "the gRPC status message IS the reason key (decision 14.37):\n{}",
        wrong.join("\n")
    );
}

/// `value` as a display shows it: bare, or, where it would not parse back
/// (empty, or holding whitespace, `=`, `"` or `\`), double-quoted with
/// its `"` and `\` escaped.
fn shown_value(value: &str) -> String {
    let ambiguous = value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '=' | '"' | '\\'));
    if !ambiguous {
        return value.to_string();
    }
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

// In process too, an error shows as its code then its args, ` name=value`
// each (owner's clarification of 2026-10-29), a value quoted where it
// would not parse back.
#[test]
fn every_error_displays_as_its_code_then_its_args() {
    let wrong: Vec<String> = every_constructor()
        .into_iter()
        .filter_map(|(name, err)| {
            let expected: String = std::iter::once(err.code.clone())
                .chain(
                    err.extras
                        .iter()
                        .map(|(k, v)| format!(" {k}={}", shown_value(v))),
                )
                .collect();
            let shown = err.to_string();
            (shown != expected || shown.is_empty())
                .then(|| format!("{name}: displays {shown:?}, expected {expected:?}"))
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "an error's Display is its code then its args, never English:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn a_bug_displays_its_cause_flattened() {
    let err = CodedError::bug("LEDGER_OFFLINE", [arg("ledger", "main")]);
    assert_eq!(
        err.to_string(),
        "ROUTER_REASON_UNHANDLED_HANDLER_ERROR cause=LEDGER_OFFLINE cause.ledger=main"
    );
}

// A handler's unclassified error is a bug with no cause: what escaped stays
// in the handler's own log (C-0669).
#[test]
fn an_unclassified_handler_errors_text_never_leaves_the_handler() {
    let err = map_handler_error(HandlerError::Other("connection reset".to_string()));
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, RouterReason::UnhandledHandlerError.as_str_name());
    assert!(
        !err.extras.contains_key(extras::CAUSE),
        "no cause: {:?}",
        err.extras
    );
    let carried = format!("{err:?}");
    assert!(
        !carried.contains("connection reset"),
        "the escaped error's text crossed: {carried}"
    );
}

// A panic's cause is ROUTER_REASON_HANDLER_PANICKED; its message stays in the handler's
// log (decision 14.37).
#[test]
fn a_panic_is_unhandled_handler_error_caused_by_handler_panicked() {
    let err = CodedError::panicked();
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, RouterReason::UnhandledHandlerError.as_str_name());
    assert_eq!(
        err.extras,
        [arg("cause", "ROUTER_REASON_HANDLER_PANICKED")].into(),
        "the panic's cause"
    );
}

// Owner's ruling on 14.37: a typed refusal's reason is ROUTER_REASON_COMMAND_REFUSED,
// `refusal_type` its object's fully-qualified type, the object in the
// status details.
#[test]
fn a_typed_refusal_is_command_refused_naming_its_type() {
    let err = CodedError::refused(typed_refusal());
    assert_eq!(err.grpc, GrpcCode::FailedPrecondition, "a rejection");
    assert_eq!(err.code, "ROUTER_REASON_COMMAND_REFUSED");
    assert_eq!(
        err.extras,
        [arg("refusal_type", "test.wallet.InsufficientFunds")].into()
    );
    assert_eq!(err.refusal.as_deref(), Some(&typed_refusal()));
}

#[test]
fn undecodable_bytes_name_the_message_type() {
    let err = CodedError::undecodable("io.angzarr.v1.ContextualCommand", &decode_error());
    // The router's own malformed-input error: a bug (owner ruling,
    // 2026-10-29). prost's own text rides as ext_err_txt.
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, RouterReason::AnyDecodeFailed.as_str_name());
    assert_eq!(
        err.extras,
        [
            arg("message_type", "io.angzarr.v1.ContextualCommand"),
            arg("ext_err_txt", &decode_error().to_string()),
        ]
        .into()
    );
}

// Decision 14.37: the fold's own failure is its `cause`, flattened; the
// `fold_message` text entry is gone.
#[test]
fn a_fold_failures_own_failure_is_its_cause_not_text() {
    let cause = pb::Diagnostic {
        code: "BAD_VARINT".to_string(),
        args: [arg("offset", "3")].into_iter().collect(),
    };
    let err = CodedError::fold_failed(
        RouterReason::FoldFailed,
        FoldAt::Page {
            sequence: 7,
            type_url: "/test.Evt",
        },
        "/test.State",
        Some(&cause),
    );
    let extra = |key: &str| err.extras.get(key).map(String::as_str);
    assert_eq!(extra("cause"), Some("BAD_VARINT"), "{:?}", err.extras);
    assert_eq!(extra("cause.offset"), Some("3"), "{:?}", err.extras);
    assert_eq!(extra("fold_message"), None, "no fold text");
}

/// Every value of reasons.proto's `RouterReason` but its proto3 zero
/// (decision 14.44): read from the enum itself, numbered from 1 with no
/// gaps.
fn router_reasons() -> Vec<RouterReason> {
    (1..)
        .map_while(|number| RouterReason::try_from(number).ok())
        .collect()
}

/// The answer the router gives with `reason`, from the constructor that
/// answers it, and the key it carries for it: its code, or for
/// ROUTER_REASON_HANDLER_PANICKED, which travels only as a cause, its
/// `cause`. Exhaustive, so a value added to the enum fails to compile here
/// until the router answers it.
fn answer(reason: RouterReason) -> String {
    let err = match reason {
        RouterReason::Unspecified => unreachable!("proto3's zero is never answered"),
        RouterReason::UnhandledHandlerError => CodedError::unhandled(),
        RouterReason::HandlerPanicked => {
            return CodedError::panicked().extras[extras::CAUSE].clone();
        }
        RouterReason::CommandRefused => CodedError::refused(typed_refusal()),
        RouterReason::FoldFailed | RouterReason::FoldFailedEmitted => CodedError::fold_failed(
            reason,
            FoldAt::Range { first: 1, last: 2 },
            "/a.State",
            None,
        ),
        RouterReason::PersistedEventCorrupt => CodedError::persisted_corrupt("/a.Evt"),
        RouterReason::AnyDecodeFailed => {
            CodedError::undecodable("io.angzarr.v1.ContextualCommand", &decode_error())
        }
        RouterReason::UnknownCommandType | RouterReason::NoFactHandler => {
            CodedError::unknown_type(reason, "/a.Sent")
        }
        RouterReason::NoComponentOfKind => CodedError::unimplemented(reason, []),
        RouterReason::NoRefusalHandler
        | RouterReason::ConflictResolutionUnsupported
        | RouterReason::NoHandlersRegistered
        | RouterReason::DuplicateComponent => CodedError::internal(reason, []),
        // A limit a router may enforce where a client library offers
        // follow-on steps (decision 14.49). This router's dispatch offers
        // none, so never reaches it; the key is still an INTERNAL key of its
        // enum, answered under its full name, with arg limit.
        RouterReason::TooManySteps => {
            CodedError::internal(reason, [("limit".to_string(), "16".to_string())])
        }
    };
    err.code
}

// Decisions 14.42 and 14.44: reasons.proto's RouterReason holds every key
// the router answers, once. The router answers each of its values but the
// zero, under the value's full name, and no other key: none the
// coordinator answers, none retired.
#[test]
fn the_router_answers_exactly_the_non_unspecified_router_reasons() {
    let reasons = router_reasons();
    assert!(
        reasons.contains(&RouterReason::UnknownCommandType)
            && !reasons.contains(&RouterReason::Unspecified),
        "the enum reads: {reasons:?}"
    );
    for reason in &reasons {
        assert_eq!(answer(*reason), reason.as_str_name(), "{reason:?}");
    }
    let unanswered: Vec<String> = every_constructor()
        .into_iter()
        .flat_map(|(name, err)| {
            let keys =
                std::iter::once(err.code.clone()).chain(err.extras.get(extras::CAUSE).cloned());
            keys.map(move |key| (name, key)).collect::<Vec<_>>()
        })
        .filter(|(_, key)| key.starts_with("ROUTER_REASON_"))
        .filter(|(_, key)| {
            RouterReason::from_str_name(key).is_none_or(|r| r == RouterReason::Unspecified)
        })
        .map(|(name, key)| format!("{name}: {key}"))
        .collect();
    assert!(
        unanswered.is_empty(),
        "router keys outside RouterReason's answered values: {unanswered:?}"
    );
}

#[test]
fn the_retired_keys_are_gone() {
    let kept: Vec<String> = [
        "MALFORMED_REFUSAL_FACT",
        "DUPLICATE_REGISTRATION",
        "NO_HANDLER_REGISTERED",
        // Retired by decision 14.39 with the router's upcaster chain.
        "DUPLICATE_UPCASTER_CHAIN",
        "DUPLICATE_COMMAND_HANDLER",
        // Retired by decision 14.42 with the router's envelope checks.
        "MISSING_COMMAND_BOOK",
        "MISSING_COMMAND_PAGE",
        "MISSING_COMMAND_PAYLOAD",
        "MISSING_EVENT_BOOK_COVER",
        "EMPTY_POLICY_SOURCE",
        "POLICY_SOURCE_OUTSIDE_INPUT_DOMAIN",
        "REFUSAL_FACT_MISSING",
        "REFUSAL_WITHOUT_COMMAND",
        "REFUSAL_WITHOUT_EVENT",
        "REFUSAL_WITHOUT_REFUSED_COMMAND",
        "REFUSED_COMMAND_WITHOUT_ID",
        "REFUSAL_WITHOUT_TRIGGER_STREAM",
        "HOST_STATUS_UNDECODABLE",
        "NULL_ABI_POINTER",
    ]
    .into_iter()
    .flat_map(|key| [key.to_string(), format!("ROUTER_REASON_{key}")])
    .filter(|key| RouterReason::from_str_name(key).is_some())
    .collect();
    assert!(
        kept.is_empty(),
        "decisions 14.37, 14.39 and 14.42 retired these, still in RouterReason: {kept:?}"
    );
}

#[test]
fn every_router_reason_is_a_prefixed_screaming_snake_key_listed_once() {
    let mut seen = std::collections::BTreeSet::new();
    for reason in router_reasons() {
        let code = reason.as_str_name();
        assert!(
            code.strip_prefix("ROUTER_REASON_")
                .is_some_and(|rest| !rest.is_empty())
                && code
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
            "{code:?} is no ROUTER_REASON_ SCREAMING_SNAKE key"
        );
        assert_eq!(RouterReason::from_str_name(code), Some(reason), "{code}");
        assert!(seen.insert(code), "{code} listed twice");
    }
}

// --- the mapping table ------------------------------------------------------------

#[test]
fn persisted_corrupt_is_data_loss() {
    let err = CodedError::persisted_corrupt("type.googleapis.com/test.Evt");
    assert_eq!(err.grpc, GrpcCode::DataLoss);
    assert_eq!(err.code, RouterReason::PersistedEventCorrupt.as_str_name());
    assert_eq!(
        err.extras.get(extras::TYPE_URL).map(String::as_str),
        Some("type.googleapis.com/test.Evt")
    );
}

// Owner ruling 2026-10-19: a fold failing on an accepted event is a
// handler bug, named by the event and the state it folded into; decision
// 14.37 gives it its own key.
#[test]
fn fold_failed_is_internal_naming_the_event_and_state() {
    let err = CodedError::fold_failed(
        RouterReason::FoldFailed,
        FoldAt::Page {
            sequence: 7,
            type_url: "/test.Evt",
        },
        "/test.State",
        None,
    );
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, "ROUTER_REASON_FOLD_FAILED");
    let extra = |key: &str| err.extras.get(key).map(String::as_str);
    assert_eq!(extra(extras::SEQUENCE), Some("7"));
    assert_eq!(extra(extras::TYPE_URL), Some("/test.Evt"));
    assert_eq!(extra(extras::STATE_TYPE_URL), Some("/test.State"));
    assert_eq!(extra(extras::FIRST_SEQUENCE), None);
    assert_eq!(extra(extras::CAUSE), None, "no cause, no key");
}

#[test]
fn an_unlocated_fold_failure_names_the_batchs_range() {
    let err = CodedError::fold_failed(
        RouterReason::FoldFailedEmitted,
        FoldAt::Range { first: 3, last: 5 },
        "",
        None,
    );
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, "ROUTER_REASON_FOLD_FAILED_EMITTED");
    let extra = |key: &str| err.extras.get(key).map(String::as_str);
    assert_eq!(extra(extras::FIRST_SEQUENCE), Some("3"));
    assert_eq!(extra(extras::LAST_SEQUENCE), Some("5"));
    assert_eq!(extra(extras::SEQUENCE), None);
    assert_eq!(extra(extras::STATE_TYPE_URL), Some(""));
}

// A request the router hosts no component for: a misroute or a stale
// deployment, not a malformed payload (C-0670).
#[test]
fn no_component_for_the_request_is_unimplemented() {
    let err = CodedError::unimplemented(RouterReason::NoComponentOfKind, []);
    assert_eq!(
        err.grpc,
        GrpcCode::Unimplemented,
        "misrouted request / stale deployment, not a malformed payload"
    );
    assert_eq!(err.code, "ROUTER_REASON_NO_COMPONENT_OF_KIND");
}

// The router's own errors on malformed input are bugs (owner ruling on
// decision 14.37, 2026-10-29): INTERNAL, never a policy's OnRefused.
#[test]
fn the_routers_own_malformed_input_is_a_bug() {
    crate::test_support::assert_bug(&CodedError::internal(RouterReason::NoRefusalHandler, []));
    crate::test_support::assert_bug(&CodedError::unknown_type(
        RouterReason::UnknownCommandType,
        "/a.B",
    ));
    crate::test_support::assert_bug(&CodedError::undecodable(
        "io.angzarr.v1.EventBook",
        &decode_error(),
    ));
}

#[test]
fn coded_handler_error_passes_through_unchanged() {
    let original = CodedError::persisted_corrupt("type.googleapis.com/test.Evt");
    let mapped = map_handler_error(HandlerError::Coded(original.clone()));
    assert_eq!(mapped, original);
}

// A handler fails in exactly two classes: a business rejection (the
// command is refused, FAILED_PRECONDITION) or a bug (a defect, INTERNAL,
// alerted and dead-lettered).
#[test]
fn a_rejection_is_a_business_refusal() {
    let err = CodedError::rejection(
        BusinessStatus::FailedPrecondition,
        "VALUE_NOT_POSITIVE",
        [arg("input", "0")],
    );
    assert_eq!(err.grpc, GrpcCode::FailedPrecondition);
    assert_eq!(err.code, "VALUE_NOT_POSITIVE");
    assert_eq!(err.extras.get("input").map(String::as_str), Some("0"));
}

// Decision 14.37: a handler's bug goes out as ROUTER_REASON_UNHANDLED_HANDLER_ERROR
// (INTERNAL), the handler's own code and args its cause, flattened as
// `cause=<code>` and `cause.<arg>=<value>`.
#[test]
fn a_bug_is_unhandled_handler_error_with_the_handlers_code_as_its_cause() {
    let err = CodedError::bug("LEDGER_INVARIANT_BROKEN", [arg("balance", "-1")]);
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, RouterReason::UnhandledHandlerError.as_str_name());
    assert_eq!(
        err.extras,
        [
            arg("cause", "LEDGER_INVARIANT_BROKEN"),
            arg("cause.balance", "-1"),
        ]
        .into()
    );
    assert_eq!(extras::CAUSE, "cause");
}

// --- the statuses a handler picks (owner ruling, 2026-10-29) ---------------------

const BUSINESS: [(BusinessStatus, GrpcCode); 6] = [
    (
        BusinessStatus::FailedPrecondition,
        GrpcCode::FailedPrecondition,
    ),
    (BusinessStatus::InvalidArgument, GrpcCode::InvalidArgument),
    (BusinessStatus::NotFound, GrpcCode::NotFound),
    (BusinessStatus::AlreadyExists, GrpcCode::AlreadyExists),
    (BusinessStatus::OutOfRange, GrpcCode::OutOfRange),
    (BusinessStatus::PermissionDenied, GrpcCode::PermissionDenied),
];
const BUGS: [GrpcCode; 3] = [GrpcCode::Internal, GrpcCode::Unknown, GrpcCode::DataLoss];
// Decision 14.37 (owner's rulings, 2026-10-29, latest): four classes.
// ABORTED is a business error returned at once ("re-read and re-decide");
// the router gains DEADLINE_EXCEEDED and RESOURCE_EXHAUSTED, retryable
// bugs beside UNAVAILABLE and UNIMPLEMENTED.
const IMMEDIATE: [GrpcCode; 1] = [GrpcCode::Aborted];
const RETRYABLE: [GrpcCode; 4] = [
    GrpcCode::Unavailable,
    GrpcCode::Unimplemented,
    GrpcCode::DeadlineExceeded,
    GrpcCode::ResourceExhausted,
];
// The statuses a handler may pick for a retryable bug, and their wire codes.
const RETRYABLE_STATUSES: [(RetryableStatus, GrpcCode); 3] = [
    (RetryableStatus::Unavailable, GrpcCode::Unavailable),
    (
        RetryableStatus::DeadlineExceeded,
        GrpcCode::DeadlineExceeded,
    ),
    (
        RetryableStatus::ResourceExhausted,
        GrpcCode::ResourceExhausted,
    ),
];

// The wire numbers are google.rpc.Code's.
#[test]
fn deadline_exceeded_and_resource_exhausted_carry_their_wire_numbers() {
    assert_eq!(GrpcCode::DeadlineExceeded as i32, 4);
    assert_eq!(GrpcCode::ResourceExhausted as i32, 8);
}

#[test]
fn each_business_status_builds_a_refusal_with_that_status() {
    let wrong: Vec<String> = BUSINESS
        .into_iter()
        .filter_map(|(status, grpc)| {
            let err = CodedError::rejection(status, "ORDER_CLOSED", []);
            (err.grpc != grpc).then(|| format!("{status:?} built a refusal with {:?}", err.grpc))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// By type: `rejection` takes a BusinessStatus, so a bug's status does not
// compile (the compile_fail doctest on BusinessStatus). A status a host
// hands over at run time is checked: TryFrom accepts the business set only.
#[test]
fn only_a_business_status_converts_to_a_business_status() {
    for (status, grpc) in BUSINESS {
        assert_eq!(
            BusinessStatus::try_from(grpc),
            Ok(status),
            "{grpc:?} is a business status"
        );
    }
    for grpc in BUGS.into_iter().chain(IMMEDIATE).chain(RETRYABLE) {
        assert_eq!(
            BusinessStatus::try_from(grpc),
            Err(grpc),
            "{grpc:?} is no business status"
        );
    }
}

#[test]
fn a_bug_stays_internal_with_its_cause() {
    let err = CodedError::bug("LEDGER_OFFLINE", [arg("ledger", "main")]);
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, RouterReason::UnhandledHandlerError.as_str_name());
    assert_eq!(
        err.extras,
        [arg("cause", "LEDGER_OFFLINE"), arg("cause.ledger", "main")].into()
    );
}

// Decision 14.37 (owner's rulings, 2026-10-29, latest): a bug the handler
// marks retryable (its ledger service is down) goes out under the status
// the handler PICKS, under ROUTER_REASON_UNHANDLED_HANDLER_ERROR with its own code and
// args as the cause. Never OnRefused.
#[test]
fn a_retryable_bug_goes_out_under_the_status_the_handler_picks() {
    for (status, grpc) in RETRYABLE_STATUSES {
        let err = CodedError::retryable_bug(status, "LEDGER_OFFLINE", [arg("ledger", "main")]);
        assert_eq!(err.grpc, grpc, "{status:?}: the status the handler picked");
        assert_eq!(
            err.code,
            RouterReason::UnhandledHandlerError.as_str_name(),
            "{status:?}"
        );
        assert_eq!(
            err.status_message(),
            RouterReason::UnhandledHandlerError.as_str_name()
        );
        assert_eq!(
            err.extras,
            [arg("cause", "LEDGER_OFFLINE"), arg("cause.ledger", "main")].into(),
            "{status:?}"
        );
    }
}

// A handler's retryable bug crosses the handler boundary unchanged: the
// router neither reclasses it nor folds it into a fatal bug.
#[test]
fn a_handlers_retryable_bug_keeps_its_status_through_the_boundary() {
    let err = call_handler::<()>(|| {
        Err(CodedError::retryable_bug(
            RetryableStatus::ResourceExhausted,
            "LEDGER_OFFLINE",
            [arg("ledger", "main")],
        )
        .into())
    })
    .expect_err("the handler failed");
    assert_eq!(err.grpc, GrpcCode::ResourceExhausted);
    assert_eq!(err.code, RouterReason::UnhandledHandlerError.as_str_name());
    assert_eq!(
        err.extras.get("cause").map(String::as_str),
        Some("LEDGER_OFFLINE")
    );
}

// Decision 14.37 (owner's rulings, 2026-10-29, latest): a handler may
// choose the immediate-return business class, "re-read and re-decide":
// ABORTED under its OWN key and args, returned at once, never OnRefused.
#[test]
fn a_handlers_immediate_rejection_is_aborted_under_its_own_code() {
    let err = CodedError::immediate("PRICE_MOVED", [arg("item", "apple")]);
    assert_eq!(err.grpc, GrpcCode::Aborted);
    assert_eq!(err.code, "PRICE_MOVED");
    assert_eq!(err.status_message(), "PRICE_MOVED");
    assert_eq!(err.extras, [arg("item", "apple")].into());
}

// A value that would not parse back is quoted (decision 14.37's display
// rule, owner ruling 2026-10-29): one empty, or holding whitespace, `=`,
// `"` or `\`, is written double-quoted, its `"` and `\` escaped with a
// backslash; any other value stays bare.
#[test]
fn a_value_that_would_not_parse_back_is_quoted() {
    let err = CodedError::bug(
        "STORE_DOWN",
        [
            arg("ext_err_txt", "database is locked"),
            arg("empty", ""),
            arg("pair", "k=v"),
            arg("said", "say \"hi\""),
            arg("path", "C:\\dir"),
            arg("tab", "a\tb"),
            arg("bare", "account/7-x_y.z"),
        ],
    );
    assert_eq!(
        err.to_string(),
        "ROUTER_REASON_UNHANDLED_HANDLER_ERROR cause=STORE_DOWN cause.bare=account/7-x_y.z \
         cause.empty=\"\" cause.ext_err_txt=\"database is locked\" cause.pair=\"k=v\" \
         cause.path=\"C:\\\\dir\" cause.said=\"say \\\"hi\\\"\" cause.tab=\"a\tb\""
    );
}
