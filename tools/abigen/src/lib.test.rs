//! The generator's contract: the version is abi.proto's hash, every role
//! pairs with exactly its two messages, and the Rust shims and the Python
//! cffi layer cover every role and symbol and match what is committed.

use prost::Message;
use prost_types::{
    DescriptorProto, EnumDescriptorProto, EnumValueDescriptorProto, FileDescriptorProto,
    FileDescriptorSet,
};

use super::*;

fn role(number: i32, name: &str, call: &str, ret: &str) -> RoleSpec {
    RoleSpec {
        number,
        name: name.to_string(),
        messages: Some((call.to_string(), ret.to_string())),
    }
}

#[test]
fn fnv1a32_matches_the_published_vectors() {
    assert_eq!(fnv1a32(b""), 0x811c_9dc5);
    assert_eq!(fnv1a32(b"a"), 0xe40c_292c);
    assert_eq!(fnv1a32(b"foobar"), 0xbf9c_f968);
}

#[test]
fn the_abi_version_is_the_hash_of_abi_proto() {
    assert_eq!(abi_version(), fnv1a32(ABI_PROTO));
    let mut edited = ABI_PROTO.to_vec();
    edited.push(b'\n');
    assert_ne!(fnv1a32(&edited), abi_version(), "any edit is a new ABI");
}

#[test]
fn every_role_pairs_with_its_call_and_return_and_starts_is_a_declaration() {
    assert_eq!(
        roles(DESCRIPTOR_SET).expect("abi.proto's roles"),
        vec![
            role(1, "FOLD", "FoldCall", "FoldReturn"),
            role(2, "DECIDE", "DecideCall", "DecideReturn"),
            role(3, "FACT", "FactCall", "FactReturn"),
            role(4, "REFUSED", "RefusedCall", "RefusedReturn"),
            role(5, "CHANGED", "ChangedCall", "ChangedReturn"),
            RoleSpec {
                number: 6,
                name: "STARTS".to_string(),
                messages: None,
            },
            role(7, "POLICY_EVENT", "PolicyEventCall", "PolicyEventReturn"),
            role(
                8,
                "POLICY_REFUSED",
                "PolicyRefusedCall",
                "PolicyRefusedReturn"
            ),
            role(9, "PROJECT", "ProjectCall", "ProjectReturn"),
        ]
    );
}

/// A descriptor set of one abi.proto declaring `roles` and `messages`.
fn synthetic(roles: &[(&str, i32)], messages: &[&str]) -> Vec<u8> {
    let mut values = vec![EnumValueDescriptorProto {
        name: Some("ROLE_UNSPECIFIED".to_string()),
        number: Some(0),
        ..Default::default()
    }];
    values.extend(roles.iter().map(|(name, number)| EnumValueDescriptorProto {
        name: Some(name.to_string()),
        number: Some(*number),
        ..Default::default()
    }));
    FileDescriptorSet {
        file: vec![FileDescriptorProto {
            name: Some(ABI_PROTO_FILE.to_string()),
            package: Some("io.angzarr.router.ffi.v1".to_string()),
            enum_type: vec![EnumDescriptorProto {
                name: Some("Role".to_string()),
                value: values,
                ..Default::default()
            }],
            message_type: messages
                .iter()
                .map(|name| DescriptorProto {
                    name: Some(name.to_string()),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }],
    }
    .encode_to_vec()
}

#[test]
fn a_role_with_one_message_of_its_two_is_refused() {
    let set = synthetic(&[("ROLE_DECIDE", 1)], &["DecideCall"]);
    assert_eq!(
        roles(&set),
        Err(GenError::HalfPaired {
            role: "DECIDE".to_string(),
            present: "DecideCall".to_string(),
        })
    );
}

#[test]
fn a_call_message_no_role_is_named_for_is_refused() {
    let set = synthetic(
        &[("ROLE_DECIDE", 1)],
        &["DecideCall", "DecideReturn", "UpcastCall"],
    );
    assert_eq!(
        roles(&set),
        Err(GenError::Orphan {
            message: "UpcastCall".to_string()
        })
    );
}

#[test]
fn a_set_without_abi_proto_or_its_role_enum_is_refused() {
    assert_eq!(roles(b"\xff\xff"), Err(GenError::Undecodable));
    let other = FileDescriptorSet {
        file: vec![FileDescriptorProto {
            name: Some("other.proto".to_string()),
            ..Default::default()
        }],
    }
    .encode_to_vec();
    assert_eq!(roles(&other), Err(GenError::NoAbiFile));
    let no_enum = FileDescriptorSet {
        file: vec![FileDescriptorProto {
            name: Some(ABI_PROTO_FILE.to_string()),
            ..Default::default()
        }],
    }
    .encode_to_vec();
    assert_eq!(roles(&no_enum), Err(GenError::NoRoleEnum));
}

#[test]
fn the_rust_shims_pair_every_called_role_with_its_return() {
    let rust = rust(DESCRIPTOR_SET).expect("rust shims");
    for spec in roles(DESCRIPTOR_SET).expect("roles") {
        let Some((call, ret)) = spec.messages else {
            assert!(
                !rust.contains(&format!("Role::{}", pascal(&spec.name))),
                "a declaration role ({}) has no shim",
                spec.name
            );
            continue;
        };
        let shim = format!(
            "impl RoleCall for {call} {{\n    const ROLE: Role = Role::{};\n    type Return = {ret};\n}}",
            pascal(&spec.name)
        );
        assert!(rust.contains(&shim), "missing shim:\n{shim}\nin:\n{rust}");
    }
}

fn pascal(screaming: &str) -> String {
    screaming
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or(String::new(), |first| {
                first.to_string() + &chars.as_str().to_ascii_lowercase()
            })
        })
        .collect()
}

