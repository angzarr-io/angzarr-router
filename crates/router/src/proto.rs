//! The framework wire types: angzarr-project's angzarr-proto crate
//! (io.angzarr.v1 books, covers, pages, options and sererr), plus the
//! request/response messages of the service protos angzarr-proto does not
//! compile (CommandHandlerResponse, CommandResponse, SagaResponse, Refusal,
//! OnRefusedResponse), generated against its types.
//!
//! Module nesting mirrors proto package segments exactly so prost's
//! cross-package `super::` references resolve.

pub use angzarr_proto::sererr;

pub mod io {
    pub mod angzarr {
        // The spec's proto comments are prose, not rustdoc lists.
        #[allow(clippy::doc_overindented_list_items, clippy::doc_lazy_continuation)]
        pub mod v1 {
            pub use angzarr_proto::v1::*;
            include!(concat!(env!("OUT_DIR"), "/io.angzarr.v1.rs"));
        }
    }
}
