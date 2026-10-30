//! Generated ABI-internal payloads (descriptors, aux protos) and the
//! vendored google.rpc error shapes.

pub mod io {
    pub mod angzarr {
        pub mod router {
            pub mod ffi {
                // DispatchRequest's arms are whole spec messages, decoded
                // once per call; boxing them buys nothing.
                #[allow(clippy::large_enum_variant)]
                pub mod v1 {
                    include!(concat!(env!("OUT_DIR"), "/io.angzarr.router.ffi.v1.rs"));
                }
            }
        }
    }
}

pub mod google {
    pub mod rpc {
        include!(concat!(env!("OUT_DIR"), "/google.rpc.rs"));
    }
}