#[test]
fn the_python_layer_declares_every_symbol_in_its_cdef() {
    let python = python(DESCRIPTOR_SET, CLIENT_PYTHON).expect("python");
    for symbol in SYMBOLS {
        let params = if symbol.params.is_empty() {
            "void".to_string()
        } else {
            symbol
                .params
                .iter()
                .map(|(name, ty)| format!("{ty} {name}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let decl = format!("{} {}({params});", symbol.returns, symbol.name);
        assert!(python.contains(&decl), "cdef lacks `{decl}`");
    }
    assert!(
        python.contains(C_TYPES),
        "cdef lacks the buffer and callback types"
    );
}

#[test]
fn the_python_layer_bakes_the_abi_version_and_checks_it_at_load() {
    let python = python(DESCRIPTOR_SET, CLIENT_PYTHON).expect("python");
    assert!(python.contains(&format!("ABI_VERSION = {}\n", abi_version())));
    assert!(python.contains("class AbiVersionError(ImportError):"));
    assert!(python.contains("lib.angzarr_abi_version()"));
}

#[test]
fn the_python_layer_has_an_invoker_for_every_called_role_and_none_for_a_declaration() {
    let python = python(DESCRIPTOR_SET, CLIENT_PYTHON).expect("python");
    assert!(python.contains(&format!(
        "from {} import abi_pb2",
        CLIENT_PYTHON.abi_pb2_package
    )));
    for spec in roles(DESCRIPTOR_SET).expect("roles") {
        assert!(
            python.contains(&format!("    {} = {}\n", spec.name, spec.number)),
            "Role.{} missing",
            spec.name
        );
        let entry = format!("Role.{}: (", spec.name);
        match spec.messages {
            Some((call, ret)) => assert!(
                python.contains(&format!("{entry}abi_pb2.{call}, abi_pb2.{ret}),")),
                "no invoker table entry for {}",
                spec.name
            ),
            None => assert!(
                !python.contains(&entry),
                "a declaration role ({}) has no invoker",
                spec.name
            ),
        }
    }
}

#[test]
fn the_committed_python_module_is_what_the_generator_writes() {
    let committed = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(PYTHON_MODULE),
    )
    .unwrap_or_else(|e| {
        panic!("{PYTHON_MODULE} missing ({e}): run `cargo run -p angzarr-abigen -- python`")
    });
    assert_eq!(
        python(DESCRIPTOR_SET, CLIENT_PYTHON).expect("python"),
        committed,
        "{PYTHON_MODULE} is stale: run `cargo run -p angzarr-abigen -- python`"
    );
}
