//! `#[derive(Branded)]` — std-trait impls without the inferred bounds.
//!
//! Std's `#[derive(Clone)]` on `struct V<'ctx, B: ModuleBrand>` emits
//! `impl<'ctx, B: ModuleBrand + Clone> Clone …`: every type parameter gains a
//! bound whether or not it appears in a field. llvmkit's branded view types
//! carry their brand (and their width/kind markers) only as `PhantomData`, so
//! those bounds are always spurious — and forcing them is what put four
//! supertraits on `ModuleBrand`. This derive emits the same impls with the
//! item's generics copied verbatim and **nothing added**, so a brand can be a
//! bare `struct LiftedBin;`.
//!
//! Trait selection: without a helper attribute the set is `Clone`, `Copy`,
//! `Debug`, `PartialEq`, `Eq`, `Hash`; `#[branded(…)]` names an explicit
//! subset (which may add `Default`, structs only, and `PartialOrd` / `Ord`).
//! `Debug` skips phantom fields — any field whose type's last path segment is
//! `PhantomData` or `Invariant` — matching the hand-written `decl_value_id!`
//! convention that phantoms never print. `PartialEq`, `Hash`, `PartialOrd`
//! and `Ord` are generated from one shared field walk over *all* fields, so
//! the `Hash`/`Eq`/`Ord` contracts cannot drift; a `PhantomData` compares
//! equal, hashes to nothing and orders `Equal`, which keeps that walk total.
//! `Copy` stays honest through the compiler: a non-`Copy` field in a
//! `Copy`-requesting type is still `E0204`.
//!
//! Ordering is opt-in rather than default because most branded types are
//! views whose fields are arena indices with no meaningful order. Where it
//! *is* meaningful — an id that is `(ModuleId, slot)` — a lexicographic order
//! is deterministic across runs, which is what makes a `BTreeMap` keyed by
//! one safe to iterate for pass output. Enum ordering is by declaration
//! order first, then fields, exactly like the std derive; the variant rank is
//! spelled as a `match` returning `usize` rather than a discriminant cast,
//! since llvmkit forbids `as`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Error, Fields, Ident, Token, Type};

/// The trait set a `Branded` expansion emits.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Trait {
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Default,
}

impl Trait {
    fn from_ident(ident: &Ident) -> Option<Self> {
        match ident.to_string().as_str() {
            "Clone" => Some(Self::Clone),
            "Copy" => Some(Self::Copy),
            "Debug" => Some(Self::Debug),
            "PartialEq" => Some(Self::PartialEq),
            "Eq" => Some(Self::Eq),
            "Hash" => Some(Self::Hash),
            "PartialOrd" => Some(Self::PartialOrd),
            "Ord" => Some(Self::Ord),
            "Default" => Some(Self::Default),
            _ => None,
        }
    }
}

pub(crate) fn derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    expand(&input)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

fn expand(input: &DeriveInput) -> Result<TokenStream, Error> {
    let traits = requested_traits(input)?;

    if matches!(input.data, Data::Union(_)) {
        return Err(Error::new(
            input.ident.span(),
            "`Branded` supports structs and enums, not unions",
        ));
    }
    if traits.contains(&Trait::Default) && matches!(input.data, Data::Enum(_)) {
        return Err(Error::new(
            input.ident.span(),
            "`Branded` does not derive `Default` for enums; only structs request it",
        ));
    }

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let mut out = TokenStream::new();
    for tr in &traits {
        let body = match tr {
            Trait::Clone => clone_impl(input, traits.contains(&Trait::Copy)),
            Trait::Copy => quote! {},
            Trait::Debug => debug_impl(input),
            Trait::PartialEq => partial_eq_impl(input),
            Trait::Eq => quote! {},
            Trait::Hash => hash_impl(input),
            Trait::PartialOrd => partial_ord_impl(input),
            Trait::Ord => ord_impl(input),
            Trait::Default => default_impl(input),
        };
        let header = match tr {
            Trait::Clone => quote! { ::core::clone::Clone },
            Trait::Copy => quote! { ::core::marker::Copy },
            Trait::Debug => quote! { ::core::fmt::Debug },
            Trait::PartialEq => quote! { ::core::cmp::PartialEq },
            Trait::Eq => quote! { ::core::cmp::Eq },
            Trait::Hash => quote! { ::core::hash::Hash },
            Trait::PartialOrd => quote! { ::core::cmp::PartialOrd },
            Trait::Ord => quote! { ::core::cmp::Ord },
            Trait::Default => quote! { ::core::default::Default },
        };
        out.extend(quote! {
            #[automatically_derived]
            impl #impl_generics #header for #name #ty_generics #where_clause {
                #body
            }
        });
    }
    Ok(out)
}

