#![forbid(unsafe_code)]
//! Procedural macros for llvmkit.
//!
//! `#[derive(IrStruct)]` maps a non-generic named-field Rust struct to an
//! llvmkit named struct schema and generates the matching `<Struct>Value`
//! wrapper. Supported helper attributes are intentionally small:
//! `#[llvmkit(name = "...")]`, `#[llvmkit(packed)]`, and
//! `#[llvmkit(crate = path::to::ir)]`.
//!
//! `#[function_pass]` / `#[module_pass]` are ergonomics sugar for the
//! capability-graded pass API: each expands an inherent
//! `impl Pass { fn run(..) }` block into exactly the
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

/// Author a `FunctionPass` from a plain inherent `impl` block.
///
/// Expands `impl Pass { fn run(&mut self, cx) -> .. { .. } }` into exactly the
/// raw `impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for Pass` a user
/// could hand-write — the impl header, the `type Access` / `type Requires` /
/// `const NAME` block, and the canonical `run` lifetimes are all supplied.
/// **Zero runtime cost:** the output is byte-for-byte that trait impl.
///
/// # Attributes
///
/// - `name = "..."` (**required**) → `const NAME`.
/// - `access = <Rung>` (**required**) → `type Access`; one of `Inspect`,
///   `PatchBody`, or `ReshapeCfg`. A module-only rung such as `RewriteModule`
///   fails the `FnAccess` bound at compile time.
/// - `requires = [A, B]` (optional, default `[]`) → `type Requires = (A, B,)`;
///   the analyses are prefetched, so `cx.analysis::<A, _>()` is infallible.
/// - `required` (optional bare flag) → `const REQUIRED: bool = true`.
///
/// The written `cx: FnCx<Self>` / `-> IrResult<FnReport>` are readability
/// sentinels: only the `&mut self` receiver, the context binding name, and the
/// body are kept, so those types need not be imported. The annotated item must
/// be a non-generic inherent `impl` with exactly one `fn run`; any other item is
/// re-emitted in a companion inherent `impl`.
///
/// A missing `name`/`access`, an unknown key, a trait impl, or a generic impl
/// each produce a pinpointed compile error. See `llvmkit_ir`'s `pass_manager`
/// docs and the `authored_pass` example for runnable end-to-end usage.
#[proc_macro_attribute]
pub fn function_pass(attr: TokenStream, item: TokenStream) -> TokenStream {
    function_pass::expand(attr, item)
}

/// Author a `ModulePass` from a plain inherent `impl` block — the module-level
/// mirror of [`macro@function_pass`].
///
/// Expands into exactly the raw `impl<'ctx, B: ModuleBrand + 'ctx>
/// ModulePass<'ctx, B> for Pass` (with `ModCx` / `ModReport` in place of `FnCx` /
/// `FnReport`). The attribute grammar is identical to
/// [`macro@function_pass`], except `access` is a module rung — `Inspect` or
/// `RewriteModule`; a function-only rung such as `PatchBody` fails the
/// `ModAccess` bound at compile time.
#[proc_macro_attribute]
pub fn module_pass(attr: TokenStream, item: TokenStream) -> TokenStream {
    module_pass::expand(attr, item)
}
