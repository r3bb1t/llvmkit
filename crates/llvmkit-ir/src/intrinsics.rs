//! Minimal LLVM intrinsic model used by the parser and verifier.
//!
//! This module intentionally covers only the Phase 5 core intrinsic subset.
//! Unsupported `llvm.*` names remain errors instead of ordinary functions.

use crate::derived_types::FunctionType;
use crate::error::{IrError, IrResult};
use crate::module::{Module, ModuleBrand, ModuleRef, Unverified};
use crate::r#type::Type;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntrinsicId {
    LifetimeStart,
    LifetimeEnd,
    Memcpy,
    Memmove,
    Memset,
    Expect,
    Trap,
    Donothing,
    ReadRegisterI64,
    WriteRegisterI64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntrinsicFloatKind {
    Half,
    BFloat,
    Float,
    Double,
    Fp128,
    X86Fp80,
    PpcFp128,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IntrinsicTypePattern {
    Void,
    Token,
    Metadata,
    Int(u32),
    Float(IntrinsicFloatKind),
    Pointer {
        address_space: u32,
    },
    SameAsParam(u32),
    Overloaded(u32),
    Vector {
        element: Box<IntrinsicTypePattern>,
        min_lanes: u32,
        scalable: bool,
    },
    Struct(Box<[IntrinsicTypePattern]>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IntrinsicSignature {
    return_ty: IntrinsicTypePattern,
    params: Box<[IntrinsicTypePattern]>,
    is_var_arg: bool,
}

impl IntrinsicSignature {
    pub fn new<Params>(return_ty: IntrinsicTypePattern, params: Params, is_var_arg: bool) -> Self
    where
        Params: Into<Box<[IntrinsicTypePattern]>>,
    {
        Self {
            return_ty,
            params: params.into(),
            is_var_arg,
        }
    }

    pub fn return_type(&self) -> &IntrinsicTypePattern {
        &self.return_ty
    }

    pub fn params(&self) -> &[IntrinsicTypePattern] {
        &self.params
    }

    pub const fn is_var_arg(&self) -> bool {
        self.is_var_arg
    }
}

impl IntrinsicId {
    pub fn lookup(name: &str) -> Option<Self> {
        match name {
            "llvm.trap" => Some(Self::Trap),
            "llvm.donothing" => Some(Self::Donothing),
            "llvm.read_register.i64" => Some(Self::ReadRegisterI64),
            "llvm.write_register.i64" => Some(Self::WriteRegisterI64),
            _ => {
                if parse_pointer_suffix(name, "llvm.lifetime.start.p").is_some() {
                    Some(Self::LifetimeStart)
                } else if parse_pointer_suffix(name, "llvm.lifetime.end.p").is_some() {
                    Some(Self::LifetimeEnd)
                } else if parse_two_pointer_suffixes(name, "llvm.memcpy.p").is_some() {
                    Some(Self::Memcpy)
                } else if parse_two_pointer_suffixes(name, "llvm.memmove.p").is_some() {
                    Some(Self::Memmove)
                } else if parse_pointer_suffix(name, "llvm.memset.p").is_some() {
                    Some(Self::Memset)
                } else if name == "llvm.expect" || name.starts_with("llvm.expect.i") {
                    Some(Self::Expect)
                } else {
                    None
                }
            }
        }
    }

    pub fn function_type<'ctx, B>(
        self,
        module: &Module<'ctx, B, Unverified>,
        name: &str,
    ) -> IrResult<FunctionType<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        self.function_type_ref(module.module_ref(), name)
    }

    pub(crate) fn function_type_ref<'ctx, B>(
        self,
        module: ModuleRef<'ctx, B>,
        name: &str,
    ) -> IrResult<FunctionType<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        match self {
            Self::LifetimeStart | Self::LifetimeEnd => {
                let addr_space = lifetime_addr_space(name).ok_or(IrError::InvalidOperation {
                    message: "intrinsic signature mismatch",
                })?;
                Ok(fn_type(
                    module,
                    void_type(module),
                    [int_type(module, 64), ptr_type(module, addr_space)],
                ))
            }
            Self::Memcpy | Self::Memmove => {
                let (dst_as, src_as) = parse_two_pointer_suffixes(
                    name,
                    if self == Self::Memcpy {
                        "llvm.memcpy.p"
                    } else {
                        "llvm.memmove.p"
                    },
                )
                .ok_or(IrError::InvalidOperation {
                    message: "intrinsic signature mismatch",
                })?;
                Ok(fn_type(
                    module,
                    void_type(module),
                    [
                        ptr_type(module, dst_as),
                        ptr_type(module, src_as),
                        int_type(module, 64),
                        int_type(module, 1),
                    ],
                ))
            }
            Self::Memset => {
                let dst_as = parse_pointer_suffix(name, "llvm.memset.p").ok_or(
                    IrError::InvalidOperation {
                        message: "intrinsic signature mismatch",
                    },
                )?;
                Ok(fn_type(
                    module,
                    void_type(module),
                    [
                        ptr_type(module, dst_as),
                        int_type(module, 8),
                        int_type(module, 64),
                        int_type(module, 1),
                    ],
                ))
            }
            Self::Trap | Self::Donothing => Ok(fn_type(
                module,
                void_type(module),
                Vec::<Type<'ctx, B>>::new(),
            )),
            Self::ReadRegisterI64 => Ok(fn_type(
                module,
                int_type(module, 64),
                [metadata_type(module)],
            )),
            Self::WriteRegisterI64 => Ok(fn_type(
                module,
                void_type(module),
                [metadata_type(module), int_type(module, 64)],
            )),
            Self::Expect => Err(IrError::InvalidOperation {
                message: "intrinsic signature mismatch",
            }),
        }
    }
}

