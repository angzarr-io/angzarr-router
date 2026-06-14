//! Buffer-ownership and error-serialization rules, each pinned: the
//! allocator pair round-trips, and coded errors survive the
//! Status/ErrorInfo crossing in both directions.

use prost::Message;

use angzarr_router::error::{CodedError, GrpcCode};

use super::*;

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
fn coded_error_survives_the_status_crossing() {
    let original = CodedError {
        code: "VALUE_NOT_POSITIVE".to_string(),
        message: "value must be positive".to_string(),
        extras: [("input".to_string(), "0".to_string())].into(),
        grpc: GrpcCode::FailedPrecondition,
    };
    let bytes = coded_to_status_bytes(original.clone());

    let status = Status::decode(bytes.as_slice()).expect("status");
    assert_eq!(status.code, 9);
    assert_eq!(status.message, "value must be positive");

    let decoded = status_to_coded(Some(&bytes), -9);
    assert_eq!(decoded, original);
}

#[test]
fn status_domain_is_angzarr_io() {
    let bytes = coded_to_status_bytes(CodedError::unhandled("boom"));
    let status = Status::decode(bytes.as_slice()).expect("status");
    let info = ErrorInfo::decode(status.details[0].value.as_slice()).expect("info");
    assert_eq!(info.domain, "angzarr.io");
    assert_eq!(info.reason, "UNHANDLED_HANDLER_ERROR");
}

#[test]
fn missing_status_payload_falls_back_to_return_code() {
    let err = status_to_coded(None, -9);
    assert_eq!(err.grpc, GrpcCode::FailedPrecondition);
    assert!(err.code.is_empty());
}

#[test]
fn unknown_wire_codes_degrade_to_internal() {
    assert_eq!(grpc_from_wire(3), GrpcCode::InvalidArgument);
    assert_eq!(grpc_from_wire(5), GrpcCode::NotFound);
    assert_eq!(grpc_from_wire(9), GrpcCode::FailedPrecondition);
    assert_eq!(grpc_from_wire(12), GrpcCode::Unimplemented);
    assert_eq!(grpc_from_wire(15), GrpcCode::DataLoss);
    assert_eq!(grpc_from_wire(42), GrpcCode::Internal);
}
