#![forbid(unsafe_code)]
//! Procedural macros for llvmkit.
//!
//! `#[derive(IrStruct)]` maps a non-generic named-field Rust struct to an
//! llvmkit named struct schema and generates the matching `<Struct>Value`
//! wrapper. Supported helper attributes are intentionally small:
//! `#[llvmkit(name = "...")]`, `#[llvmkit(packed)]`, and
//! `#[llvmkit(crate = path::to::ir)]`.
//!
//! `#[function_pass]` / `#[module_pass]` are ergonomics sugar for the Pass API
//! v2: each expands an inherent `impl Pass { fn run(..) }` block into exactly the
//! raw `FunctionPass`/`ModulePass` trait impl a user could hand-write, hiding the
//! `impl<'ctx, B: ModuleBrand + 'ctx> … for` header, the associated-item block,
//! and the `run` lifetimes. Zero runtime cost — the output is the same impl.

use proc_macro::TokenStream;

mod function_pass;
mod ir_struct;
mod module_pass;
mod pass_macro_shared;

#[proc_macro_derive(IrStruct, attributes(llvmkit))]
pub fn derive_ir_struct(input: TokenStream) -> TokenStream {
    ir_struct::derive(input)
}

/// Author a `FunctionPass` from an inherent `impl` block. See the crate docs and
/// `llvmkit_ir`'s pass-API documentation for the attribute grammar
/// (`name` / `access` / `requires` / `required`).
#[proc_macro_attribute]
pub fn function_pass(attr: TokenStream, item: TokenStream) -> TokenStream {
    function_pass::expand(attr, item)
}

/// Author a `ModulePass` from an inherent `impl` block — the module-level mirror
/// of [`macro@function_pass`].
#[proc_macro_attribute]
pub fn module_pass(attr: TokenStream, item: TokenStream) -> TokenStream {
    module_pass::expand(attr, item)
}