fn void_type<'ctx, B>(module: ModuleRef<'ctx, B>) -> Type<'ctx, B>
where
    B: ModuleBrand + 'ctx,
{
    Type::new(module.module().context().void(), module)
}

fn metadata_type<'ctx, B>(module: ModuleRef<'ctx, B>) -> Type<'ctx, B>
where
    B: ModuleBrand + 'ctx,
{
    Type::new(module.module().context().metadata(), module)
}

fn int_type<'ctx, B>(module: ModuleRef<'ctx, B>, bits: u32) -> Type<'ctx, B>
where
    B: ModuleBrand + 'ctx,
{
    Type::new(module.module().context().int_type(bits), module)
}

fn ptr_type<'ctx, B>(module: ModuleRef<'ctx, B>, addr_space: u32) -> Type<'ctx, B>
where
    B: ModuleBrand + 'ctx,
{
    Type::new(module.module().context().ptr_type(addr_space), module)
}

fn fn_type<'ctx, B, I>(
    module: ModuleRef<'ctx, B>,
    ret: Type<'ctx, B>,
    params: I,
) -> FunctionType<'ctx, B>
where
    B: ModuleBrand + 'ctx,
    I: IntoIterator<Item = Type<'ctx, B>>,
{
    let param_ids: Vec<_> = params.into_iter().map(Type::id).collect();
    let id = module
        .module()
        .context()
        .function_type(ret.id(), param_ids.into_boxed_slice(), false);
    FunctionType::new(id, module)
}

fn lifetime_addr_space(name: &str) -> Option<u32> {
    parse_pointer_suffix(name, "llvm.lifetime.start.p")
        .or_else(|| parse_pointer_suffix(name, "llvm.lifetime.end.p"))
}

fn parse_pointer_suffix(name: &str, prefix: &str) -> Option<u32> {
    let rest = name.strip_prefix(prefix)?;
    parse_addr_space(rest)
}

fn parse_two_pointer_suffixes(name: &str, prefix: &str) -> Option<(u32, u32)> {
    let rest = name.strip_prefix(prefix)?;
    let (dst, src) = rest.split_once(".p")?;
    Some((parse_addr_space(dst)?, parse_addr_space(src)?))
}

fn parse_addr_space(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}
