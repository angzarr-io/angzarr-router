//! Buffer-ownership and error-serialization rules, each pinned: the
//! allocator pair round-trips, and coded errors cross the Status/ErrorInfo
//! boundary exactly, as keys and args, in both directions (decision 14.37).

use prost::Message;

use angzarr_router::error::{codes, extras, CodedError, GrpcCode, RetryableStatus};

use super::*;
use crate::proto::google::rpc::{ErrorInfo, Status};

#[test]
fn alloc_release_roundtrip() {
    let ptr = alloc_bytes(16);
    assert!(!ptr.is_null());
    unsafe {
        std::ptr::write_bytes(ptr, 0xAB, 16);
        free_bytes(ptr, 16);
    }
    // Zero-length allocation is null and releasing it is a no-op.
    assert!(alloc_bytes(0).is_null());
    unsafe { free_bytes(std::ptr::null_mut(), 0) };
}

#[test]
fn fill_and_consume_out_transfer_ownership() {
    let mut out = AngzarrBuf {
        data: std::ptr::null_mut(),
        len: 0,
    };
    fill_out(&mut out, b"payload");
    assert_eq!(out.len, 7);
    let bytes = consume_out(&mut out).expect("bytes");
    assert_eq!(bytes, b"payload");
    assert!(out.data.is_null(), "consume must clear the buffer");
    assert!(consume_out(&mut out).is_none());
}

#[test]
fn the_abi_version_is_abi_proto_s_fnv1a_hash() {
    assert_eq!(fnv1a32(b""), 0x811c_9dc5);
    assert_eq!(fnv1a32(b"foobar"), 0xbf9c_f968);
    assert_eq!(
        ABI_VERSION,
        fnv1a32(include_bytes!(
            "../../../proto/io/angzarr/router/ffi/v1/abi.proto"
        ))
    );
}

#[test]
fn a_coded_error_crosses_as_its_key_and_args_with_no_english() {
    let original = CodedError::rejection(
        angzarr_router::error::BusinessStatus::OutOfRange,
        "VALUE_TOO_LARGE",
        [("limit".to_string(), "10".to_string())],
    );
    let bytes = coded_to_status_bytes(&original);

    let status = Status::decode(bytes.as_slice()).expect("status");
    assert_eq!(status.code, GrpcCode::OutOfRange as i32);
    assert_eq!(
        status.message, "VALUE_TOO_LARGE",
        "the status message is the key"
    );
    let info = ErrorInfo::decode(status.details[0].value.as_slice()).expect("info");
    assert_eq!(info.domain, "angzarr.io");
    assert_eq!(info.reason, "VALUE_TOO_LARGE");
    assert_eq!(info.metadata.get("limit").map(String::as_str), Some("10"));

    assert_eq!(status_to_coded(Some(&bytes), -11), original);
}

#[test]
fn a_retryable_bug_crosses_as_its_chosen_status() {
    let original = CodedError::retryable_bug(
        RetryableStatus::ResourceExhausted,
        "QUOTA_HIT",
        [("quota".to_string(), "writes".to_string())],
    );
    let decoded = status_to_coded(Some(&coded_to_status_bytes(&original)), -8);
    assert_eq!(decoded.grpc, GrpcCode::ResourceExhausted);
    assert_eq!(decoded.code, codes::UNHANDLED_HANDLER_ERROR);
    assert_eq!(decoded, original);
}

#[test]
fn a_typed_refusal_object_crosses_as_a_second_detail() {
    let refusal = prost_types::Any {
        type_url: "/orders.v1.InsufficientFunds".to_string(),
        value: vec![8, 5],
    };
    let original = CodedError::refused(refusal.clone());
    let bytes = coded_to_status_bytes(&original);
    let status = Status::decode(bytes.as_slice()).expect("status");
    assert_eq!(status.details.len(), 2);
    assert_eq!(status.details[1], refusal);
    let decoded = status_to_coded(Some(&bytes), -9);
    assert_eq!(decoded.refusal.as_deref(), Some(&refusal));
    assert_eq!(decoded, original);
}

#[test]
fn undecodable_host_status_bytes_are_host_status_undecodable_with_prost_s_text() {
    let err = status_to_coded(Some(b"\xff\xff\xff"), -9);
    assert_eq!(err.code, codes::HOST_STATUS_UNDECODABLE);
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert!(
        err.extras
            .get(extras::EXT_ERR_TXT)
            .is_some_and(|text| !text.is_empty()),
        "prost's own text travels as ext_err_txt: {err:?}"
    );
    assert_eq!(err.extras.len(), 1, "no other args: {err:?}");
}

#[test]
fn a_failure_without_status_bytes_is_unhandled_under_the_returned_status() {
    let err = status_to_coded(None, -14);
    assert_eq!(err.code, codes::UNHANDLED_HANDLER_ERROR);
    assert_eq!(err.grpc, GrpcCode::Unavailable);
    assert!(err.extras.is_empty());
}

#[test]
fn every_status_the_router_knows_crosses_as_itself_and_the_rest_degrade_to_internal() {
    for code in [
        GrpcCode::Unknown,
        GrpcCode::InvalidArgument,
        GrpcCode::DeadlineExceeded,
        GrpcCode::NotFound,
        GrpcCode::AlreadyExists,
        GrpcCode::PermissionDenied,
        GrpcCode::ResourceExhausted,
        GrpcCode::FailedPrecondition,
        GrpcCode::Aborted,
        GrpcCode::OutOfRange,
        GrpcCode::Unimplemented,
        GrpcCode::Internal,
        GrpcCode::Unavailable,
        GrpcCode::DataLoss,
    ] {
        assert_eq!(grpc_from_wire(code as i32), code);
    }
    for other in [0, 1, 16, 17, 42, -3] {
        assert_eq!(
            grpc_from_wire(other),
            GrpcCode::Internal,
            "wire code {other}"
        );
    }
}
