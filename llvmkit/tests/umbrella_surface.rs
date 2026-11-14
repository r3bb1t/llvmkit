//! Umbrella-crate surface smoke test.
//!
//! llvmkit-specific packaging guard with no upstream counterpart: it proves
//! the `llvmkit` umbrella's module re-exports (`llvmkit::ir`,
//! `llvmkit::asmparser`) are sufficient to build, print, and re-parse a
//! module without depending on the implementation crates directly. The
//! closest upstream functional reference is the umbrella `llvm/IR` +
//! `llvm/AsmParser` link surface a C++ tool consumes.

use llvmkit::asmparser::parse_dynamic;
use llvmkit::ir::{Dyn, IrBuilder, Linkage, Module};

/// llvmkit-specific packaging guard (no upstream counterpart): the umbrella
/// re-exports alone build `define void @f()`, print it, and parse the
/// printed text back into a module that verifies and prints the same
/// function.
#[test]
fn umbrella_reexports_build_print_and_reparse() -> Result<(), Box<dyn std::error::Error>> {
    let mut module = Module::dynamic("umbrella_surface");
    let mut key = module.layout_token();
    let m = key.module();
    let void_type = m.void_type();
    let function_type = m.function_type_no_parameters(void_type)?;
    let f = m.add_function_dyn("f", function_type, Linkage::External)?;
    let entry = m.view(f).append_basic_block(m, "entry");
    let builder = IrBuilder::new_for::<Dyn>(&mut key).try_position_at_end(entry)?;
    let _terminated = builder.ret_void()?;

    let module = module.verify()?;
    let printed = format!("{module}");
    assert!(printed.contains("define void @f()"));
    assert!(printed.contains("ret void"));

    let reparsed = parse_dynamic(&printed)?;
    let reparsed = reparsed.verify()?;
    let reprinted = format!("{reparsed}");
    assert!(reprinted.contains("define void @f()"));
    assert!(reprinted.contains("ret void"));
    Ok(())
}
