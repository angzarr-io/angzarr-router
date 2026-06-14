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

#[test]
fn no_handler_registered_is_unimplemented() {
    let err = CodedError::invalid_argument(codes::NO_HANDLER_REGISTERED, messages::UNKNOWN_COMMAND, []);
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
        codes::NOTIFICATION_DECODE_FAILED,
        codes::REJECTION_NOTIFICATION_DECODE_FAILED,
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

#[test]
fn rejections_keep_their_grpc_code() {
    let err = CodedError::rejection_precondition_failed("VALUE_NOT_POSITIVE", "value must be positive", []);
    assert_eq!(err.grpc, GrpcCode::FailedPrecondition);
    assert_eq!(err.code, "VALUE_NOT_POSITIVE");

    let err = CodedError::rejection_invalid_argument("VALUE_EMPTY", "value must not be empty", []);
    assert_eq!(err.grpc, GrpcCode::InvalidArgument);

    let err = CodedError::rejection_not_found("ENTITY_NOT_FOUND", "entity not found", []);
    assert_eq!(err.grpc, GrpcCode::NotFound);
}

#[test]
fn rejection_details_travel_as_extras() {
    let err = CodedError::rejection_precondition_failed(
        "VALUE_NOT_POSITIVE",
        "value must be positive",
        [("input".to_string(), "0".to_string())],
    );
    assert_eq!(err.extras.get("input").map(String::as_str), Some("0"));
}
