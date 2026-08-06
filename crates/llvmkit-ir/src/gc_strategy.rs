//! Names of LLVM's built-in garbage-collection strategies.
//!
//! Mirrors the `GCRegistry::Add<...>` registrations in
//! `llvm/lib/IR/BuiltinGCs.cpp`. Each constant is the exact string a
//! function's `gc "<name>"` marker uses to select that strategy.
//!
//! These are **constants, not an enum**, on purpose: upstream's
//! `GCStrategy` registry (`llvm/include/llvm/IR/GCStrategy.h`) is designed
//! for out-of-tree collectors to register themselves by name, so the
//! namespace is open and almost all real-world `gc` markers name a
//! strategy this module cannot know about. An enum would put ~95% of
//! traffic through its `Custom` arm; `FunctionValue::set_gc` therefore
//! keeps accepting any string, and these constants only spell the five
//! built-ins without typos.
//!
//! ```
//! use llvmkit_ir::{IrError, Linkage, gc_strategy, module_new};
//!
//! let m = module_new!("gc")?;
//! let fn_ty = m.function_type_no_parameters(m.void_type());
//! let f = m.add_function_dyn("collected", fn_ty, Linkage::External)?;
//! m.view(f).set_gc(&m, gc_strategy::STATEPOINT_EXAMPLE);
//! assert_eq!(m.view(f).gc().as_deref(), Some("statepoint-example"));
//! # Ok::<(), IrError>(())
//! ```

/// The Erlang-compatible collector (`GCRegistry::Add<ErlangGC>` in
/// `BuiltinGCs.cpp`).
pub const ERLANG: &str = "erlang";

/// The OCaml 3.10-compatible collector (`GCRegistry::Add<OcamlGC>` in
/// `BuiltinGCs.cpp`).
pub const OCAML: &str = "ocaml";

/// The portable shadow-stack collector for uncooperative code generators
/// (`GCRegistry::Add<ShadowStackGC>` in `BuiltinGCs.cpp`).
pub const SHADOW_STACK: &str = "shadow-stack";

/// The example statepoint strategy (`GCRegistry::Add<StatepointGC>` in
/// `BuiltinGCs.cpp`).
pub const STATEPOINT_EXAMPLE: &str = "statepoint-example";

/// The CoreCLR-compatible collector (`GCRegistry::Add<CoreCLRGC>` in
/// `BuiltinGCs.cpp`).
pub const CORECLR: &str = "coreclr";
