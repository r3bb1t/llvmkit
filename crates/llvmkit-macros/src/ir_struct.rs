use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::{Data, DeriveInput, Error, Fields, Ident, LitStr, Path, Result, parse_macro_input};

pub(crate) fn derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

struct Config {
    llvm_name: String,
    packed: bool,
    crate_path: TokenStream2,
}

fn expand(input: DeriveInput) -> Result<TokenStream2> {
    if !input.generics.params.is_empty() || input.generics.where_clause.is_some() {
        return Err(Error::new_spanned(
            input.generics,
            "IrStruct supports only non-generic structs",
        ));
    }

    let ident = input.ident;
    let vis = input.vis;
    let mut config = Config {
        llvm_name: ident.unraw().to_string(),
        packed: false,
        crate_path: default_crate_path(),
    };
    parse_container_attrs(&input.attrs, &mut config)?;

    let fields = match input.data {
        Data::Struct(data) => match data.fields {
            Fields::Named(fields) => fields.named,
            other => {
                return Err(Error::new_spanned(
                    other,
                    "IrStruct supports only structs with named fields",
                ));
            }
        },
        _ => {
            return Err(Error::new(
                ident.span(),
                "IrStruct can only be derived for structs",
            ));
        }
    };

    let mut field_idents = Vec::new();
    let mut field_tys = Vec::new();
    for field in fields {
        reject_field_attrs(&field.attrs)?;
        let Some(field_ident) = field.ident else {
            return Err(Error::new_spanned(field, "IrStruct requires named fields"));
        };
        field_idents.push(field_ident);
        field_tys.push(field.ty);
    }

    let value_ident = format_ident!("{}Value", ident);
    let build_trait = format_ident!("{}Build", ident);
    let ir = config.crate_path;
    let llvm_name = config.llvm_name;
    let packed = config.packed;
    let field_count = field_idents.len();
    let indices: Vec<u32> = (0..u32::try_from(field_count).map_err(|_| {
        Error::new(
            Span::call_site(),
            "IrStruct supports at most u32::MAX fields",
        )
    })?)
        .collect();
    // The generated items' generic parameters, and the capability one below.
    // `mixed_site` is hygienic for locals, not for type parameters, so the
    // reserved names do the work: a field whose type is spelled `B` or `Name` is
    // the user's own, and a parameter of that name would shadow it, and then
    // fail with an error about the derive's internals.
    let brand_param = Ident::new("__LlvmkitBrand", Span::mixed_site());
    let folder_param = Ident::new("__LlvmkitFolder", Span::mixed_site());
    let return_param = Ident::new("__LlvmkitReturn", Span::mixed_site());
    let name_generic = Ident::new("__LlvmkitName", Span::mixed_site());
    let build_generics: Vec<Ident> = indices
        .iter()
        .map(|idx| Ident::new(&format!("__LlvmkitField{idx}"), Span::mixed_site()))
        .collect();
    let builder_param = Ident::new("__llvmkit_builder", Span::mixed_site());
    let name_param = Ident::new("__llvmkit_name", Span::mixed_site());
    let base_name_ident = Ident::new("__llvmkit_base_name", Span::mixed_site());
    let aggregate_ident = Ident::new("__llvmkit_aggregate", Span::mixed_site());
    // The value type's one field: the llvmkit-owned carrier, which only
    // llvmkit mints after checking a value against the schema — so code that
    // spells the field still cannot put a value of another type in it.
    let schema_value_field = Ident::new("__llvmkit_schema_value", Span::mixed_site());
    let capability_param = Ident::new("__LlvmkitCapability", Span::mixed_site());
    // Each field's instruction name: `base.field`, or `field` when the base
    // name is empty. With no field there is no instruction to name.
    let base_name = if field_count == 0 {
        quote! { let _ = #name_param; }
    } else {
        quote! { let #base_name_ident = ::core::convert::AsRef::<str>::as_ref(&#name_param); }
    };
    let field_names: Vec<TokenStream2> = field_idents
        .iter()
        .map(|field_ident| {
            // `.unraw()`, as `try_ident` below: a field spelled `r#type` names
            // its instruction `type`, and `r#` is Rust's, not the IR's.
            let field_name = field_ident.unraw().to_string();
            quote! {
                if #base_name_ident.is_empty() {
                    ::std::string::String::from(#field_name)
                } else {
                    ::std::format!("{}.{}", #base_name_ident, #field_name)
                }
            }
        })
        .collect();
    let write_steps = field_idents
        .iter()
        .zip(indices.iter())
        .zip(field_names.iter())
        .map(|((field_ident, idx), field_name)| {
            quote! {
                let #aggregate_ident = #builder_param.write_field::<#ident, #idx, _, _>(
                    #aggregate_ident,
                    #field_ident,
                    #field_name,
                );
            }
        });
    let insert_steps = field_idents
        .iter()
        .zip(field_tys.iter())
        .zip(indices.iter())
        .zip(field_names.iter())
        .map(|(((field_ident, ty), idx), field_name)| {
            quote! {
                let #aggregate_ident = #builder_param.insert_field::<#ident, #ty, _, _, _>(
                    #aggregate_ident,
                    #field_ident,
                    #idx,
                    #field_name,
                )?;
            }
        });
    let build_doc = format!(
        "Build a `{ident}` value — `poison` of `%{llvm_name}` and one `insertvalue` per field, \
         folded by the builder's folder — with no `Result`, on a `UniqueBrand` module with \
         llvmkit's own folder. Each argument is a view handle of its field's type or a Rust \
         scalar literal (`FieldArg`). Any brand, any folder and a storable id build with \
         `try_build`; a `SpaceDyn` pointer is narrowed first with `PointerValue::try_from`."
    );
    let try_build_doc = format!(
        "Build a `{ident}` value on any brand, through any folder, from any argument a field \
         admits (`IntoIrField`, ids included): each is admitted at the builder's module and \
         checked against its field. A `Ptr<AS>` field admits a pointer known to be in `AS`, so \
         a `SpaceDyn` pointer is narrowed first with `PointerValue::try_from`."
    );

    let accessors = field_idents
        .iter()
        .zip(field_tys.iter())
        .zip(indices.iter())
        .map(|((field_ident, ty), idx)| {
            let field_name = field_ident.unraw().to_string();
            let try_ident = format_ident!("try_{}", field_ident.unraw());
            let read_doc = format!(
                "Read field `{field_name}` — no `Result` on a `UniqueBrand` module with \
                 llvmkit's own folder. Any brand and any folder read with `{try_ident}`."
            );
            let try_doc = format!(
                "Read field `{field_name}` on any brand, through any folder: the value is \
                 admitted at the builder's module and the folder's answer is checked."
            );
            quote! {
                #[doc = #read_doc]
                #[inline]
                #vis fn #field_ident<'m, #folder_param, #return_param>(
                    self,
                    builder: &#ir::IrBuilder<'m, 'ctx, #brand_param, #folder_param, #ir::Positioned, #return_param>,
                ) -> <#ty as #ir::IrField>::Value<'ctx, #brand_param, #ir::Mutable>
                where
                    #brand_param: #ir::UniqueBrand,
                    #folder_param: #ir::TrustedFolder<'ctx, #brand_param>,
                    #return_param: #ir::ReturnMarker,
                {
                    builder.read_field::<#ident, #idx, _>(self.#schema_value_field, #field_name)
                }

                #[doc = #try_doc]
                #[inline]
                #vis fn #try_ident<'m, #folder_param, #return_param>(
                    self,
                    builder: &#ir::IrBuilder<'m, 'ctx, #brand_param, #folder_param, #ir::Positioned, #return_param>,
                ) -> #ir::IrResult<<#ty as #ir::IrField>::Value<'ctx, #brand_param, #ir::Mutable>>
                where
                    #folder_param: #ir::IrBuilderFolder<'ctx, #brand_param>,
                    #return_param: #ir::ReturnMarker,
                {
                    builder.extract_field::<#ident, #ty, _, _>(self, #idx, #field_name)
                }
            }
        });

    let build_params: Vec<TokenStream2> = field_idents
        .iter()
        .zip(build_generics.iter())
        .map(|(field_ident, generic)| quote! { #field_ident: #generic })
        .collect();
    // `build` takes what lowers to the field's layout with no check;
    // `try_build` takes what a field admits, checked.
    let build_bounds: Vec<TokenStream2> = build_generics
        .iter()
        .zip(field_tys.iter())
        .map(|(generic, ty)| {
            quote! { #generic: #ir::FieldArg<'ctx, <#ty as #ir::IrField>::Layout, #brand_param> }
        })
        .collect();
    let try_build_bounds: Vec<TokenStream2> = build_generics
        .iter()
        .zip(field_tys.iter())
        .map(|(generic, ty)| quote! { #generic: #ir::IntoIrField<'ctx, #ty, #brand_param> })
        .collect();

    Ok(quote! {
        /// The value of a struct of this schema, at the capability of the
        /// struct value it wraps (D8). It holds an llvmkit-checked
        /// `SchemaValue`, so it is valid for the schema by construction.
        #vis struct #value_ident<
            'ctx,
            #brand_param: #ir::ModuleBrand,
            #capability_param: #ir::Capability = #ir::Mutable,
        > {
            #schema_value_field: #ir::SchemaValue<'ctx, #ident, #brand_param, #capability_param>,
        }

        // Bound-free std impls, spelled out instead of derived: a std derive
        // would demand `B: Clone`/`Debug`/… of the brand, and brands are bare
        // unit structs. Mirrors the `Branded` derive's output for this shape.
        #[automatically_derived]
        impl<'ctx, #brand_param: #ir::ModuleBrand, #capability_param: #ir::Capability> ::core::clone::Clone
            for #value_ident<'ctx, #brand_param, #capability_param>
        {
            #[inline]
            fn clone(&self) -> Self {
                *self
            }
        }
        #[automatically_derived]
        impl<'ctx, #brand_param: #ir::ModuleBrand, #capability_param: #ir::Capability> ::core::marker::Copy
            for #value_ident<'ctx, #brand_param, #capability_param>
        {
        }
        #[automatically_derived]
        impl<'ctx, #brand_param: #ir::ModuleBrand, #capability_param: #ir::Capability> ::core::fmt::Debug
            for #value_ident<'ctx, #brand_param, #capability_param>
        {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_struct(::core::stringify!(#value_ident))
                    .field("schema_value", &self.#schema_value_field)
                    .finish()
            }
        }
        #[automatically_derived]
        impl<'ctx, #brand_param: #ir::ModuleBrand, #capability_param: #ir::Capability> ::core::cmp::PartialEq
            for #value_ident<'ctx, #brand_param, #capability_param>
        {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                self.#schema_value_field == other.#schema_value_field
            }
        }
        #[automatically_derived]
        impl<'ctx, #brand_param: #ir::ModuleBrand, #capability_param: #ir::Capability> ::core::cmp::Eq
            for #value_ident<'ctx, #brand_param, #capability_param>
        {
        }
        #[automatically_derived]
        impl<'ctx, #brand_param: #ir::ModuleBrand, #capability_param: #ir::Capability> ::core::hash::Hash
            for #value_ident<'ctx, #brand_param, #capability_param>
        {
            #[inline]
            fn hash<__H: ::core::hash::Hasher>(&self, state: &mut __H) {
                ::core::hash::Hash::hash(&self.#schema_value_field, state);
            }
        }
        #[automatically_derived]
        impl<'ctx, #brand_param: #ir::ModuleBrand, #capability_param: #ir::Capability> #ir::CapabilityOf
            for #value_ident<'ctx, #brand_param, #capability_param>
        {
            type Capability = #capability_param;
        }

        impl<'ctx, #brand_param, #capability_param> #value_ident<'ctx, #brand_param, #capability_param>
        where
            #brand_param: #ir::ModuleBrand + 'ctx,
            #capability_param: #ir::Capability,
        {
            #[inline]
            #vis fn as_struct_value(self) -> #ir::StructValue<'ctx, #brand_param, #capability_param> {
                self.#schema_value_field.as_struct_value()
            }

            #[inline]
            #vis fn as_erased(self) -> #ir::Value<'ctx, #brand_param, #capability_param> {
                self.#schema_value_field.as_erased()
            }

            #(#accessors)*
        }

        /// The builds of this schema's value, from the struct type
        /// `get_or_insert_struct_of` answers: an extension of
        /// `StructOf<Self>`, which holds the proof that the type exists with
        /// this schema's body.
        #vis trait #build_trait<'ctx, #brand_param: #ir::ModuleBrand + 'ctx>: ::core::marker::Sized {
            #[doc = #build_doc]
            fn build<'m, #folder_param, #return_param, #name_generic, #(#build_generics,)*>(
                self,
                #builder_param: &#ir::IrBuilder<'m, 'ctx, #brand_param, #folder_param, #ir::Positioned, #return_param>,
                #(#build_params,)*
                #name_param: #name_generic,
            ) -> #value_ident<'ctx, #brand_param>
            where
                #brand_param: #ir::UniqueBrand,
                #folder_param: #ir::TrustedFolder<'ctx, #brand_param>,
                #return_param: #ir::ReturnMarker,
                #name_generic: ::core::convert::AsRef<str>,
                #(#build_bounds,)*;

            #[doc = #try_build_doc]
            ///
            /// # Errors
            ///
            /// `IrError::ForeignType` when another module minted the struct
            /// type, and each field's admission error — `IrError::ForeignValueId`
            /// for another module's value, a type mismatch for a value of
            /// another type.
            fn try_build<'m, #folder_param, #return_param, #name_generic, #(#build_generics,)*>(
                self,
                #builder_param: &#ir::IrBuilder<'m, 'ctx, #brand_param, #folder_param, #ir::Positioned, #return_param>,
                #(#build_params,)*
                #name_param: #name_generic,
            ) -> #ir::IrResult<#value_ident<'ctx, #brand_param>>
            where
                #folder_param: #ir::IrBuilderFolder<'ctx, #brand_param>,
                #return_param: #ir::ReturnMarker,
                #name_generic: ::core::convert::AsRef<str>,
                #(#try_build_bounds,)*;
        }

        impl<'ctx, #brand_param, #capability_param> #build_trait<'ctx, #brand_param>
            for #ir::StructOf<'ctx, #ident, #brand_param, #capability_param>
        where
            #brand_param: #ir::ModuleBrand + 'ctx,
            #capability_param: #ir::Capability,
        {
            #[inline]
            fn build<'m, #folder_param, #return_param, #name_generic, #(#build_generics,)*>(
                self,
                #builder_param: &#ir::IrBuilder<'m, 'ctx, #brand_param, #folder_param, #ir::Positioned, #return_param>,
                #(#build_params,)*
                #name_param: #name_generic,
            ) -> #value_ident<'ctx, #brand_param>
            where
                #brand_param: #ir::UniqueBrand,
                #folder_param: #ir::TrustedFolder<'ctx, #brand_param>,
                #return_param: #ir::ReturnMarker,
                #name_generic: ::core::convert::AsRef<str>,
                #(#build_bounds,)*
            {
                #base_name
                let #aggregate_ident = #builder_param.struct_poison(self);
                #(#write_steps)*
                <#value_ident<'ctx, #brand_param> as #ir::StructSchemaValue<'ctx, #ident, #brand_param, #ir::Mutable>>::from_schema_value(
                    #aggregate_ident,
                )
            }

            #[inline]
            fn try_build<'m, #folder_param, #return_param, #name_generic, #(#build_generics,)*>(
                self,
                #builder_param: &#ir::IrBuilder<'m, 'ctx, #brand_param, #folder_param, #ir::Positioned, #return_param>,
                #(#build_params,)*
                #name_param: #name_generic,
            ) -> #ir::IrResult<#value_ident<'ctx, #brand_param>>
            where
                #folder_param: #ir::IrBuilderFolder<'ctx, #brand_param>,
                #return_param: #ir::ReturnMarker,
                #name_generic: ::core::convert::AsRef<str>,
                #(#try_build_bounds,)*
            {
                #base_name
                let #aggregate_ident =
                    <#value_ident<'ctx, #brand_param> as #ir::StructSchemaValue<'ctx, #ident, #brand_param, #ir::Mutable>>::from_schema_value(
                        #builder_param.try_struct_poison(self)?,
                    );
                #(#insert_steps)*
                ::core::result::Result::Ok(#aggregate_ident)
            }
        }

        // A value of this schema fills a field laid out as this schema — a
        // nested struct's field — in a build without `?`.
        impl<'ctx, #brand_param, #capability_param> #ir::FieldArg<'ctx, #ident, #brand_param>
            for #value_ident<'ctx, #brand_param, #capability_param>
        where
            #brand_param: #ir::UniqueBrand + 'ctx,
            #capability_param: #ir::Capability,
        {
            #[inline]
            fn lower(self, module: #ir::ModuleRef<'ctx, #brand_param>) -> #ir::LoweredField<'ctx, #ident, #brand_param> {
                #ir::LoweredField::of(self, module)
            }
        }

        impl #ir::StructSchema for #ident {
            type Value<'ctx, #brand_param: #ir::ModuleBrand + 'ctx, #capability_param: #ir::Capability> =
                #value_ident<'ctx, #brand_param, #capability_param>;
            type Fields = (#(#field_tys,)*);

            const NAME: &'static str = #llvm_name;
            const PACKED: bool = #packed;
        }

        impl<'ctx, #brand_param, #capability_param> #ir::StructSchemaValue<'ctx, #ident, #brand_param, #capability_param>
            for #value_ident<'ctx, #brand_param, #capability_param>
        where
            #brand_param: #ir::ModuleBrand + 'ctx,
            #capability_param: #ir::Capability,
        {
            #[inline]
            fn from_schema_value(
                value: #ir::SchemaValue<'ctx, #ident, #brand_param, #capability_param>,
            ) -> Self {
                Self { #schema_value_field: value }
            }

            #[inline]
            fn schema_value(self) -> #ir::SchemaValue<'ctx, #ident, #brand_param, #capability_param> {
                self.#schema_value_field
            }
        }

        impl<'ctx, #brand_param, #capability_param>
            ::core::convert::TryFrom<#ir::Argument<'ctx, #brand_param, #capability_param>>
            for #value_ident<'ctx, #brand_param, #capability_param>
        where
            #brand_param: #ir::ModuleBrand + 'ctx,
            #capability_param: #ir::Capability,
        {
            type Error = #ir::IrError;

            #[inline]
            fn try_from(value: #ir::Argument<'ctx, #brand_param, #capability_param>) -> #ir::IrResult<Self> {
                Ok(Self { #schema_value_field: #ir::SchemaValue::try_new(value)? })
            }
        }

        impl<'ctx, #brand_param, #capability_param>
            ::core::convert::TryFrom<#ir::StructValue<'ctx, #brand_param, #capability_param>>
            for #value_ident<'ctx, #brand_param, #capability_param>
        where
            #brand_param: #ir::ModuleBrand + 'ctx,
            #capability_param: #ir::Capability,
        {
            type Error = #ir::IrError;

            #[inline]
            fn try_from(
                value: #ir::StructValue<'ctx, #brand_param, #capability_param>,
            ) -> #ir::IrResult<Self> {
                Ok(Self { #schema_value_field: #ir::SchemaValue::try_new(value)? })
            }
        }

        impl<'ctx, #brand_param, #capability_param>
            ::core::convert::TryFrom<#ir::Value<'ctx, #brand_param, #capability_param>>
            for #value_ident<'ctx, #brand_param, #capability_param>
        where
            #brand_param: #ir::ModuleBrand + 'ctx,
            #capability_param: #ir::Capability,
        {
            type Error = #ir::IrError;

            #[inline]
            fn try_from(value: #ir::Value<'ctx, #brand_param, #capability_param>) -> #ir::IrResult<Self> {
                Ok(Self { #schema_value_field: #ir::SchemaValue::try_new(value)? })
            }
        }

        impl<'ctx, #brand_param, #capability_param>
            ::core::convert::TryFrom<#ir::Constant<'ctx, #brand_param, #capability_param>>
            for #value_ident<'ctx, #brand_param, #capability_param>
        where
            #brand_param: #ir::ModuleBrand + 'ctx,
            #capability_param: #ir::Capability,
        {
            type Error = #ir::IrError;

            #[inline]
            fn try_from(value: #ir::Constant<'ctx, #brand_param, #capability_param>) -> #ir::IrResult<Self> {
                Ok(Self { #schema_value_field: #ir::SchemaValue::try_new(value)? })
            }
        }

        impl<'ctx, #brand_param> ::core::convert::TryFrom<#ir::Instruction<'ctx, #ir::instruction::state::Attached, #brand_param>>
            for #value_ident<'ctx, #brand_param>
        where
            #brand_param: #ir::ModuleBrand + 'ctx,
        {
            type Error = #ir::IrError;

            #[inline]
            fn try_from(
                value: #ir::Instruction<'ctx, #ir::instruction::state::Attached, #brand_param>,
            ) -> #ir::IrResult<Self> {
                Ok(Self { #schema_value_field: #ir::SchemaValue::try_new(value)? })
            }
        }

        // The three operand lifts admit the value at the receiving module
        // through the checked door — `IrError::ForeignValueId` for a value
        // another module minted — and re-mint it at that module's reference,
        // so a value of either capability is an operand: naming it is not
        // mutating it.
        impl<'ctx, #brand_param, #capability_param> #ir::IntoLayoutValue<'ctx, #ident, #brand_param>
            for #value_ident<'ctx, #brand_param, #capability_param>
        where
            #brand_param: #ir::ModuleBrand + 'ctx,
            #capability_param: #ir::Capability,
        {
            #[inline]
            fn into_layout_value(
                self,
                module: #ir::ModuleRef<'ctx, #brand_param>,
            ) -> #ir::IrResult<#ir::Value<'ctx, #brand_param>> {
                #ir::IntoErasedValue::into_erased_value(
                    self.#schema_value_field.as_struct_value(),
                    module,
                )
            }
        }

        impl<'ctx, #brand_param: #ir::ModuleBrand + 'ctx, #capability_param: #ir::Capability>
            #ir::IntoCallArg<'ctx, #ident, #brand_param> for #value_ident<'ctx, #brand_param, #capability_param>
        {
            #[inline]
            fn into_call_arg(
                self,
                module: #ir::ModuleRef<'ctx, #brand_param>,
            ) -> #ir::IrResult<#ir::Value<'ctx, #brand_param>> {
                #ir::IntoErasedValue::into_erased_value(
                    self.#schema_value_field.as_struct_value(),
                    module,
                )
            }
        }

        impl<'ctx, #brand_param, #capability_param> #ir::ir_builder::IntoReturnValue<'ctx, #ir::Dyn, #brand_param>
            for #value_ident<'ctx, #brand_param, #capability_param>
        where
            #brand_param: #ir::ModuleBrand + 'ctx,
            #capability_param: #ir::Capability,
        {
            #[inline]
            fn into_return_value(
                self,
                module: #ir::ModuleRef<'ctx, #brand_param>,
            ) -> #ir::IrResult<#ir::Value<'ctx, #brand_param>> {
                #ir::IntoErasedValue::into_erased_value(
                    self.#schema_value_field.as_struct_value(),
                    module,
                )
            }
        }
    })
}

