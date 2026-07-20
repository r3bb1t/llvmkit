use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
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

struct BuildStepContext<'a> {
    ir: &'a TokenStream2,
    module_param: &'a Ident,
    builder_param: &'a Ident,
    base_name_ident: &'a Ident,
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
        llvm_name: ident.to_string(),
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
    let build_generics: Vec<Ident> = indices
        .iter()
        .map(|idx| format_ident!("Field{}", idx))
        .collect();
    let module_param = Ident::new("__llvmkit_module", Span::mixed_site());
    let builder_param = Ident::new("__llvmkit_builder", Span::mixed_site());
    let name_param = Ident::new("__llvmkit_name", Span::mixed_site());
    let base_name_ident = Ident::new("__llvmkit_base_name", Span::mixed_site());
    let value_steps = build_value_steps(
        &ident,
        &field_idents,
        &field_tys,
        &indices,
        BuildStepContext {
            ir: &ir,
            module_param: &module_param,
            builder_param: &builder_param,
            base_name_ident: &base_name_ident,
        },
    );
    let build_return = if field_count == 0 {
        quote! {
            let __llvmkit_ty = <#ident as #ir::StructSchema>::ir_type(#module_param)?;
            let __llvmkit_raw = <#ir::StructValue<'ctx, B> as ::core::convert::TryFrom<#ir::Constant<'ctx, B>>>::try_from(
                __llvmkit_ty.as_type().get_poison().as_constant(),
            )?;
            Ok(Self { raw: __llvmkit_raw })
        }
    } else {
        let last = Ident::new(
            &format!("__llvmkit_value_{}", field_count - 1),
            Span::mixed_site(),
        );
        quote! {
            #(#value_steps)*
            Ok(#last)
        }
    };

    let accessors = field_idents
        .iter()
        .zip(field_tys.iter())
        .zip(indices.iter())
        .map(|((field_ident, ty), idx)| {
            let field_name = field_ident.to_string();
            quote! {
                #[inline]
                #vis fn #field_ident<'m, F, R>(
                    self,
                    builder: &#ir::IRBuilder<'m, 'ctx, B, F, #ir::Positioned, R>,
                ) -> #ir::IrResult<<#ty as #ir::IrField>::Value<'ctx, B>>
                where
                    F: #ir::IRBuilderFolder<'ctx, B>,
                    R: #ir::ReturnMarker,
                    #ty: #ir::IrField,
                {
                    builder.build_extract_field::<#ident, #ty, _, _>(self, #idx, #field_name)
                }
            }
        });

    let matches_terms = field_tys
        .iter()
        .zip(indices.iter())
        .map(|(ty, idx)| {
            let idx_usize = usize::try_from(*idx).map_err(|_| {
                Error::new(Span::call_site(), "IrStruct field index does not fit usize")
            })?;
            Ok(quote! { <#ty as #ir::IrField>::matches_ir_type(fields[#idx_usize]) })
        })
        .collect::<Result<Vec<_>>>()?;

    let build_params = field_idents
        .iter()
        .zip(build_generics.iter())
        .map(|(field_ident, generic)| quote! { #field_ident: #generic });
    let build_bounds = build_generics
        .iter()
        .zip(field_tys.iter())
        .map(|(generic, ty)| quote! { #generic: #ir::IntoIrField<'ctx, #ty, B> });

    Ok(quote! {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #vis struct #value_ident<'ctx, B: #ir::ModuleBrand = #ir::Brand<'ctx>> {
            raw: #ir::StructValue<'ctx, B>,
        }

        impl<'ctx, B> #value_ident<'ctx, B>
        where
            B: #ir::ModuleBrand + 'ctx,
        {
            #[inline]
            #vis fn as_struct_value(self) -> #ir::StructValue<'ctx, B> {
                self.raw
            }

            #[inline]
            #vis fn into_erased(self) -> #ir::Value<'ctx, B> {
                self.raw.into_erased()
            }

            #(#accessors)*

            #[inline]
            #vis fn build<'m, F, R, Name, #(#build_generics,)*>(
                #module_param: &#ir::Module<'ctx, B, #ir::Unverified>,
                #builder_param: &#ir::IRBuilder<'m, 'ctx, B, F, #ir::Positioned, R>,
                #(#build_params,)*
                #name_param: Name,
            ) -> #ir::IrResult<Self>
            where
                F: #ir::IRBuilderFolder<'ctx, B>,
                R: #ir::ReturnMarker,
                Name: ::core::convert::AsRef<str>,
                #(#build_bounds,)*
            {
                let #base_name_ident = #name_param.as_ref();
                #build_return
            }
        }

        impl #ir::StructSchema for #ident {
            type Value<'ctx, B: #ir::ModuleBrand + 'ctx> = #value_ident<'ctx, B>;
            type FieldParams = (#(#field_tys,)*);

            const NAME: &'static str = #llvm_name;
            const PACKED: bool = #packed;

            fn field_types<'ctx, B>(
                module: &#ir::Module<'ctx, B, #ir::Unverified>,
            ) -> #ir::IrResult<::std::vec::Vec<#ir::Type<'ctx, B>>>
            where
                B: #ir::ModuleBrand + 'ctx,
            {
                Ok(::std::vec![#(<#field_tys as #ir::IrField>::ir_type(module)?,)*])
            }

            fn matches_fields<'ctx, B>(fields: &[#ir::Type<'ctx, B>]) -> bool
            where
                B: #ir::ModuleBrand + 'ctx,
            {
                fields.len() == #field_count #(&& #matches_terms)*
            }
        }

        impl<'ctx, B> #ir::StructSchemaValue<'ctx, #ident, B> for #value_ident<'ctx, B>
        where
            B: #ir::ModuleBrand + 'ctx,
        {
            #[inline]
            fn as_struct_value(self) -> #ir::StructValue<'ctx, B> {
                self.raw
            }

            #[inline]
            fn from_struct_value(
                raw: #ir::StructValue<'ctx, B>,
                _validated: &#ir::ValidatedStructValue<'_>,
            ) -> Self {
                Self { raw }
            }
        }

        impl<'ctx, B> ::core::convert::TryFrom<#ir::Argument<'ctx, B>> for #value_ident<'ctx, B>
        where
            B: #ir::ModuleBrand + 'ctx,
        {
            type Error = #ir::IrError;

            #[inline]
            fn try_from(value: #ir::Argument<'ctx, B>) -> #ir::IrResult<Self> {
                <#ident as #ir::StructSchema>::try_value_from_ir(value)
            }
        }

        impl<'ctx, B> ::core::convert::TryFrom<#ir::StructValue<'ctx, B>> for #value_ident<'ctx, B>
        where
            B: #ir::ModuleBrand + 'ctx,
        {
            type Error = #ir::IrError;

            #[inline]
            fn try_from(value: #ir::StructValue<'ctx, B>) -> #ir::IrResult<Self> {
                <#ident as #ir::StructSchema>::try_value_from_ir(value)
            }
        }

        impl<'ctx, B> ::core::convert::TryFrom<#ir::Value<'ctx, B>> for #value_ident<'ctx, B>
        where
            B: #ir::ModuleBrand + 'ctx,
        {
            type Error = #ir::IrError;

            #[inline]
            fn try_from(value: #ir::Value<'ctx, B>) -> #ir::IrResult<Self> {
                <#ident as #ir::StructSchema>::try_value_from_ir(value)
            }
        }

        impl<'ctx, B> ::core::convert::TryFrom<#ir::Constant<'ctx, B>> for #value_ident<'ctx, B>
        where
            B: #ir::ModuleBrand + 'ctx,
        {
            type Error = #ir::IrError;

            #[inline]
            fn try_from(value: #ir::Constant<'ctx, B>) -> #ir::IrResult<Self> {
                <#ident as #ir::StructSchema>::try_value_from_ir(value)
            }
        }

        impl<'ctx, B> ::core::convert::TryFrom<#ir::Instruction<'ctx, #ir::instruction::state::Attached, B>>
            for #value_ident<'ctx, B>
        where
            B: #ir::ModuleBrand + 'ctx,
        {
            type Error = #ir::IrError;

            #[inline]
            fn try_from(
                value: #ir::Instruction<'ctx, #ir::instruction::state::Attached, B>,
            ) -> #ir::IrResult<Self> {
                <#ident as #ir::StructSchema>::try_value_from_ir(value)
            }
        }

        impl<'ctx, B> #ir::IntoIrField<'ctx, #ident, B> for #value_ident<'ctx, B>
        where
            B: #ir::ModuleBrand + 'ctx,
        {
            #[inline]
            fn into_ir_field(
                self,
                _module: #ir::ModuleRef<'ctx, B>,
            ) -> #ir::IrResult<#ir::Value<'ctx, B>> {
                Ok(self.raw.into_erased())
            }
        }

        impl<'ctx, B: #ir::ModuleBrand + 'ctx> #ir::IntoCallArg<'ctx, #ident, B>
            for #value_ident<'ctx, B>
        {
            #[inline]
            fn into_call_arg(
                self,
                _module: #ir::ModuleRef<'ctx, B>,
            ) -> #ir::IrResult<#ir::Value<'ctx, B>> {
                Ok(self.as_struct_value().into_erased())
            }
        }

        impl<'ctx, B> #ir::ir_builder::IntoReturnValue<'ctx, #ir::Dyn, B> for #value_ident<'ctx, B>
        where
            B: #ir::ModuleBrand + 'ctx,
        {
            #[inline]
            fn into_return_value(
                self,
                _module: #ir::ModuleRef<'ctx, B>,
            ) -> #ir::IrResult<#ir::Value<'ctx, B>> {
                Ok(self.raw.into_erased())
            }
        }
    })
}

