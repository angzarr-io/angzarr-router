//! Generated ABI-internal payloads (descriptors, aux protos) and the
//! vendored google.rpc error shapes.

pub mod angzarr {
    pub mod router {
        pub mod ffi {
            pub mod v1 {
                include!(concat!(env!("OUT_DIR"), "/angzarr.router.ffi.v1.rs"));
            }
        }
    }
}

pub mod google {
    pub mod rpc {
        include!(concat!(env!("OUT_DIR"), "/google.rpc.rs"));
    }
}
