//! The single error-mapping table, pinned: these wire codes are the
//! adapter-level contracts client-go's engine_test.go checks through its
//! gRPC handler (DATA_LOSS for corrupt persisted events, UNIMPLEMENTED
//! for a request the router hosts no component for, INTERNAL +
//! UNHANDLED_HANDLER_ERROR for unclassified failures), asserted here
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
            CodedError::internal(codes::MISSING_COMMAND_BOOK, []),
        ),
        (
            "internal with args",
            CodedError::internal(
                codes::MISSING_RULES_VERSION,
                [arg(extras::COMPONENT, "order")],
            ),
        ),
        (
            "unimplemented",
            CodedError::unimplemented("NO_COMPONENT_OF_KIND", []),
        ),
        (
            "unknown_type",
            CodedError::unknown_type("UNKNOWN_COMMAND_TYPE", "/a.Sent", "/a.Upcast", []),
        ),
        (
            "undecodable",
            CodedError::undecodable("io.angzarr.v1.ContextualCommand"),
        ),
        ("stale", CodedError::stale(codes::CONFLICT_UNRESOLVED, [])),
        ("persisted_corrupt", CodedError::persisted_corrupt("/a.Evt")),
        (
            "fold_failed",
            CodedError::fold_failed(
                "FOLD_FAILED",
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
            CodedError::retryable_bug("LEDGER_OFFLINE", [arg("ledger", "main")]),
        ),
        (
            "internal, a policy's",
            CodedError::internal(codes::POLICY_COMMAND_NO_ROOT, []),
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

// In process too, an error shows as its code then its args, ` name=value`
// each (owner's clarification of 2026-10-29).
#[test]
fn every_error_displays_as_its_code_then_its_args() {
    let wrong: Vec<String> = every_constructor()
        .into_iter()
        .filter_map(|(name, err)| {
            let expected: String = std::iter::once(err.code.clone())
                .chain(err.extras.iter().map(|(k, v)| format!(" {k}={v}")))
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
        "UNHANDLED_HANDLER_ERROR cause=LEDGER_OFFLINE cause.ledger=main"
    );
}

// A handler's unclassified error is a bug with no cause: what escaped stays
// in the handler's own log (C-0669).
#[test]
fn an_unclassified_handler_errors_text_never_leaves_the_handler() {
    let err = map_handler_error(HandlerError::Other("connection reset".to_string()));
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
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

// A panic's cause is HANDLER_PANICKED; its message stays in the handler's
// log (decision 14.37).
#[test]
fn a_panic_is_unhandled_handler_error_caused_by_handler_panicked() {
    let err = CodedError::panicked();
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
    assert_eq!(
        err.extras,
        [arg("cause", "HANDLER_PANICKED")].into(),
        "the panic's cause"
    );
}

// Owner's ruling on 14.37: a typed refusal's reason is COMMAND_REFUSED,
// `refusal_type` its object's fully-qualified type, the object in the
// status details.
#[test]
fn a_typed_refusal_is_command_refused_naming_its_type() {
    let err = CodedError::refused(typed_refusal());
    assert_eq!(err.grpc, GrpcCode::FailedPrecondition, "a rejection");
    assert_eq!(err.code, "COMMAND_REFUSED");
    assert_eq!(
        err.extras,
        [arg("refusal_type", "test.wallet.InsufficientFunds")].into()
    );
    assert_eq!(err.refusal.as_deref(), Some(&typed_refusal()));
    assert_eq!(
        err.refusal_response(),
        Some(pb::RefusalResponse {
            reason: Some(pb::refusal_response::Reason::Refusal(typed_refusal())),
        })
    );
}

#[test]
fn undecodable_bytes_name_the_message_type() {
    let err = CodedError::undecodable("io.angzarr.v1.ContextualCommand");
    // The router's own malformed-input error: a bug (owner ruling,
    // 2026-10-29).
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, codes::ANY_DECODE_FAILED);
    assert_eq!(
        err.extras,
        [arg("message_type", "io.angzarr.v1.ContextualCommand")].into()
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
        "FOLD_FAILED",
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

/// Decision 14.37's reason inventory for the router (its table, and the
/// codes that keep their key).
const INVENTORY_14_37: &[&str] = &[
    "FOLD_FAILED",
    "FOLD_FAILED_EMITTED",
    "UNHANDLED_HANDLER_ERROR",
    "HANDLER_PANICKED",
    "UNKNOWN_COMMAND_TYPE",
    "NO_COMPONENT_OF_KIND",
    "POLICY_SOURCE_OUTSIDE_INPUT_DOMAIN",
    "NO_FACT_HANDLER",
    "NO_REFUSAL_HANDLER",
    "DUPLICATE_COMPONENT",
    "DUPLICATE_UPCASTER_CHAIN",
    "REFUSAL_WITHOUT_COMMAND",
    "REFUSAL_WITHOUT_EVENT",
    "REFUSAL_FACT_MISSING",
    "REFUSAL_FACT_ADDRESSED",
    "REFUSAL_FACT_NO_PAGE",
    "REFUSAL_FACT_NOT_A_FACT",
    "REFUSAL_FACT_IDENTIFIED",
    "REFUSAL_WITHOUT_REFUSED_COMMAND",
    "REFUSED_COMMAND_WITHOUT_ID",
    "REFUSAL_WITHOUT_TRIGGER_STREAM",
    "ANY_DECODE_FAILED",
    "HOST_STATUS_UNDECODABLE",
    "NULL_ABI_POINTER",
    "COMMAND_REFUSED",
    "MISSING_RULES_VERSION",
    "MISSING_STATE_TYPE",
    "STARTING_TYPES_ON_NON_PM",
    "DUPLICATE_COMMAND_HANDLER",
    "NO_HANDLERS_REGISTERED",
    "MISSING_COMMAND_BOOK",
    "MISSING_COMMAND_PAGE",
    "MISSING_COMMAND_PAYLOAD",
    "MISSING_EVENT_BOOK_COVER",
    "EMPTY_POLICY_SOURCE",
    "PERSISTED_EVENT_CORRUPT",
    "CONFLICT_UNRESOLVED",
    "CONFLICT_OVERLAP",
    "CONFLICT_RESOLUTION_UNSUPPORTED",
    "POLICY_COMMAND_OUTSIDE_DESTINATION",
    "POLICY_COMMAND_OUTSIDE_EDITION",
    "POLICY_COMMAND_NO_ROOT",
    "POLICY_COMMAND_OTHER_CORRELATION",
    "PM_NO_INSTANCE",
    "REFUSAL_FACT_WRONG_STREAM",
];

#[test]
fn every_key_of_the_14_37_inventory_is_a_router_code() {
    let missing: Vec<&str> = INVENTORY_14_37
        .iter()
        .copied()
        .filter(|key| !codes::ALL.contains(key))
        .collect();
    assert!(
        missing.is_empty(),
        "decision 14.37's keys missing from codes::ALL: {missing:?}"
    );
}

#[test]
fn the_retired_keys_are_gone() {
    let kept: Vec<&str> = [
        "MALFORMED_REFUSAL_FACT",
        "DUPLICATE_REGISTRATION",
        "NO_HANDLER_REGISTERED",
    ]
    .into_iter()
    .filter(|key| codes::ALL.contains(key))
    .collect();
    assert!(
        kept.is_empty(),
        "decision 14.37 retired these, still in codes::ALL: {kept:?}"
    );
}

#[test]
fn every_router_code_is_a_screaming_snake_key_listed_once() {
    let mut seen = std::collections::BTreeSet::new();
    for code in codes::ALL {
        assert!(
            !code.is_empty()
                && code
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
            "{code:?} is no SCREAMING_SNAKE key"
        );
        assert!(seen.insert(code), "{code} listed twice");
    }
}

// --- the mapping table ------------------------------------------------------------

#[test]
fn persisted_corrupt_is_data_loss() {
    let err = CodedError::persisted_corrupt("type.googleapis.com/test.Evt");
    assert_eq!(err.grpc, GrpcCode::DataLoss);
    assert_eq!(err.code, codes::PERSISTED_EVENT_CORRUPT);
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
        "FOLD_FAILED",
        FoldAt::Page {
            sequence: 7,
            type_url: "/test.Evt",
        },
        "/test.State",
        None,
    );
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, "FOLD_FAILED");
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
        "FOLD_FAILED_EMITTED",
        FoldAt::Range { first: 3, last: 5 },
        "",
        None,
    );
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, "FOLD_FAILED_EMITTED");
    let extra = |key: &str| err.extras.get(key).map(String::as_str);
    assert_eq!(extra(extras::FIRST_SEQUENCE), Some("3"));
    assert_eq!(extra(extras::LAST_SEQUENCE), Some("5"));
    assert_eq!(extra(extras::SEQUENCE), None);
    assert_eq!(extra(extras::STATE_TYPE_URL), Some(""));
}

// A request the router hosts no component for, or a policy source from
// outside its input domain: a misroute or a stale deployment, not a
// malformed payload (C-0670).
#[test]
fn no_component_for_the_request_is_unimplemented() {
    for code in ["NO_COMPONENT_OF_KIND", "POLICY_SOURCE_OUTSIDE_INPUT_DOMAIN"] {
        assert_eq!(
            CodedError::unimplemented(code, []).grpc,
            GrpcCode::Unimplemented,
            "{code}: misrouted request / stale deployment, not a malformed payload"
        );
    }
}

// The router's own errors on malformed input are bugs (owner ruling on
// decision 14.37, 2026-10-29): INTERNAL, never a policy's OnRefused.
#[test]
fn the_routers_own_malformed_input_is_a_bug() {
    for code in [
        codes::MISSING_COMMAND_BOOK,
        codes::MISSING_COMMAND_PAGE,
        codes::MISSING_COMMAND_PAYLOAD,
    ] {
        crate::test_support::assert_bug(&CodedError::internal(code, []));
    }
    crate::test_support::assert_bug(&CodedError::unknown_type(
        "UNKNOWN_COMMAND_TYPE",
        "/a.B",
        "/a.B",
        [],
    ));
    crate::test_support::assert_bug(&CodedError::undecodable("io.angzarr.v1.EventBook"));
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

// Decision 14.37: a handler's bug goes out as UNHANDLED_HANDLER_ERROR
// (INTERNAL), the handler's own code and args its cause, flattened as
// `cause=<code>` and `cause.<arg>=<value>`.
#[test]
fn a_bug_is_unhandled_handler_error_with_the_handlers_code_as_its_cause() {
    let err = CodedError::bug("LEDGER_INVARIANT_BROKEN", [arg("balance", "-1")]);
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
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

// --- the refused policy's view (types.proto RefusalResponse) -------------------

// An untyped rejection reaches the refused policy as its key and args.
#[test]
fn an_untyped_rejection_reaches_the_policy_as_its_diagnostic() {
    let err = CodedError::rejection(
        BusinessStatus::FailedPrecondition,
        "CLOSED",
        [arg("wallet", "w-1")],
    );
    assert_eq!(
        err.refusal_response(),
        Some(pb::RefusalResponse {
            reason: Some(pb::refusal_response::Reason::Diagnostic(pb::Diagnostic {
                code: "CLOSED".to_string(),
                args: [arg("wallet", "w-1")].into_iter().collect(),
            })),
        })
    );
}

// Only a business rejection (FAILED_PRECONDITION) reaches OnRefused; anything
// else is retried or dead-lettered as a bug.
#[test]
fn only_a_business_rejection_reaches_on_refused() {
    for err in [
        CodedError::bug("BROKEN", []),
        CodedError::unhandled(),
        CodedError::panicked(),
        CodedError::stale(codes::CONFLICT_OVERLAP, []),
    ] {
        assert_eq!(err.refusal_response(), None, "{}", err.code);
    }
}

// --- the two classes, by status (owner ruling, 2026-10-29) ---------------------

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
// Decision 14.37 (owner's rulings, 2026-10-29, late): the router gains
// DEADLINE_EXCEEDED and RESOURCE_EXHAUSTED, both a bug's retryable branch.
const RETRYABLE: [GrpcCode; 5] = [
    GrpcCode::Unavailable,
    GrpcCode::Aborted,
    GrpcCode::Unimplemented,
    GrpcCode::DeadlineExceeded,
    GrpcCode::ResourceExhausted,
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
    for grpc in BUGS.into_iter().chain(RETRYABLE) {
        assert_eq!(
            BusinessStatus::try_from(grpc),
            Err(grpc),
            "{grpc:?} is no business status"
        );
    }
}

#[test]
fn the_status_decides_the_class() {
    let wrong: Vec<String> = BUSINESS
        .iter()
        .map(|(_, grpc)| (*grpc, ErrorClass::BusinessRefusal))
        // A bug is fatal or retryable (owner's rulings, 2026-10-29, late).
        .chain(BUGS.map(|grpc| (grpc, ErrorClass::FatalBug)))
        .chain(RETRYABLE.map(|grpc| (grpc, ErrorClass::RetryableBug)))
        .filter(|(grpc, class)| {
            grpc.class() != *class
                || grpc.is_business_refusal() != (*class == ErrorClass::BusinessRefusal)
        })
        .map(|(grpc, class)| format!("{grpc:?}: {:?}, expected {class:?}", grpc.class()))
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// Every business refusal reaches the refused policy's OnRefused, whatever
// its business status.
#[test]
fn every_business_refusal_reaches_on_refused() {
    for (status, _) in BUSINESS {
        let err = CodedError::rejection(status, "ORDER_CLOSED", []);
        assert!(
            err.refusal_response().is_some(),
            "{status:?} reaches OnRefused"
        );
    }
}

#[test]
fn a_bug_stays_internal_with_its_cause() {
    let err = CodedError::bug("LEDGER_OFFLINE", [arg("ledger", "main")]);
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.grpc.class(), ErrorClass::FatalBug);
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
    assert_eq!(
        err.extras,
        [arg("cause", "LEDGER_OFFLINE"), arg("cause.ledger", "main")].into()
    );
}

// Decision 14.37, PROPOSAL (a) of 2026-10-29 (late): a bug the handler
// marks retryable (its ledger service is down) goes out as a retryable
// STATUS, UNAVAILABLE, under UNHANDLED_HANDLER_ERROR with its own code and
// args as the cause; the status alone decides the class. Never OnRefused.
#[test]
fn a_retryable_bug_is_unavailable_with_its_cause() {
    let err = CodedError::retryable_bug("LEDGER_OFFLINE", [arg("ledger", "main")]);
    assert_eq!(err.grpc, GrpcCode::Unavailable, "the retryable status");
    assert_eq!(err.grpc.class(), ErrorClass::RetryableBug);
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
    assert_eq!(err.status_message(), codes::UNHANDLED_HANDLER_ERROR);
    assert_eq!(
        err.extras,
        [arg("cause", "LEDGER_OFFLINE"), arg("cause.ledger", "main")].into()
    );
    assert_eq!(err.refusal_response(), None, "never a policy's OnRefused");
}

// A handler's retryable bug crosses the handler boundary unchanged: the
// router neither reclasses it nor folds it into a fatal bug.
#[test]
fn a_handlers_retryable_bug_keeps_its_status_through_the_boundary() {
    let err = call_handler::<()>(|| {
        Err(CodedError::retryable_bug("LEDGER_OFFLINE", [arg("ledger", "main")]).into())
    })
    .expect_err("the handler failed");
    assert_eq!(err.grpc, GrpcCode::Unavailable);
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
    assert_eq!(
        err.extras.get("cause").map(String::as_str),
        Some("LEDGER_OFFLINE")
    );
}
