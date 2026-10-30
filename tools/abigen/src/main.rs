//! `cargo run -p angzarr-abigen -- python`: (re)writes the committed Python
//! cffi layer (angzarr_abigen::PYTHON_MODULE) from abi.proto.

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args != ["python"] {
        eprintln!("usage: angzarr-abigen python");
        return ExitCode::from(2);
    }
    let module = match angzarr_abigen::python(
        angzarr_abigen::DESCRIPTOR_SET,
        angzarr_abigen::CLIENT_PYTHON,
    ) {
        Ok(module) => module,
        Err(err) => {
            eprintln!("angzarr-abigen: {err:?}");
            return ExitCode::FAILURE;
        }
    };
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(angzarr_abigen::PYTHON_MODULE);
    if let Some(dir) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(dir) {
            eprintln!("angzarr-abigen: {}: {err}", dir.display());
            return ExitCode::FAILURE;
        }
    }
    match std::fs::write(&path, module) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("angzarr-abigen: {}: {err}", path.display());
            ExitCode::FAILURE
        }
    }
}