fn parse_container_attrs(attrs: &[syn::Attribute], config: &mut Config) -> Result<()> {
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("llvmkit")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("packed") {
                config.packed = true;
                return Ok(());
            }
            if meta.path.is_ident("name") {
                let value: LitStr = meta.value()?.parse()?;
                if value.value().is_empty() {
                    // `StructType::create(Context, "")` makes an anonymous
                    // struct, not one printed `%""`, so an empty name names
                    // no identified struct.
                    return Err(Error::new_spanned(
                        value,
                        "IrStruct name must not be empty: an identified struct needs a name",
                    ));
                }
                config.llvm_name = value.value();
                return Ok(());
            }
            if meta.path.is_ident("crate") {
                let value: Path = meta.value()?.parse()?;
                config.crate_path = quote! { #value };
                return Ok(());
            }
            Err(meta.error("unsupported llvmkit attribute; expected name, packed, or crate"))
        })?;
    }
    Ok(())
}

fn reject_field_attrs(attrs: &[syn::Attribute]) -> Result<()> {
    if let Some(attr) = attrs.iter().find(|attr| attr.path().is_ident("llvmkit")) {
        return Err(Error::new_spanned(
            attr,
            "field-level llvmkit attributes are not supported",
        ));
    }
    Ok(())
}

pub(crate) fn default_crate_path() -> TokenStream2 {
    match proc_macro_crate::crate_name("llvmkit-ir") {
        Ok(proc_macro_crate::FoundCrate::Itself) => quote! { ::llvmkit_ir },
        Ok(proc_macro_crate::FoundCrate::Name(name)) => {
            let ident = Ident::new(&name, Span::call_site());
            quote! { ::#ident }
        }
        Err(_) => match proc_macro_crate::crate_name("llvmkit") {
            Ok(proc_macro_crate::FoundCrate::Itself) => quote! { ::llvmkit::ir },
            Ok(proc_macro_crate::FoundCrate::Name(name)) => {
                let ident = Ident::new(&name, Span::call_site());
                quote! { ::#ident::ir }
            }
            Err(_) => quote! { ::llvmkit_ir },
        },
    }
}
