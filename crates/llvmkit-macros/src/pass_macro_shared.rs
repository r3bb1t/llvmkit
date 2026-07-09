//! Shared front-end for the `#[function_pass]` / `#[module_pass]` attribute
//! macros.
//!
//! Both macros accept the same attribute grammar and the same `impl` shape; only
//! the generated trait (`FunctionPass`/`ModulePass`), context type
//! (`FnCx`/`ModCx`), and report type (`FnReport`/`ModReport`) differ. This module
//! owns everything the two share — attribute parsing, the `fn run` extraction,
//! and the `Requires`/`REQUIRED` token builders — so the two `expand` functions
//! reduce to the final trait-impl `quote!`. Anything trait-specific stays in
//! `function_pass.rs` / `module_pass.rs`.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{
    Block, Error, FnArg, Ident, ImplItem, ImplItemFn, ItemImpl, LitStr, Pat, Path, Result, Token,
    Type, bracketed,
};

/// The parsed `#[function_pass(...)]` / `#[module_pass(...)]` attribute inputs.
pub(crate) struct PassAttrs {
    /// `name = "..."` → `const NAME`.
    pub(crate) name: LitStr,
    /// `access = <Ident>` → `type Access` (passed through under the crate path;
    /// the `FnAccess`/`ModAccess` bound rejects a wrong rung at compile time).
    pub(crate) access: Ident,
    /// `requires = [A, B]` → `type Requires` (empty/absent → `()`).
    pub(crate) requires: Vec<Path>,
    /// bare `required` flag → `const REQUIRED: bool = true` (absent → omitted).
    pub(crate) required: bool,
}

impl PassAttrs {
    /// Parse the attribute token stream. Every malformed or missing key surfaces
    /// as a `syn::Error` (never a panic), pointing at the offending token where
    /// one exists and at the attribute call site for an entirely missing key.
    pub(crate) fn parse(attr: TokenStream) -> Result<Self> {
        let mut name: Option<LitStr> = None;
        let mut access: Option<Ident> = None;
        let mut requires: Option<Vec<Path>> = None;
        let mut required = false;

        let parser = syn::meta::parser(|meta| {
            if meta.path.is_ident("name") {
                if name.is_some() {
                    return Err(meta.error("duplicate `name`"));
                }
                name = Some(meta.value()?.parse()?);
                Ok(())
            } else if meta.path.is_ident("access") {
                if access.is_some() {
                    return Err(meta.error("duplicate `access`"));
                }
                access = Some(meta.value()?.parse()?);
                Ok(())
            } else if meta.path.is_ident("requires") {
                if requires.is_some() {
                    return Err(meta.error("duplicate `requires`"));
                }
                let value = meta.value()?;
                let content;
                bracketed!(content in value);
                let paths = Punctuated::<Path, Token![,]>::parse_terminated(&content)?;
                requires = Some(paths.into_iter().collect());
                Ok(())
            } else if meta.path.is_ident("required") {
                required = true;
                Ok(())
            } else {
                Err(meta
                    .error("unsupported key; expected `name`, `access`, `requires`, or `required`"))
            }
        });
        parser.parse(attr)?;

        let name = name.ok_or_else(|| {
            Error::new(
                Span::call_site(),
                "missing `name = \"...\"`; a pass must declare its `NAME`",
            )
        })?;
        let access = access.ok_or_else(|| {
            Error::new(
                Span::call_site(),
                "missing `access = <rung>`; a pass must declare its capability rung",
            )
        })?;

        Ok(Self {
            name,
            access,
            requires: requires.unwrap_or_default(),
            required,
        })
    }
}

/// The parts extracted from the annotated `impl` block.
pub(crate) struct PassImpl {
    /// The pass type the trait is implemented for (e.g. `DcePass`).
    pub(crate) self_ty: Box<Type>,
    /// The `run` context parameter's binding identifier (usually `cx`).
    pub(crate) cx_ident: Ident,
    /// The author's original `run` body, re-emitted verbatim.
    pub(crate) body: Block,
    /// Any non-`run` items, re-emitted in a companion inherent `impl`.
    pub(crate) extras: Vec<ImplItem>,
}

