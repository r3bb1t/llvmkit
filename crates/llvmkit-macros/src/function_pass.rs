//! The `#[function_pass]` attribute macro.
//!
//! Expands an inherent `impl Pass { fn run(&mut self, cx: FnCx<Self>) -> .. }`
//! block into exactly the raw [`FunctionPass`] trait impl a user could
//! hand-write: it supplies the `impl<'ctx, B: ModuleBrand + 'ctx>` header, the
//! `type Access`/`type Requires`/`const NAME` associated-item block, and the
//! canonical `run` signature with its lifetimes — nothing else. Zero runtime
//! cost: the output is byte-for-byte the impl behind the sugar.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::Result;

use crate::ir_struct::default_crate_path;
use crate::pass_macro_shared::{PassAttrs, parse_pass_impl, required_const, requires_tuple};

/// Delegator boundary mirroring `ir_struct::derive`: run the fallible expansion
/// and turn any `syn::Error` into a `compile_error!` invocation with good spans.
pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    match try_expand(attr, item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn try_expand(attr: TokenStream, item: TokenStream) -> Result<TokenStream2> {
    let PassAttrs {
        name,
        access,
        requires,
        required,
    } = PassAttrs::parse(attr)?;
    let pass = parse_pass_impl(item)?;

    let ir = default_crate_path();
    let self_ty = &pass.self_ty;
    let cx_ident = &pass.cx_ident;
    let body = &pass.body;
    let requires_ty = requires_tuple(&requires);
    let required_item = required_const(required);
    let inherent = pass.inherent();

    Ok(quote! {
        impl<'ctx, B: #ir::ModuleBrand + 'ctx> #ir::FunctionPass<'ctx, B> for #self_ty {
            type Access = #ir::#access;
            type Requires = #requires_ty;
            const NAME: &'static str = #name;
            #required_item

            fn run(
                &mut self,
                #cx_ident: #ir::FnCx<'_, '_, 'ctx, B, #ir::#access, #requires_ty>,
            ) -> #ir::IrResult<#ir::FnReport>
            #body
        }

        #inherent
    })
}
