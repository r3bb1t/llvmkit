//! Minimal LLVM intrinsic model used by the parser and verifier.
//!
//! This module intentionally covers only the Phase 5 core intrinsic subset.
//! Unsupported `llvm.*` names remain errors instead of ordinary functions.

use crate::derived_types::FunctionType;
use crate::error::{IrError, IrResult};
use crate::module::Module;
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
    pub return_ty: IntrinsicTypePattern,
    pub params: Box<[IntrinsicTypePattern]>,
    pub is_var_arg: bool,
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

    pub fn function_type<'ctx>(
        self,
        module: &'ctx Module<'ctx>,
        name: &str,
    ) -> IrResult<FunctionType<'ctx>> {
        match self {
            Self::LifetimeStart | Self::LifetimeEnd => {
                let addr_space = lifetime_addr_space(name).ok_or(IrError::InvalidOperation {
                    message: "intrinsic signature mismatch",
                })?;
                Ok(module.fn_type(
                    module.void_type(),
                    [
                        module.i64_type().as_type(),
                        module.ptr_type(addr_space).as_type(),
                    ],
                    false,
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
                Ok(module.fn_type(
                    module.void_type(),
                    [
                        module.ptr_type(dst_as).as_type(),
                        module.ptr_type(src_as).as_type(),
                        module.i64_type().as_type(),
                        module.i1_type().as_type(),
                    ],
                    false,
                ))
            }
            Self::Memset => {
                let dst_as = parse_pointer_suffix(name, "llvm.memset.p").ok_or(
                    IrError::InvalidOperation {
                        message: "intrinsic signature mismatch",
                    },
                )?;
                Ok(module.fn_type(
                    module.void_type(),
                    [
                        module.ptr_type(dst_as).as_type(),
                        module.i8_type().as_type(),
                        module.i64_type().as_type(),
                        module.i1_type().as_type(),
                    ],
                    false,
                ))
            }
            Self::Trap | Self::Donothing => {
                Ok(module.fn_type(module.void_type(), std::iter::empty::<Type<'ctx>>(), false))
            }
            Self::ReadRegisterI64 => {
                Ok(module.fn_type(module.i64_type(), [module.metadata_type().as_type()], false))
            }
            Self::WriteRegisterI64 => Ok(module.fn_type(
                module.void_type(),
                [
                    module.metadata_type().as_type(),
                    module.i64_type().as_type(),
                ],
                false,
            )),
            Self::Expect => Err(IrError::InvalidOperation {
                message: "intrinsic signature mismatch",
            }),
        }
    }
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