impl PassImpl {
    /// A companion inherent `impl` carrying any helper items the author wrote
    /// alongside `run` (they cannot live in the generated trait impl). Empty when
    /// the block held only `run`.
    pub(crate) fn inherent(&self) -> TokenStream2 {
        if self.extras.is_empty() {
            TokenStream2::new()
        } else {
            let self_ty = &self.self_ty;
            let extras = &self.extras;
            quote! {
                impl #self_ty {
                    #(#extras)*
                }
            }
        }
    }
}

/// Parse the annotated `impl` and extract the pieces the expansion re-emits.
///
/// The author's written `cx: FnCx<Self>` / `-> IrResult<FnReport>` are readability
/// sentinels: only the receiver's presence, the second parameter's binding
/// identifier, and the body block are kept — the macro supplies the canonical
/// signature. This is why the sentinel type never needs to resolve.
pub(crate) fn parse_pass_impl(item: TokenStream) -> Result<PassImpl> {
    let item_impl: ItemImpl = syn::parse(item)?;

    if let Some((_, path, _)) = &item_impl.trait_ {
        return Err(Error::new_spanned(
            path,
            "expected an inherent `impl Pass { fn run(..) }` block, not a trait impl",
        ));
    }
    if !item_impl.generics.params.is_empty() || item_impl.generics.where_clause.is_some() {
        return Err(Error::new_spanned(
            &item_impl.generics,
            "the pass impl must be non-generic; the macro supplies the `<'ctx, B>` generics",
        ));
    }

    let self_ty = item_impl.self_ty;
    let mut run: Option<ImplItemFn> = None;
    let mut extras: Vec<ImplItem> = Vec::new();
    for item in item_impl.items {
        match item {
            ImplItem::Fn(function) if function.sig.ident == "run" => {
                if run.is_some() {
                    return Err(Error::new_spanned(
                        &function.sig.ident,
                        "duplicate `run`; the pass impl needs exactly one `fn run`",
                    ));
                }
                run = Some(function);
            }
            other => extras.push(other),
        }
    }
    let run = run.ok_or_else(|| {
        Error::new_spanned(
            &self_ty,
            "missing `fn run(&mut self, cx: ..) -> ..`; the pass impl must define it",
        )
    })?;

    let cx_ident = {
        let inputs = &run.sig.inputs;
        if inputs.len() != 2 {
            return Err(Error::new_spanned(
                inputs,
                "`run` takes exactly `&mut self` and one context parameter",
            ));
        }
        match &inputs[0] {
            FnArg::Receiver(_) => {}
            other => {
                return Err(Error::new_spanned(
                    other,
                    "`run` must take `&mut self` as its first parameter",
                ));
            }
        }
        match &inputs[1] {
            FnArg::Typed(pat_type) => match &*pat_type.pat {
                Pat::Ident(pat_ident) => pat_ident.ident.clone(),
                other => {
                    return Err(Error::new_spanned(
                        other,
                        "the `run` context parameter must be a plain identifier binding like `cx`",
                    ));
                }
            },
            other => {
                return Err(Error::new_spanned(
                    other,
                    "the `run` context parameter must be a plain identifier binding like `cx`",
                ));
            }
        }
    };

    Ok(PassImpl {
        self_ty,
        cx_ident,
        body: run.block,
        extras,
    })
}

/// Build the `Requires` tuple type: `(A, B,)` (trailing comma) or `()` when the
/// list is empty. The paths are the author's own analysis types, emitted as
/// written — not prefixed with the crate path.
pub(crate) fn requires_tuple(requires: &[Path]) -> TokenStream2 {
    quote! { ( #( #requires, )* ) }
}

/// Build the optional `const REQUIRED: bool = true;` item — emitted only when the
/// bare `required` flag was present, otherwise nothing (the trait default is
/// `false`).
pub(crate) fn required_const(required: bool) -> TokenStream2 {
    if required {
        quote! { const REQUIRED: bool = true; }
    } else {
        TokenStream2::new()
    }
}
