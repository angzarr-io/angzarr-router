use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::*;
use crate::model::{Model, Scope};

const WS: &str = "/ws";

fn files(entries: &[(&str, &str)]) -> BTreeMap<PathBuf, String> {
    entries
        .iter()
        .map(|(p, t)| (Path::new(WS).join(p), t.to_string()))
        .collect()
}

fn known(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// Scans one library crate `app` (src/lib.rs) whose code may name `deps`.
fn scan_lib(entries: &[(&str, &str)], deps: &[&str]) -> (Model, Vec<String>) {
    let mut s = Scanner::in_memory(PathBuf::from(WS), files(entries));
    s.add_workspace_crate("app");
    let mut k = known(deps);
    k.insert("app".into());
    s.add_target("app", &Path::new(WS).join("src/lib.rs"), Scope::Lib, &k)
        .unwrap();
    s.finish()
}

fn edges(model: &Model) -> Vec<(String, String, Scope)> {
    let set: BTreeSet<_> = model
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone(), e.scope))
        .collect();
    set.into_iter().collect()
}

fn has(model: &Model, from: &str, to: &str, scope: Scope) -> bool {
    edges(model).contains(&(from.into(), to.into(), scope))
}

#[test]
fn follows_flat_nested_and_path_attr_modules() {
    let (m, warnings) = scan_lib(
        &[
            ("src/lib.rs", "pub mod a; mod b; #[path = \"c.impl.rs\"] mod c; pub mod inline { pub mod deep {} }"),
            ("src/a.rs", "mod sub;"),
            ("src/a/sub.rs", ""),
            ("src/b/mod.rs", "mod x;"),
            ("src/b/x.rs", ""),
            ("src/c.impl.rs", ""),
        ],
        &[],
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    let mods: Vec<&str> = m.modules.keys().map(String::as_str).collect();
    assert_eq!(
        mods,
        vec![
            "app",
            "app::a",
            "app::a::sub",
            "app::b",
            "app::b::x",
            "app::c",
            "app::inline",
            "app::inline::deep"
        ]
    );
    assert_eq!(m.modules["app::c"], PathBuf::from("src/c.impl.rs"));
    assert_eq!(m.modules["app::b::x"], PathBuf::from("src/b/x.rs"));
    assert_eq!(m.modules["app::inline::deep"], PathBuf::from("src/lib.rs"));
}

#[test]
fn missing_module_file_is_a_warning() {
    let (m, warnings) = scan_lib(&[("src/lib.rs", "mod gone;")], &[]);
    assert_eq!(
        warnings,
        vec!["src/lib.rs: module `gone` has no source file".to_string()]
    );
    assert!(!m.modules.contains_key("app::gone"));
}

#[test]
fn unreadable_or_unparsable_root_is_an_error() {
    let mut s = Scanner::in_memory(PathBuf::from(WS), files(&[("src/bad.rs", "fn (")]));
    let err = s
        .add_target(
            "app",
            Path::new("/ws/src/missing.rs"),
            Scope::Lib,
            &known(&[]),
        )
        .unwrap_err();
    assert!(err.contains("cannot read"), "{err}");
    let err = s
        .add_target("app", Path::new("/ws/src/bad.rs"), Scope::Lib, &known(&[]))
        .unwrap_err();
    assert!(err.starts_with("/ws/src/bad.rs:"), "{err}");
}

#[test]
fn resolves_crate_self_super_and_child_paths() {
    let (m, _) = scan_lib(
        &[
            ("src/lib.rs", "pub mod error; pub mod saga; pub mod kernel; pub fn root_fn() {}"),
            ("src/error.rs", "pub mod codes { pub const X: u8 = 1; }"),
            (
                "src/saga.rs",
                "use crate::error::codes; fn f() { let _ = super::kernel::g(); let _ = self::h(); crate::root_fn(); }\nfn h() {}",
            ),
            ("src/kernel.rs", "pub fn g() {} fn k() { let _ = crate::error::codes::X; }"),
        ],
        &[],
    );
    assert!(has(&m, "app::saga", "app::error::codes", Scope::Lib));
    assert!(has(&m, "app::saga", "app::kernel", Scope::Lib));
    assert!(has(&m, "app::saga", "app", Scope::Lib));
    assert!(has(&m, "app::kernel", "app::error::codes", Scope::Lib));
    assert!(
        !edges(&m).iter().any(|(f, t, _)| f == t),
        "self edges dropped"
    );
}

#[test]
fn records_the_line_and_written_path_of_each_reference() {
    let (m, _) = scan_lib(
        &[
            ("src/lib.rs", "pub mod a;\npub mod b;"),
            ("src/a.rs", "\n\nuse crate::b::thing;"),
            ("src/b.rs", ""),
        ],
        &[],
    );
    let e = m.edges.iter().find(|e| e.from == "app::a").unwrap();
    assert_eq!(e.to, "app::b");
    assert_eq!(e.line, 3);
    assert_eq!(e.file, PathBuf::from("src/a.rs"));
    assert_eq!(e.via, "crate::b::thing");
}

#[test]
fn resolves_through_reexport_aliases() {
    let (m, _) = scan_lib(
        &[
            (
                "src/lib.rs",
                "pub mod proto; pub mod saga; pub use proto::v1 as pb;",
            ),
            ("src/proto.rs", "pub mod v1 { pub struct Book; }"),
            ("src/saga.rs", "use crate::pb; fn f(_: pb::Book) {}"),
        ],
        &[],
    );
    assert!(has(&m, "app::saga", "app::proto::v1", Scope::Lib));
    assert!(has(&m, "app", "app::proto::v1", Scope::Lib));
    assert!(
        !has(&m, "app::saga", "app", Scope::Lib),
        "alias resolves past the root"
    );
}

#[test]
fn resolves_alias_chains_into_external_crates() {
    let (m, _) = scan_lib(
        &[
            ("src/lib.rs", "pub mod a; pub use prost as wire;"),
            (
                "src/a.rs",
                "fn f() { let _ = crate::wire::Message::decode; }",
            ),
        ],
        &["prost"],
    );
    assert!(has(&m, "app::a", "prost::Message::decode", Scope::Lib));
}

#[test]
fn external_paths_need_a_known_crate_name() {
    let (m, _) = scan_lib(
        &[("src/lib.rs", "use std::ffi::c_void; use prost::Message; fn f() { let _ = HashMap::new(); let _ = Local::X; }")],
        &["prost"],
    );
    let targets: Vec<String> = edges(&m).into_iter().map(|(_, t, _)| t).collect();
    assert!(targets.contains(&"prost::Message".to_string()));
    assert!(!targets
        .iter()
        .any(|t| t.starts_with("HashMap") || t.starts_with("Local")));
    let (m, _) = scan_lib(&[("src/lib.rs", "use std::ffi::c_void;")], &["std"]);
    assert!(has(&m, "app", "std::ffi::c_void", Scope::Lib));
}

#[test]
fn leading_colon_paths_are_crate_paths() {
    let (m, _) = scan_lib(
        &[("src/lib.rs", "fn f() { let _ = ::unknown_crate::x(); }")],
        &[],
    );
    assert!(has(&m, "app", "unknown_crate::x", Scope::Lib));
}

#[test]
fn flattens_use_groups_globs_self_and_renames() {
    let (m, _) = scan_lib(
        &[
            ("src/lib.rs", "pub mod a; pub mod b; pub mod c; pub mod d; mod user;"),
            ("src/a.rs", ""),
            ("src/b.rs", "pub mod inner {}"),
            ("src/c.rs", ""),
            ("src/d.rs", ""),
            (
                "src/user.rs",
                "use crate::{a::{self}, b::inner as renamed, c::*}; use crate::d::Thing as _; fn f() { let _ = renamed::X; }",
            ),
        ],
        &[],
    );
    for to in ["app::a", "app::b::inner", "app::c", "app::d"] {
        assert!(
            has(&m, "app::user", to, Scope::Lib),
            "missing {to}: {:?}",
            edges(&m)
        );
    }
}

#[test]
fn cfg_test_items_and_modules_are_test_scope() {
    let (m, _) = scan_lib(
        &[
            (
                "src/lib.rs",
                "pub mod a; pub mod b; pub mod c;\n#[cfg(test)] mod tests { use crate::a::X; }\n#[cfg(test)] fn helper() { crate::b::y(); }\n#[cfg(not(test))] fn real() { crate::c::z(); }\nstruct S; impl S { #[cfg(test)] fn t() { crate::c::w(); } }",
            ),
            ("src/a.rs", ""),
            ("src/b.rs", ""),
            ("src/c.rs", ""),
        ],
        &[],
    );
    assert!(has(&m, "app::tests", "app::a", Scope::Test));
    assert!(has(&m, "app", "app::b", Scope::Test));
    assert!(has(&m, "app", "app::c", Scope::Lib));
    assert!(has(&m, "app", "app::c", Scope::Test));
    assert!(!has(&m, "app", "app::b", Scope::Lib));
}

#[test]
fn cfg_test_module_files_inherit_test_scope() {
    let (m, _) = scan_lib(
        &[
            (
                "src/lib.rs",
                "pub mod a;\n#[cfg(test)]\n#[path = \"lib.test.rs\"]\nmod lib_tests;",
            ),
            ("src/a.rs", ""),
            ("src/lib.test.rs", "use crate::a::X;"),
        ],
        &[],
    );
    assert!(has(&m, "app::lib_tests", "app::a", Scope::Test));
    assert!(!has(&m, "app::lib_tests", "app::a", Scope::Lib));
}

#[test]
fn paths_inside_macro_arguments_are_references() {
    let (m, _) = scan_lib(
        &[
            (
                "src/lib.rs",
                "pub mod a; fn f() { assert_eq!(crate::a::x(), 1); let _ = vec![crate::a::y()]; }",
            ),
            ("src/a.rs", ""),
        ],
        &[],
    );
    assert!(has(&m, "app", "app::a", Scope::Lib));
}

#[test]
fn include_macros_are_path_edges() {
    let (m, _) = scan_lib(
        &[(
            "src/lib.rs",
            "const H: &str = include_str!(\"../bindings/go/x.h\"); mod g { include!(\"gen.rs\"); }",
        )],
        &[],
    );
    assert!(has(&m, "app", "path:bindings/go/x.h", Scope::Lib));
    assert!(has(&m, "app::g", "path:src/gen.rs", Scope::Lib));
}

#[test]
fn cross_crate_paths_walk_into_workspace_crates() {
    let mut s = Scanner::in_memory(
        PathBuf::from(WS),
        files(&[
            ("core/src/lib.rs", "pub mod error; pub use error::Coded;"),
            ("core/src/error.rs", "pub struct Coded;"),
            (
                "ffi/src/lib.rs",
                "use core_crate::error::Coded; fn f(_: core_crate::Coded) {}",
            ),
        ]),
    );
    s.add_workspace_crate("core_crate");
    s.add_workspace_crate("ffi");
    s.add_target(
        "core_crate",
        Path::new("/ws/core/src/lib.rs"),
        Scope::Lib,
        &known(&["core_crate"]),
    )
    .unwrap();
    s.add_target(
        "ffi",
        Path::new("/ws/ffi/src/lib.rs"),
        Scope::Lib,
        &known(&["ffi", "core_crate"]),
    )
    .unwrap();
    let (m, _) = s.finish();
    assert!(has(&m, "ffi", "core_crate::error", Scope::Lib));
    assert!(
        has(&m, "ffi", "core_crate", Scope::Lib),
        "re-export ends at the re-exporting module"
    );
    assert_eq!(
        edges(&m).iter().filter(|(f, _, _)| f == "ffi").count(),
        2,
        "{:?}",
        edges(&m)
    );
    assert!(m.workspace_crates.contains("ffi"));
}

#[test]
fn manifest_edges_are_kept_with_relative_manifest_paths() {
    let mut s = Scanner::in_memory(PathBuf::from(WS), BTreeMap::new());
    s.add_manifest_edge(
        "app",
        "dep",
        Scope::Test,
        Path::new("/ws/crates/app/Cargo.toml"),
        "[dependencies] dep",
    );
    let (m, _) = s.finish();
    assert_eq!(m.edges.len(), 1);
    assert_eq!(m.edges[0].file, PathBuf::from("crates/app/Cargo.toml"));
    assert_eq!(m.edges[0].scope, Scope::Test);
    assert_eq!(m.edges[0].via, "[dependencies] dep");
}

#[test]
fn relative_normalises_dot_segments() {
    let s = Scanner::new(PathBuf::from("/ws"));
    assert_eq!(
        s.relative(Path::new("/ws/crates/a/../b/./c")),
        PathBuf::from("crates/b/c")
    );
    assert_eq!(
        s.relative(Path::new("/elsewhere/x")),
        PathBuf::from("/elsewhere/x")
    );
}

#[test]
fn super_at_the_root_stays_at_the_root() {
    assert_eq!(parent_of("app", "app"), "app");
    assert_eq!(parent_of("app::a::b", "app"), "app::a");
}

#[test]
fn is_cfg_test_recognises_test_gates_only() {
    let attrs = |src: &str| -> Vec<Attribute> {
        let item: syn::ItemFn = syn::parse_str(&format!("{src} fn f() {{}}")).unwrap();
        item.attrs
    };
    assert!(is_cfg_test(&attrs("#[cfg(test)]")));
    assert!(is_cfg_test(&attrs("#[cfg(all(test, feature = \"x\"))]")));
    assert!(!is_cfg_test(&attrs("#[cfg(not(test))]")));
    assert!(!is_cfg_test(&attrs("#[cfg(feature = \"testing\")]")));
    assert!(!is_cfg_test(&attrs("#[test]")));
    assert!(!is_cfg_test(&attrs("#[doc = \"test\"]")));
}

#[test]
fn workspace_crate_names_resolve_without_a_declared_dependency() {
    let mut s = Scanner::in_memory(
        PathBuf::from(WS),
        files(&[
            ("core/src/lib.rs", "use ffi::Buf;"),
            ("ffi/src/lib.rs", "pub struct Buf;"),
        ]),
    );
    s.add_workspace_crate("core_crate");
    s.add_workspace_crate("ffi");
    s.add_target(
        "core_crate",
        Path::new("/ws/core/src/lib.rs"),
        Scope::Lib,
        &known(&["core_crate"]),
    )
    .unwrap();
    s.add_target(
        "ffi",
        Path::new("/ws/ffi/src/lib.rs"),
        Scope::Lib,
        &known(&["ffi"]),
    )
    .unwrap();
    let (m, _) = s.finish();
    assert!(has(&m, "core_crate", "ffi", Scope::Lib), "{:?}", edges(&m));
}
