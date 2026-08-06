#![forbid(unsafe_code)]

//! Expands the vendored TableGen tree into this crate's intrinsic tables.
//!
//! The generator itself is `llvmkit-tablegen`, which ports LLVM's
//! `lib/TableGen` front end and `utils/TableGen/Basic` intrinsic emitter. It is
//! a crate rather than a file included here, so that its own tests run under
//! `cargo test` and its modules can mirror the upstream files they port.
//!
//! This script is only the plumbing around that library: point
//! [`llvmkit_tablegen::generate`] at the vendored tree and `OUT_DIR`, then
//! replay every input the run read as a `cargo:rerun-if-changed` line.

use std::path::PathBuf;

fn main() {
    let out_dir = std::env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR for build scripts");
    let generated_file = PathBuf::from(out_dir).join(llvmkit_tablegen::GENERATED_FILE_NAME);
    let llvm_root = llvmkit_tablegen::vendored_llvm_root();

    match llvmkit_tablegen::generate(&llvm_root, &generated_file) {
        Ok(generated) => {
            println!("cargo:rerun-if-changed=build.rs");
            // The generator's own sources need no naming here: it is a
            // build-dependency crate, so Cargo already reruns the script when
            // it changes.
            for input in generated.inputs() {
                println!("cargo:rerun-if-changed={}", input.display());
            }
            println!("cargo:rerun-if-changed={}", llvm_root.display());
        }
        Err(err) => {
            eprintln!("llvmkit-tablegen: {err}");
            std::process::exit(1);
        }
    }
}
