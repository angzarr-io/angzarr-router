//! The single error-mapping table, pinned: these wire codes are the
//! adapter-level contracts client-go's engine_test.go checks through its
//! gRPC handler (DATA_LOSS for corrupt persisted events, UNIMPLEMENTED
//! for unknown commands, INTERNAL + UNHANDLED_HANDLER_ERROR for
//! unclassified failures), asserted here directly on the coded form the
//! FFI boundary will serialize.

use super::*;

#[test]
fn persisted_corrupt_is_data_loss() {
    let err = CodedError::persisted_corrupt("type.googleapis.com/test.Evt");
    assert_eq!(err.grpc, GrpcCode::DataLoss);
    assert_eq!(err.code, codes::PERSISTED_EVENT_CORRUPT);
    assert_eq!(err.message, messages::PERSISTED_EVENT_CORRUPT);
    assert_eq!(
        err.extras.get(extras::TYPE_URL).map(String::as_str),
        Some("type.googleapis.com/test.Evt")
    );
}

// Owner ruling 2026-10-19: a fold failing on an accepted event is a
// handler bug, named by the event and the state it folded into.
#[test]
fn fold_failed_is_internal_naming_the_event_and_state() {
    let err = CodedError::fold_failed(
        messages::FOLD_FAILED,
        FoldAt::Page {
            sequence: 7,
            type_url: "/test.Evt",
        },
        "/test.State",
        "bad varint",
    );
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
    assert_eq!(err.message, messages::FOLD_FAILED);
    let extra = |key: &str| err.extras.get(key).map(String::as_str);
    assert_eq!(extra(extras::SEQUENCE), Some("7"));
    assert_eq!(extra(extras::TYPE_URL), Some("/test.Evt"));
    assert_eq!(extra(extras::STATE_TYPE_URL), Some("/test.State"));
    assert_eq!(extra(extras::FOLD_MESSAGE), Some("bad varint"));
    assert_eq!(extra(extras::FIRST_SEQUENCE), None);
}

#[test]
fn an_unlocated_fold_failure_names_the_batchs_range() {
    let err = CodedError::fold_failed(
        messages::FOLD_FAILED_EMITTED,
        FoldAt::Range { first: 3, last: 5 },
        "",
        "",
    );
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.message, messages::FOLD_FAILED_EMITTED);
    let extra = |key: &str| err.extras.get(key).map(String::as_str);
    assert_eq!(extra(extras::FIRST_SEQUENCE), Some("3"));
    assert_eq!(extra(extras::LAST_SEQUENCE), Some("5"));
    assert_eq!(extra(extras::SEQUENCE), None);
    assert_eq!(extra(extras::STATE_TYPE_URL), Some(""));
    assert_eq!(extra(extras::FOLD_MESSAGE), None, "no text, no key");
}

#[test]
fn no_handler_registered_is_unimplemented() {
    let err =
        CodedError::invalid_argument(codes::NO_HANDLER_REGISTERED, messages::UNKNOWN_COMMAND, []);
    assert_eq!(
        err.grpc,
        GrpcCode::Unimplemented,
        "misrouted command / stale deployment, not a malformed payload"
    );
}

#[test]
fn other_coded_client_errors_are_invalid_argument() {
    for code in [
        codes::MISSING_COMMAND_BOOK,
        codes::MISSING_COMMAND_PAGE,
        codes::MISSING_COMMAND_PAYLOAD,
    ] {
        let err = CodedError::invalid_argument(code, "x", []);
        assert_eq!(err.grpc, GrpcCode::InvalidArgument, "code {code}");
    }
}

#[test]
fn unclassified_handler_error_is_internal_unhandled() {
    let err = map_handler_error(HandlerError::Other("boom".to_string()));
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
    assert_eq!(err.message, "boom");
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
    let err = CodedError::rejection("VALUE_NOT_POSITIVE", "value must be positive", []);
    assert_eq!(err.grpc, GrpcCode::FailedPrecondition);
    assert_eq!(err.code, "VALUE_NOT_POSITIVE");
    assert_eq!(err.message, "value must be positive");
}

#[test]
fn a_bug_is_internal() {
    let err = CodedError::bug(
        "LEDGER_INVARIANT_BROKEN",
        "the ledger went negative",
        [("balance".to_string(), "-1".to_string())],
    );
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert_eq!(err.code, "LEDGER_INVARIANT_BROKEN");
    assert_eq!(err.message, "the ledger went negative");
    assert_eq!(err.extras.get("balance").map(String::as_str), Some("-1"));
}

#[test]
fn rejection_details_travel_as_extras() {
    let err = CodedError::rejection(
        "VALUE_NOT_POSITIVE",
        "value must be positive",
        [("input".to_string(), "0".to_string())],
    );
    assert_eq!(err.extras.get("input").map(String::as_str), Some("0"));
}

// --- typed refusals (decision 14.31; types.proto RefusalResponse) ---------------

fn typed_refusal() -> prost_types::Any {
    prost_types::Any {
        type_url: "/test.wallet.InsufficientFunds".to_string(),
        value: vec![8, 100, 16, 150],
    }
}

#[test]
fn a_business_rejection_becomes_the_policies_refusal_response() {
    let err = CodedError::rejection("INSUFFICIENT_FUNDS", "holds 100", [])
        .with_refusal(typed_refusal());
    assert_eq!(err.grpc, GrpcCode::FailedPrecondition, "still a rejection");
    assert_eq!(
        err.refusal_response(),
        Some(angzarr_proto::v1::RefusalResponse {
            refusal: Some(typed_refusal()),
            message: "holds 100".to_string(),
        })
    );
}

#[test]
fn a_rejection_without_a_typed_refusal_still_reaches_the_policy() {
    let err = CodedError::rejection("CLOSED", "the wallet is closed", []);
    assert_eq!(
        err.refusal_response(),
        Some(angzarr_proto::v1::RefusalResponse {
            refusal: None,
            message: "the wallet is closed".to_string(),
        })
    );
}

// Only a business rejection (FAILED_PRECONDITION) reaches OnRefused; anything
// else is retried or dead-lettered as a bug.
#[test]
fn only_a_business_rejection_reaches_on_refused() {
    for err in [
        CodedError::bug("BROKEN", "broken", []),
        CodedError::unhandled("boom"),
        CodedError::stale(codes::CONFLICT_OVERLAP, messages::CONFLICT_OVERLAP, []),
    ] {
        assert_eq!(err.refusal_response(), None, "{}", err.code);
    }
}