/// Resolve the requested trait set: the default six, or the `#[branded(…)]`
/// list. Duplicates, unknown names, `Copy` without `Clone`, and `Eq` without
/// `PartialEq` are pinpointed errors.
fn requested_traits(input: &DeriveInput) -> Result<Vec<Trait>, Error> {
    let attrs: Vec<_> = input
        .attrs
        .iter()
        .filter(|a| a.path().is_ident("branded"))
        .collect();
    if attrs.len() > 1 {
        return Err(Error::new(
            attrs[1].span(),
            "at most one `#[branded(…)]` attribute per type",
        ));
    }
    let Some(attr) = attrs.first() else {
        return Ok(vec![
            Trait::Clone,
            Trait::Copy,
            Trait::Debug,
            Trait::PartialEq,
            Trait::Eq,
            Trait::Hash,
        ]);
    };

    let idents = attr.parse_args_with(Punctuated::<Ident, Token![,]>::parse_terminated)?;
    if idents.is_empty() {
        return Err(Error::new(
            attr.span(),
            "`#[branded(…)]` lists at least one trait; drop the attribute for the default set",
        ));
    }
    let mut traits = Vec::new();
    for ident in &idents {
        let tr = Trait::from_ident(ident).ok_or_else(|| {
            Error::new(
                ident.span(),
                "unrecognized trait; `Branded` derives Clone, Copy, Debug, PartialEq, Eq, Hash, \
                 PartialOrd, Ord, and Default",
            )
        })?;
        if traits.contains(&tr) {
            return Err(Error::new(
                ident.span(),
                "duplicate trait in `#[branded(…)]`",
            ));
        }
        traits.push(tr);
    }
    if traits.contains(&Trait::Copy) && !traits.contains(&Trait::Clone) {
        return Err(Error::new(
            attr.span(),
            "`Copy` requires `Clone`; add it to `#[branded(…)]`",
        ));
    }
    if traits.contains(&Trait::Eq) && !traits.contains(&Trait::PartialEq) {
        return Err(Error::new(
            attr.span(),
            "`Eq` requires `PartialEq`; add it to `#[branded(…)]`",
        ));
    }
    if traits.contains(&Trait::PartialOrd) && !traits.contains(&Trait::PartialEq) {
        return Err(Error::new(
            attr.span(),
            "`PartialOrd` requires `PartialEq`; add it to `#[branded(…)]`",
        ));
    }
    if traits.contains(&Trait::Ord)
        && !(traits.contains(&Trait::Eq) && traits.contains(&Trait::PartialOrd))
    {
        return Err(Error::new(
            attr.span(),
            "`Ord` requires `Eq` and `PartialOrd`; add them to `#[branded(…)]`",
        ));
    }
    Ok(traits)
}

/// A field is phantom iff its type's last path segment is `PhantomData` or
/// `Invariant` (llvmkit's `PhantomData<fn(B) -> B>` alias). Phantoms are
/// skipped by `Debug` only; every other trait walks all fields.
fn is_phantom(ty: &Type) -> bool {
    let Type::Path(p) = ty else { return false };
    p.path
        .segments
        .last()
        .is_some_and(|seg| seg.ident == "PhantomData" || seg.ident == "Invariant")
}

/// Bind-pattern identifiers for a variant's fields: `f0`, `f1`, … (optionally
/// prefixed, for the `PartialEq` two-sided match).
fn bind_idents(fields: &Fields, prefix: &str) -> Vec<Ident> {
    (0..fields.len())
        .map(|i| format_ident!("{prefix}{i}"))
        .collect()
}

