#![forbid(unsafe_code)]
//! Procedural macros for llvmkit.
//!
//! `#[derive(IrStruct)]` maps a non-generic named-field Rust struct to an
//! llvmkit named struct schema and generates the matching `<Struct>Value`
//! wrapper. Supported helper attributes are intentionally small:
//! `#[llvmkit(name = "...")]`, `#[llvmkit(packed)]`, and
//! `#[llvmkit(crate = path::to::ir)]`.

use proc_macro::TokenStream;

mod ir_struct;

#[proc_macro_derive(IrStruct, attributes(llvmkit))]
pub fn derive_ir_struct(input: TokenStream) -> TokenStream {
    ir_struct::derive(input)
}
