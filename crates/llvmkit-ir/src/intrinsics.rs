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
    ReadCycleCounter,
    ReadRegisterI64,
    WriteRegisterI64,
    Assume,
    Abs,
    BSwap,
    BitReverse,
    CTLZ,
    CTTZ,
    CTPOP,
    FShl,
    FShr,
    UMax,
    UMin,
    SMax,
    SMin,
    UAddSat,
    USubSat,
    SAddSat,
    SSubSat,
    VectorReduceAdd,
    PtrMask,
    VScale,
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
            "llvm.readcyclecounter" => Some(Self::ReadCycleCounter),
            "llvm.read_register.i64" => Some(Self::ReadRegisterI64),
            "llvm.write_register.i64" => Some(Self::WriteRegisterI64),
            "llvm.assume" => Some(Self::Assume),
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
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.abs.").is_some() {
                    Some(Self::Abs)
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.bswap.").is_some() {
                    Some(Self::BSwap)
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.bitreverse.").is_some() {
                    Some(Self::BitReverse)
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.ctlz.").is_some() {
                    Some(Self::CTLZ)
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.cttz.").is_some() {
                    Some(Self::CTTZ)
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.ctpop.").is_some() {
                    Some(Self::CTPOP)
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.fshl.").is_some() {
                    Some(Self::FShl)
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.fshr.").is_some() {
                    Some(Self::FShr)
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.umax.").is_some() {
                    Some(Self::UMax)
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.umin.").is_some() {
                    Some(Self::UMin)
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.smax.").is_some() {
                    Some(Self::SMax)
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.smin.").is_some() {
                    Some(Self::SMin)
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.uadd.sat.").is_some() {
                    Some(Self::UAddSat)
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.usub.sat.").is_some() {
                    Some(Self::USubSat)
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.sadd.sat.").is_some() {
                    Some(Self::SAddSat)
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.ssub.sat.").is_some() {
                    Some(Self::SSubSat)
                } else if parse_int_or_fixed_vector_suffix(name, "llvm.vector.reduce.add.")
                    .is_some()
                {
                    Some(Self::VectorReduceAdd)
                } else if parse_ptrmask_suffix(name).is_some() {
                    Some(Self::PtrMask)
                } else if parse_int_suffix(name, "llvm.vscale.").is_some() {
                    Some(Self::VScale)
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
                let addr_space = lifetime_addr_space(name).ok_or_else(intrinsic_mismatch)?;
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
                .ok_or_else(intrinsic_mismatch)?;
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
                let dst_as =
                    parse_pointer_suffix(name, "llvm.memset.p").ok_or_else(intrinsic_mismatch)?;
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
            Self::ReadCycleCounter => Ok(fn_type(
                module,
                int_type(module, 64),
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
            Self::Assume => Ok(fn_type(module, void_type(module), [int_type(module, 1)])),
            Self::Abs | Self::CTLZ | Self::CTTZ => {
                let prefix = match self {
                    Self::Abs => "llvm.abs.",
                    Self::CTLZ => "llvm.ctlz.",
                    Self::CTTZ => "llvm.cttz.",
                    _ => unreachable!("integer unary-with-i1 intrinsic prefix is exhaustive"),
                };
                let ty = overloaded_int_ty(module, name, prefix)?;
                Ok(fn_type(module, ty, [ty, int_type(module, 1)]))
            }
            Self::BSwap | Self::BitReverse | Self::CTPOP => {
                let prefix = match self {
                    Self::BSwap => "llvm.bswap.",
                    Self::BitReverse => "llvm.bitreverse.",
                    Self::CTPOP => "llvm.ctpop.",
                    _ => unreachable!("integer unary intrinsic prefix is exhaustive"),
                };
                let ty = overloaded_int_ty(module, name, prefix)?;
                Ok(fn_type(module, ty, [ty]))
            }
            Self::FShl | Self::FShr => {
                let prefix = if self == Self::FShl {
                    "llvm.fshl."
                } else {
                    "llvm.fshr."
                };
                let ty = overloaded_int_ty(module, name, prefix)?;
                Ok(fn_type(module, ty, [ty, ty, ty]))
            }
            Self::UMax
            | Self::UMin
            | Self::SMax
            | Self::SMin
            | Self::UAddSat
            | Self::USubSat
            | Self::SAddSat
            | Self::SSubSat => {
                let prefix = match self {
                    Self::UMax => "llvm.umax.",
                    Self::UMin => "llvm.umin.",
                    Self::SMax => "llvm.smax.",
                    Self::SMin => "llvm.smin.",
                    Self::UAddSat => "llvm.uadd.sat.",
                    Self::USubSat => "llvm.usub.sat.",
                    Self::SAddSat => "llvm.sadd.sat.",
                    Self::SSubSat => "llvm.ssub.sat.",
                    _ => unreachable!("integer binary intrinsic prefix is exhaustive"),
                };
                let ty = overloaded_int_ty(module, name, prefix)?;
                Ok(fn_type(module, ty, [ty, ty]))
            }
            Self::VectorReduceAdd => {
                let overload = parse_int_or_fixed_vector_suffix(name, "llvm.vector.reduce.add.")
                    .filter(|overload| overload.is_fixed_vector())
                    .ok_or_else(intrinsic_mismatch)?;
                let scalar_ty = int_type(module, overload.bits);
                let vector_ty = int_overload_type(module, overload);
                Ok(fn_type(module, scalar_ty, [vector_ty]))
            }
            Self::PtrMask => {
                let overload = parse_ptrmask_suffix(name).ok_or_else(intrinsic_mismatch)?;
                let ptr_ty = ptrmask_pointer_type(module, overload);
                let mask_ty = ptrmask_mask_type(module, overload);
                Ok(fn_type(module, ptr_ty, [ptr_ty, mask_ty]))
            }
            Self::VScale => {
                let bits = parse_int_suffix(name, "llvm.vscale.").ok_or_else(intrinsic_mismatch)?;
                Ok(fn_type(
                    module,
                    int_type(module, bits),
                    Vec::<Type<'ctx, B>>::new(),
                ))
            }
            Self::Expect => Err(intrinsic_mismatch()),
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

fn intrinsic_mismatch() -> IrError {
    IrError::InvalidOperation {
        message: "intrinsic signature mismatch",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct IntOverload {
    bits: u32,
    lanes: Option<u32>,
}

impl IntOverload {
    #[inline]
    fn is_fixed_vector(&self) -> bool {
        self.lanes.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PtrMaskOverload {
    Scalar {
        addr_space: u32,
        mask_bits: u32,
    },
    FixedVector {
        lanes: u32,
        addr_space: u32,
        mask_bits: u32,
    },
}

fn overloaded_int_ty<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    name: &str,
    prefix: &str,
) -> IrResult<Type<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
{
    let overload = parse_int_or_fixed_vector_suffix(name, prefix).ok_or_else(intrinsic_mismatch)?;
    Ok(int_overload_type(module, overload))
}

fn int_overload_type<'ctx, B>(module: ModuleRef<'ctx, B>, overload: IntOverload) -> Type<'ctx, B>
where
    B: ModuleBrand + 'ctx,
{
    let scalar = int_type(module, overload.bits);
    if let Some(lanes) = overload.lanes {
        fixed_vector_type(module, scalar, lanes)
    } else {
        scalar
    }
}

fn fixed_vector_type<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    elem: Type<'ctx, B>,
    lanes: u32,
) -> Type<'ctx, B>
where
    B: ModuleBrand + 'ctx,
{
    let id = module
        .module()
        .context()
        .fixed_vector_type(elem.id(), lanes);
    Type::new(id, module)
}

fn ptrmask_pointer_type<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    overload: PtrMaskOverload,
) -> Type<'ctx, B>
where
    B: ModuleBrand + 'ctx,
{
    match overload {
        PtrMaskOverload::Scalar { addr_space, .. } => ptr_type(module, addr_space),
        PtrMaskOverload::FixedVector {
            lanes, addr_space, ..
        } => fixed_vector_type(module, ptr_type(module, addr_space), lanes),
    }
}

fn ptrmask_mask_type<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    overload: PtrMaskOverload,
) -> Type<'ctx, B>
where
    B: ModuleBrand + 'ctx,
{
    match overload {
        PtrMaskOverload::Scalar { mask_bits, .. } => int_type(module, mask_bits),
        PtrMaskOverload::FixedVector {
            lanes, mask_bits, ..
        } => fixed_vector_type(module, int_type(module, mask_bits), lanes),
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

fn parse_int_suffix(name: &str, prefix: &str) -> Option<u32> {
    let rest = name.strip_prefix(prefix)?;
    parse_int_code(rest)
}

fn parse_int_or_fixed_vector_suffix(name: &str, prefix: &str) -> Option<IntOverload> {
    let rest = name.strip_prefix(prefix)?;
    parse_int_or_fixed_vector_code(rest)
}

fn parse_int_or_fixed_vector_code(s: &str) -> Option<IntOverload> {
    if let Some(int) = parse_int_code(s) {
        return Some(IntOverload {
            bits: int,
            lanes: None,
        });
    }
    let rest = s.strip_prefix('v')?;
    let (lanes, bits) = rest.split_once('i')?;
    Some(IntOverload {
        bits: parse_positive_u32(bits)?,
        lanes: Some(parse_positive_u32(lanes)?),
    })
}

fn parse_int_code(s: &str) -> Option<u32> {
    let bits = s.strip_prefix('i')?;
    parse_positive_u32(bits)
}

fn parse_ptrmask_suffix(name: &str) -> Option<PtrMaskOverload> {
    let rest = name.strip_prefix("llvm.ptrmask.")?;
    let (ptr, mask) = rest.split_once('.')?;
    if let Some(addr_space) = parse_pointer_code(ptr) {
        return Some(PtrMaskOverload::Scalar {
            addr_space,
            mask_bits: parse_int_code(mask)?,
        });
    }
    let (lanes, addr_space) = parse_fixed_vector_pointer_code(ptr)?;
    let mask = parse_int_or_fixed_vector_code(mask)?;
    if mask.lanes != Some(lanes) {
        return None;
    }
    Some(PtrMaskOverload::FixedVector {
        lanes,
        addr_space,
        mask_bits: mask.bits,
    })
}

fn parse_pointer_code(s: &str) -> Option<u32> {
    let addr_space = s.strip_prefix('p')?;
    parse_addr_space(addr_space)
}

fn parse_fixed_vector_pointer_code(s: &str) -> Option<(u32, u32)> {
    let rest = s.strip_prefix('v')?;
    let (lanes, addr_space) = rest.split_once('p')?;
    Some((parse_positive_u32(lanes)?, parse_addr_space(addr_space)?))
}

fn parse_positive_u32(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value = s.parse().ok()?;
    if value == 0 { None } else { Some(value) }
}