/// A pattern matching `path { … }` / `path(…)` / `path`, binding each field to
/// the corresponding ident from `binds`.
fn variant_pattern(path: TokenStream, fields: &Fields, binds: &[Ident]) -> TokenStream {
    match fields {
        Fields::Named(named) => {
            let names = named.named.iter().map(|f| &f.ident);
            quote! { #path { #( #names: #binds ),* } }
        }
        Fields::Unnamed(_) => quote! { #path ( #( #binds ),* ) },
        Fields::Unit => path,
    }
}

fn clone_impl(input: &DeriveInput, is_copy: bool) -> TokenStream {
    if is_copy {
        return quote! {
            #[inline]
            fn clone(&self) -> Self {
                *self
            }
        };
    }
    let body = match &input.data {
        Data::Struct(data) => {
            let binds = bind_idents(&data.fields, "f");
            let pattern = variant_pattern(quote! { Self }, &data.fields, &binds);
            let make = variant_pattern(quote! { Self }, &data.fields, &binds);
            let clones = binds
                .iter()
                .map(|b| quote! { let #b = ::core::clone::Clone::clone(#b); });
            quote! {
                let #pattern = self;
                #( #clones )*
                #make
            }
        }
        Data::Enum(data) => {
            let arms = data.variants.iter().map(|v| {
                let vname = &v.ident;
                let binds = bind_idents(&v.fields, "f");
                let pattern = variant_pattern(quote! { Self::#vname }, &v.fields, &binds);
                let make = variant_pattern(quote! { Self::#vname }, &v.fields, &binds);
                let clones = binds
                    .iter()
                    .map(|b| quote! { let #b = ::core::clone::Clone::clone(#b); });
                quote! { #pattern => { #( #clones )* #make } }
            });
            quote! {
                match self {
                    #( #arms, )*
                }
            }
        }
        Data::Union(_) => unreachable!("rejected in expand"),
    };
    quote! {
        #[inline]
        fn clone(&self) -> Self {
            #body
        }
    }
}

fn partial_eq_impl(input: &DeriveInput) -> TokenStream {
    let body = match &input.data {
        Data::Struct(data) => {
            let l = bind_idents(&data.fields, "l");
            let r = bind_idents(&data.fields, "r");
            let lpat = variant_pattern(quote! { Self }, &data.fields, &l);
            let rpat = variant_pattern(quote! { Self }, &data.fields, &r);
            let cmps = l.iter().zip(&r).map(|(a, b)| quote! { && #a == #b });
            quote! {
                let #lpat = self;
                let #rpat = other;
                true #( #cmps )*
            }
        }
        Data::Enum(data) => {
            let arms = data.variants.iter().map(|v| {
                let vname = &v.ident;
                let l = bind_idents(&v.fields, "l");
                let r = bind_idents(&v.fields, "r");
                let lpat = variant_pattern(quote! { Self::#vname }, &v.fields, &l);
                let rpat = variant_pattern(quote! { Self::#vname }, &v.fields, &r);
                let cmps = l.iter().zip(&r).map(|(a, b)| quote! { && #a == #b });
                quote! { (#lpat, #rpat) => true #( #cmps )* }
            });
            let fallthrough = if data.variants.len() > 1 {
                quote! { _ => false, }
            } else {
                quote! {}
            };
            quote! {
                match (self, other) {
                    #( #arms, )*
                    #fallthrough
                }
            }
        }
        Data::Union(_) => unreachable!("rejected in expand"),
    };
    quote! {
        #[inline]
        fn eq(&self, other: &Self) -> bool {
            #body
        }
    }
}

fn hash_impl(input: &DeriveInput) -> TokenStream {
    let body = match &input.data {
        Data::Struct(data) => {
            let binds = bind_idents(&data.fields, "f");
            let pattern = variant_pattern(quote! { Self }, &data.fields, &binds);
            let hashes = binds
                .iter()
                .map(|b| quote! { ::core::hash::Hash::hash(#b, state); });
            quote! {
                let #pattern = self;
                #( #hashes )*
            }
        }
        Data::Enum(data) => {
            let arms = data.variants.iter().map(|v| {
                let vname = &v.ident;
                let binds = bind_idents(&v.fields, "f");
                let pattern = variant_pattern(quote! { Self::#vname }, &v.fields, &binds);
                let hashes = binds
                    .iter()
                    .map(|b| quote! { ::core::hash::Hash::hash(#b, state); });
                quote! { #pattern => { #( #hashes )* } }
            });
            quote! {
                ::core::hash::Hash::hash(&::core::mem::discriminant(self), state);
                match self {
                    #( #arms, )*
                }
            }
        }
        Data::Union(_) => unreachable!("rejected in expand"),
    };
    quote! {
        #[inline]
        fn hash<__H: ::core::hash::Hasher>(&self, state: &mut __H) {
            #body
        }
    }
}

/// A `match` mapping each variant to its declaration rank, used as the
/// leading comparison key for enum ordering. Spelled as a match rather than
/// `discriminant as usize` because llvmkit forbids `as` casts, and a
/// `#[repr]`-less enum has no stable numeric discriminant to cast anyway.
fn variant_rank(data: &syn::DataEnum) -> TokenStream {
    let arms = data.variants.iter().enumerate().map(|(rank, v)| {
        let vname = &v.ident;
        let pattern = match &v.fields {
            Fields::Named(_) => quote! { Self::#vname { .. } },
            Fields::Unnamed(_) => quote! { Self::#vname ( .. ) },
            Fields::Unit => quote! { Self::#vname },
        };
        quote! { #pattern => #rank }
    });
    quote! {
        |__v: &Self| -> usize {
            match __v {
                #( #arms, )*
            }
        }
    }
}

/// Fold a field-pair list into the nested `match` chain both orderings use:
/// compare the first pair, and only fall through to the rest when it ties.
/// `equal` is the all-tied result, `compare` builds one pair's comparison,
/// and `tie` is the pattern that means "keep going".
fn ordering_chain<F>(
    lhs: &[Ident],
    rhs: &[Ident],
    equal: TokenStream,
    tie: TokenStream,
    compare: F,
) -> TokenStream
where
    F: Fn(&Ident, &Ident) -> TokenStream,
{
    let mut chain = equal;
    for (l, r) in lhs.iter().zip(rhs).rev() {
        let step = compare(l, r);
        chain = quote! {
            match #step {
                #tie => { #chain }
                __ordering => __ordering,
            }
        };
    }
    chain
}

fn ord_impl(input: &DeriveInput) -> TokenStream {
    let equal = quote! { ::core::cmp::Ordering::Equal };
    let compare = |l: &Ident, r: &Ident| quote! { ::core::cmp::Ord::cmp(#l, #r) };
    let body = match &input.data {
        Data::Struct(data) => {
            let l = bind_idents(&data.fields, "l");
            let r = bind_idents(&data.fields, "r");
            let lpat = variant_pattern(quote! { Self }, &data.fields, &l);
            let rpat = variant_pattern(quote! { Self }, &data.fields, &r);
            let chain = ordering_chain(&l, &r, equal.clone(), equal, compare);
            quote! {
                let #lpat = self;
                let #rpat = other;
                #chain
            }
        }
        Data::Enum(data) => {
            let arms = data.variants.iter().map(|v| {
                let vname = &v.ident;
                let l = bind_idents(&v.fields, "l");
                let r = bind_idents(&v.fields, "r");
                let lpat = variant_pattern(quote! { Self::#vname }, &v.fields, &l);
                let rpat = variant_pattern(quote! { Self::#vname }, &v.fields, &r);
                let chain = ordering_chain(&l, &r, equal.clone(), equal.clone(), compare);
                quote! { (#lpat, #rpat) => { #chain } }
            });
            if data.variants.len() > 1 {
                let rank = variant_rank(data);
                quote! {
                    match (self, other) {
                        #( #arms, )*
                        _ => {
                            let __rank = #rank;
                            ::core::cmp::Ord::cmp(&__rank(self), &__rank(other))
                        }
                    }
                }
            } else {
                quote! {
                    match (self, other) {
                        #( #arms, )*
                    }
                }
            }
        }
        Data::Union(_) => unreachable!("rejected in expand"),
    };
    quote! {
        #[inline]
        fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
            #body
        }
    }
}

fn partial_ord_impl(input: &DeriveInput) -> TokenStream {
    let equal = quote! { ::core::option::Option::Some(::core::cmp::Ordering::Equal) };
    let compare = |l: &Ident, r: &Ident| quote! { ::core::cmp::PartialOrd::partial_cmp(#l, #r) };
    let body = match &input.data {
        Data::Struct(data) => {
            let l = bind_idents(&data.fields, "l");
            let r = bind_idents(&data.fields, "r");
            let lpat = variant_pattern(quote! { Self }, &data.fields, &l);
            let rpat = variant_pattern(quote! { Self }, &data.fields, &r);
            let chain = ordering_chain(&l, &r, equal.clone(), equal, compare);
            quote! {
                let #lpat = self;
                let #rpat = other;
                #chain
            }
        }
        Data::Enum(data) => {
            let arms = data.variants.iter().map(|v| {
                let vname = &v.ident;
                let l = bind_idents(&v.fields, "l");
                let r = bind_idents(&v.fields, "r");
                let lpat = variant_pattern(quote! { Self::#vname }, &v.fields, &l);
                let rpat = variant_pattern(quote! { Self::#vname }, &v.fields, &r);
                let chain = ordering_chain(&l, &r, equal.clone(), equal.clone(), compare);
                quote! { (#lpat, #rpat) => { #chain } }
            });
            if data.variants.len() > 1 {
                let rank = variant_rank(data);
                quote! {
                    match (self, other) {
                        #( #arms, )*
                        _ => {
                            let __rank = #rank;
                            ::core::cmp::PartialOrd::partial_cmp(&__rank(self), &__rank(other))
                        }
                    }
                }
            } else {
                quote! {
                    match (self, other) {
                        #( #arms, )*
                    }
                }
            }
        }
        Data::Union(_) => unreachable!("rejected in expand"),
    };
    quote! {
        #[inline]
        fn partial_cmp(&self, other: &Self) -> ::core::option::Option<::core::cmp::Ordering> {
            #body
        }
    }
}

fn debug_impl(input: &DeriveInput) -> TokenStream {
    let body = match &input.data {
        Data::Struct(data) => {
            let name = input.ident.to_string();
            let binds = bind_idents(&data.fields, "f");
            let pattern = variant_pattern(quote! { Self }, &data.fields, &binds);
            let build = debug_builder(&name, &data.fields, &binds);
            quote! {
                let #pattern = self;
                #build
            }
        }
        Data::Enum(data) => {
            let arms = data.variants.iter().map(|v| {
                let vname = &v.ident;
                let label = vname.to_string();
                let binds = bind_idents(&v.fields, "f");
                let pattern = variant_pattern(quote! { Self::#vname }, &v.fields, &binds);
                let build = debug_builder(&label, &v.fields, &binds);
                quote! { #pattern => { #build } }
            });
            quote! {
                match self {
                    #( #arms, )*
                }
            }
        }
        Data::Union(_) => unreachable!("rejected in expand"),
    };
    quote! {
        fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
            #body
        }
    }
}

/// The `Formatter` builder chain for one struct or variant, phantoms skipped.
/// A shape whose non-phantom fields are named uses `debug_struct`; unnamed
/// uses `debug_tuple`; unit (or all-phantom) prints the bare name.
fn debug_builder(label: &str, fields: &Fields, binds: &[Ident]) -> TokenStream {
    match fields {
        Fields::Named(named) => {
            let printed: Vec<_> = named
                .named
                .iter()
                .zip(binds)
                .filter(|(field, _)| !is_phantom(&field.ty))
                .map(|(field, bind)| {
                    let label = field.ident.as_ref().expect("named field").to_string();
                    quote! { .field(#label, #bind) }
                })
                .collect();
            if printed.is_empty() {
                quote! { f.write_str(#label) }
            } else {
                quote! { f.debug_struct(#label) #( #printed )* .finish() }
            }
        }
        Fields::Unnamed(unnamed) => {
            let printed: Vec<_> = unnamed
                .unnamed
                .iter()
                .zip(binds)
                .filter(|(field, _)| !is_phantom(&field.ty))
                .map(|(_, bind)| quote! { .field(#bind) })
                .collect();
            if printed.is_empty() {
                quote! { f.write_str(#label) }
            } else {
                quote! { f.debug_tuple(#label) #( #printed )* .finish() }
            }
        }
        Fields::Unit => quote! { f.write_str(#label) },
    }
}

fn default_impl(input: &DeriveInput) -> TokenStream {
    let Data::Struct(data) = &input.data else {
        unreachable!("enum Default rejected in expand");
    };
    let body = match &data.fields {
        Fields::Named(named) => {
            let inits = named.named.iter().map(|f| {
                let name = &f.ident;
                quote! { #name: ::core::default::Default::default() }
            });
            quote! { Self { #( #inits ),* } }
        }
        Fields::Unnamed(unnamed) => {
            let inits = unnamed
                .unnamed
                .iter()
                .map(|_| quote! { ::core::default::Default::default() });
            quote! { Self( #( #inits ),* ) }
        }
        Fields::Unit => quote! { Self },
    };
    quote! {
        #[inline]
        fn default() -> Self {
            #body
        }
    }
}