fn build_value_steps(
    schema_ident: &Ident,
    field_idents: &[Ident],
    field_tys: &[syn::Type],
    indices: &[u32],
    cx: BuildStepContext<'_>,
) -> Vec<TokenStream2> {
    let ir = cx.ir;
    let module_param = cx.module_param;
    let builder_param = cx.builder_param;
    let base_name_ident = cx.base_name_ident;
    let mut steps = Vec::new();
    for (position, ((field_ident, ty), idx)) in field_idents
        .iter()
        .zip(field_tys.iter())
        .zip(indices.iter())
        .enumerate()
    {
        let value_ident = Ident::new(&format!("__llvmkit_value_{position}"), Span::mixed_site());
        let aggregate = if position == 0 {
            quote! {
                <#schema_ident as #ir::StructSchema>::ir_type(#module_param)?
                    .as_type()
                    .get_poison()
                    .as_constant()
            }
        } else {
            let previous = Ident::new(
                &format!("__llvmkit_value_{}", position - 1),
                Span::mixed_site(),
            );
            quote! { #previous }
        };
        let field_name = field_ident.to_string();
        let name_ident = Ident::new(
            &format!("__llvmkit_field_name_{position}"),
            Span::mixed_site(),
        );
        steps.push(quote! {
            let #name_ident = if #base_name_ident.is_empty() {
                ::std::string::String::from(#field_name)
            } else {
                ::std::format!("{}.{}", #base_name_ident, #field_name)
            };
            let #value_ident = #builder_param.build_insert_field::<#schema_ident, #ty, _, _, _>(
                #aggregate,
                #field_ident,
                #idx,
                #name_ident,
            )?;
        });
    }
    steps
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
