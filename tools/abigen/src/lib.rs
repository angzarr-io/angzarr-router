//! angzarr-abigen: generates the router C ABI's bindings from its one
//! definition, `proto/io/angzarr/router/ffi/v1/abi.proto` (owner's ruling,
//! 2026-10-29: a generic, generated C ABI; the Python cffi layer generated,
//! not hand-written).
//!
//! What it reads: abi.proto's compiled descriptor ([`DESCRIPTOR_SET`]) for
//! the `Role` enum and its `<Role>Call` / `<Role>Return` messages
//! ([`roles`]), abi.proto's bytes for the ABI version ([`abi_version`]),
//! and [`SYMBOLS`], the C entry points (the one part of the ABI a proto
//! cannot state).
//!
//! What it writes:
//! - [`rust`]: the router side's role shims (`impl RoleCall for <R>Call`,
//!   pairing each Call with its Return and Role), included by router-ffi;
//! - [`python`]: the whole cffi layer of a Python binding: the `cdef`, the
//!   library loader and ABI-version check, the Role enum, the per-role
//!   invokers (decode the Call, call the thunk, encode the Return or the
//!   error), the one callback trampoline and the router handle
//!   (register / dispatch). Committed at [`PYTHON_MODULE`], checked against
//!   a regeneration by the tests, and vendored by client-python. What stays
//!   hand-written there is the user-facing registration API the codegen
//!   templates target, and the error classes.

/// abi.proto's bytes: the ABI version's input.
pub const ABI_PROTO: &[u8] = include_bytes!("../../../proto/io/angzarr/router/ffi/v1/abi.proto");

/// abi.proto's path within the proto root, as its descriptor names it.
pub const ABI_PROTO_FILE: &str = "io/angzarr/router/ffi/v1/abi.proto";

/// abi.proto's compiled FileDescriptorSet (build.rs).
pub const DESCRIPTOR_SET: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/abi_fds.bin"));

/// The committed Python module [`python`] generates, relative to the
/// workspace root.
pub const PYTHON_MODULE: &str = "crates/router-ffi/generated/python/angzarr_router_abi.py";

/// The FNV-1a 32-bit hash of `bytes`.
pub const fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u32;
        hash = hash.wrapping_mul(0x0100_0193);
        i += 1;
    }
    hash
}

/// The ABI version: abi.proto's FNV-1a 32-bit hash. Any edit of abi.proto
/// is a new ABI.
pub const fn abi_version() -> u32 {
    fnv1a32(ABI_PROTO)
}

/// One C entry point: its name, C return type and C parameters (name, C
/// type), as a `cdef` declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Symbol {
    pub name: &'static str,
    pub returns: &'static str,
    pub params: &'static [(&'static str, &'static str)],
}

/// The buffer struct and the one host callback, as C declares them.
pub const C_TYPES: &str = "typedef struct { uint8_t* data; size_t len; } angzarr_buf;\n\
typedef int32_t (*angzarr_cb)(uint64_t callback_id,\n    \
const uint8_t* type_url, size_t type_url_len,\n    \
const uint8_t* payload, size_t payload_len,\n    \
const uint8_t* aux, size_t aux_len,\n    \
angzarr_buf* out);";

/// The C entry points, all of them: one register, one dispatch, and the
/// allocator, lifecycle and version calls around them.
pub const SYMBOLS: &[Symbol] = &[
    Symbol {
        name: "angzarr_abi_version",
        returns: "uint32_t",
        params: &[],
    },
    Symbol {
        name: "angzarr_buf_alloc",
        returns: "uint8_t*",
        params: &[("len", "size_t")],
    },
    Symbol {
        name: "angzarr_buf_release",
        returns: "void",
        params: &[("ptr", "uint8_t*"), ("len", "size_t")],
    },
    Symbol {
        name: "angzarr_router_new",
        returns: "void*",
        params: &[],
    },
    Symbol {
        name: "angzarr_router_free",
        returns: "void",
        params: &[("r", "void*")],
    },
    Symbol {
        name: "angzarr_router_register",
        returns: "int32_t",
        params: &[
            ("r", "void*"),
            ("spec", "const uint8_t*"),
            ("spec_len", "size_t"),
            ("cb", "angzarr_cb"),
            ("out", "angzarr_buf*"),
        ],
    },
    Symbol {
        name: "angzarr_router_dispatch",
        returns: "int32_t",
        params: &[
            ("r", "void*"),
            ("request", "const uint8_t*"),
            ("request_len", "size_t"),
            ("out", "angzarr_buf*"),
        ],
    },
];

/// One value of abi.proto's `Role` enum: its number, its name without the
/// `ROLE_` prefix (`POLICY_EVENT`), and its Call and Return message names
/// (`PolicyEventCall`, `PolicyEventReturn`), `None` for a declaration role
/// (never called back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleSpec {
    pub number: i32,
    pub name: String,
    pub messages: Option<(String, String)>,
}

/// Why abi.proto cannot be generated from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenError {
    /// The descriptor set does not decode.
    Undecodable,
    /// The descriptor set holds no abi.proto.
    NoAbiFile,
    /// abi.proto declares no `Role` enum.
    NoRoleEnum,
    /// A role with only one of its two messages (`present` names it).
    HalfPaired { role: String, present: String },
    /// A `<X>Call` or `<X>Return` message no role is named for.
    Orphan { message: String },
}

/// abi.proto's roles, in enum order, `ROLE_UNSPECIFIED` excepted, each
/// paired with its Call and Return messages by name (`ROLE_FOLD`:
/// `FoldCall`, `FoldReturn`).
pub fn roles(descriptor_set: &[u8]) -> Result<Vec<RoleSpec>, GenError> {
    let _ = descriptor_set;
    todo!("abigen::roles")
}

/// The router side's role shims: one `impl RoleCall for <R>Call` per
/// called role, naming its Return and its Role.
pub fn rust(descriptor_set: &[u8]) -> Result<String, GenError> {
    let _ = descriptor_set;
    todo!("abigen::rust")
}

/// Where a Python binding's generated module finds its protobuf classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PythonTarget {
    /// The package holding abi.proto's `abi_pb2`.
    pub abi_pb2_package: &'static str,
}

/// client-python's layout: protos generated under `angzarr_client.proto`.
pub const CLIENT_PYTHON: PythonTarget = PythonTarget {
    abi_pb2_package: "angzarr_client.proto.io.angzarr.router.ffi.v1",
};

/// The Python cffi layer for `target` (see the crate docs): the module
/// committed at [`PYTHON_MODULE`].
pub fn python(descriptor_set: &[u8], target: PythonTarget) -> Result<String, GenError> {
    let _ = (descriptor_set, target);
    todo!("abigen::python")
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod lib_tests;
