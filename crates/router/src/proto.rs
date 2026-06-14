//! Generated framework types (books, covers, envelopes).
//!
//! Module nesting mirrors proto package segments exactly so prost's
//! cross-package `super::` references resolve (types.proto pulls
//! sererr.v1 in via DLQ detail messages).

pub mod sererr {
    pub mod v1 {
        include!(concat!(env!("OUT_DIR"), "/sererr.v1.rs"));
    }
}

pub mod io {
    pub mod angzarr {
        pub mod v1 {
            include!(concat!(env!("OUT_DIR"), "/io.angzarr.v1.rs"));
        }
    }
}
