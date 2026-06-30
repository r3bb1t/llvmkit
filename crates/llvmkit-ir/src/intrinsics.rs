//! Generated-backed LLVM intrinsic lookup used by current parser/verifier code.
//!
//! Full IIT signature decoding is intentionally deferred. This module exposes
//! stable generated IDs/descriptors now while preserving the old semantic subset
//! needed by existing analysis and verifier paths.
use core::num::NonZeroU32;

use crate::attributes::{AttrIndex, AttrKind, Attribute, AttributeStorage, MemoryEffects};
use crate::derived_types::FunctionType;
use crate::error::{IrError, IrResult};
use crate::module::{Brand, Module, ModuleBrand, ModuleRef};
use crate::r#type::{Type, TypeData, TypeId};
use crate::value::{Value, ValueKindData};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IntrinsicId(NonZeroU32);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IntrinsicDescriptor<'ctx, B: ModuleBrand = Brand<'ctx>> {
    id: IntrinsicId,
    overloads: Box<[Type<'ctx, B>]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct IntrinsicFunctionData {
    pub id: IntrinsicId,
    pub overloads: Box<[TypeId]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntrinsicNameResolution {
    NonIntrinsic,
    UnknownIntrinsic,
    Known(IntrinsicId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinaryIntrinsic {
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum IntrinsicSemantic {
    LifetimeStart,
    LifetimeEnd,
    Memcpy,
    Memmove,
    Memset,
    Expect,
    Trap,
    Donothing,
    ReadCycleCounter,
    ReadRegister,
    WriteRegister,
    Assume,
    Abs,
    BSwap,
    BitReverse,
    Ctlz,
    Cttz,
    Ctpop,
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
pub(crate) struct IntrinsicRecord {
    pub enum_name: &'static str,
    pub base_name: &'static str,
    pub target_prefix: &'static str,
    pub is_overloaded: bool,
    pub iit_table_index: u32,
    pub fn_attrs: IntrinsicFnAttrs,
    pub arg_attrs: &'static [IntrinsicIndexedAttr],
    pub memory_effects: MemoryEffects,
    pub clang_builtin: Option<&'static str>,
    pub ms_builtin: Option<&'static str>,
    pub pretty_print: &'static [PrettyPrintArg],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct IntrinsicTargetSet {
    pub prefix: &'static str,
    pub offset: u32,
    pub count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct IntrinsicFnAttrs {
    pub no_unwind: bool,
    pub no_return: bool,
    pub no_callback: bool,
    pub no_sync: bool,
    pub no_free: bool,
    pub will_return: bool,
    pub cold: bool,
    pub no_duplicate: bool,
    pub no_merge: bool,
    pub commutative: bool,
    pub convergent: bool,
    pub speculatable: bool,
    pub strict_fp: bool,
    pub no_create_undef_or_poison: bool,
    pub has_side_effects: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct IntrinsicIndexedAttr {
    pub index: u32,
    pub attr: IntrinsicArgAttr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum IntrinsicArgAttr {
    NoCapture,
    NoAlias,
    NoUndef,
    NonNull,
    Returned,
    ReadOnly,
    WriteOnly,
    ReadNone,
    ImmArg,
    Alignment(u64),
    Dereferenceable(u64),
    Range { lower: i64, upper: i64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PrettyPrintArg {
    pub arg_index: u32,
    pub name: &'static str,
    pub printer: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct IntrinsicSampleOverload {
    pub raw_id: u32,
    pub overloads: &'static [IntrinsicSampleType],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum IntrinsicSampleType {
    Int(u32),
    Float(&'static str),
    Pointer(u32),
    FixedVector {
        lanes: u32,
        element: &'static IntrinsicSampleType,
    },
}

static INVALID_INTRINSIC_RECORD: IntrinsicRecord = IntrinsicRecord {
    enum_name: "invalid",
    base_name: "llvm.invalid",
    target_prefix: "",
    is_overloaded: false,
    iit_table_index: 0,
    fn_attrs: IntrinsicFnAttrs {
        no_unwind: false,
        no_return: false,
        no_callback: false,
        no_sync: false,
        no_free: false,
        will_return: false,
        cold: false,
        no_duplicate: false,
        no_merge: false,
        commutative: false,
        convergent: false,
        speculatable: false,
        strict_fp: false,
        no_create_undef_or_poison: false,
        has_side_effects: false,
    },
    arg_attrs: &[],
    memory_effects: MemoryEffects::create_from_int_value(0),
    clang_builtin: None,
    ms_builtin: None,
    pretty_print: &[],
};

mod generated {
    use super::{
        IntrinsicArgAttr, IntrinsicFnAttrs, IntrinsicIndexedAttr, IntrinsicRecord,
        IntrinsicSampleOverload, IntrinsicSampleType, IntrinsicTargetSet, MemoryEffects,
        PrettyPrintArg,
    };

    include!(concat!(env!("OUT_DIR"), "/intrinsics_generated.rs"));
}

// Reachability anchors for generated data that is typechecked in this step and
// consumed by the full IIT decoder in the next step. Keeping these references
// avoids dead-code warnings without relaxing lints or editing generated.rs.
const _: IntrinsicArgAttr = IntrinsicArgAttr::Dereferenceable(0);
const _: IntrinsicSampleType = IntrinsicSampleType::Int(0);
const _: IntrinsicSampleType = IntrinsicSampleType::Float("f32");
const _: IntrinsicSampleType = IntrinsicSampleType::Pointer(0);
const _: IntrinsicSampleType = IntrinsicSampleType::FixedVector {
    lanes: 1,
    element: &IntrinsicSampleType::Int(0),
};
const _: IntrinsicSampleOverload = IntrinsicSampleOverload {
    raw_id: 0,
    overloads: &[],
};
const _: u8 = generated::IIT_WASM_EXNREF;

fn generated_table_anchor() -> usize {
    generated::IIT_TABLE
        .len()
        .saturating_add(generated::IIT_LONG_ENCODING_TABLE.len())
        .saturating_add(generated::SAMPLE_OVERLOADS.len())
        .saturating_add(usize::from(generated::IIT_WASM_EXNREF))
}

pub fn resolve_intrinsic_name(name: &str) -> IntrinsicNameResolution {
    IntrinsicId::resolve_name(name)
}

pub fn descriptor_for_callee<'ctx, B>(
    callee: Value<'ctx, B>,
) -> Option<IntrinsicDescriptor<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
{
    let ValueKindData::Function(function) = &callee.data().kind else {
        return None;
    };
    let module = ModuleRef::<B>::new(callee.module().core_ref());
    if let Some(data) = &function.intrinsic {
        let overloads = data
            .overloads
            .iter()
            .map(|id| Type::new(*id, module))
            .collect::<Box<[_]>>();
        return IntrinsicDescriptor::new(data.id, overloads).ok();
    }
    let id = IntrinsicId::lookup(&function.name)?;
    let descriptor = descriptor_for_name(module, id, &function.name).ok()?;
    let expected = descriptor.function_type_ref(module).ok()?;
    (expected.as_type().id() == function.signature).then_some(descriptor)
}

pub(crate) fn semantic_for_callee<'ctx, B>(callee: Value<'ctx, B>) -> Option<IntrinsicSemantic>
where
    B: ModuleBrand + 'ctx,
{
    let descriptor = descriptor_for_callee(callee)?;
    descriptor.id.semantic_kind()
}

const fn checked_intrinsic_id(raw: u32) -> IntrinsicId {
    let Some(raw) = NonZeroU32::new(raw) else {
        panic!("generated intrinsic id constants are nonzero");
    };
    IntrinsicId(raw)
}

impl IntrinsicId {
    pub const LIFETIME_START: Self = checked_intrinsic_id(generated::SEMANTIC_LIFETIME_START);
    pub const LIFETIME_END: Self = checked_intrinsic_id(generated::SEMANTIC_LIFETIME_END);
    pub const MEMCPY: Self = checked_intrinsic_id(generated::SEMANTIC_MEMCPY);
    pub const MEMMOVE: Self = checked_intrinsic_id(generated::SEMANTIC_MEMMOVE);
    pub const MEMSET: Self = checked_intrinsic_id(generated::SEMANTIC_MEMSET);
    pub const EXPECT: Self = checked_intrinsic_id(generated::SEMANTIC_EXPECT);
    pub const TRAP: Self = checked_intrinsic_id(generated::SEMANTIC_TRAP);
    pub const DONOTHING: Self = checked_intrinsic_id(generated::SEMANTIC_DONOTHING);
    pub const READCYCLECOUNTER: Self = checked_intrinsic_id(generated::SEMANTIC_READCYCLECOUNTER);
    pub const READ_REGISTER: Self = checked_intrinsic_id(generated::SEMANTIC_READ_REGISTER);
    pub const WRITE_REGISTER: Self = checked_intrinsic_id(generated::SEMANTIC_WRITE_REGISTER);
    pub const ASSUME: Self = checked_intrinsic_id(generated::SEMANTIC_ASSUME);
    pub const ABS: Self = checked_intrinsic_id(generated::SEMANTIC_ABS);
    pub const BSWAP: Self = checked_intrinsic_id(generated::SEMANTIC_BSWAP);
    pub const BITREVERSE: Self = checked_intrinsic_id(generated::SEMANTIC_BITREVERSE);
    pub const CTLZ: Self = checked_intrinsic_id(generated::SEMANTIC_CTLZ);
    pub const CTTZ: Self = checked_intrinsic_id(generated::SEMANTIC_CTTZ);
    pub const CTPOP: Self = checked_intrinsic_id(generated::SEMANTIC_CTPOP);
    pub const FSHL: Self = checked_intrinsic_id(generated::SEMANTIC_FSHL);
    pub const FSHR: Self = checked_intrinsic_id(generated::SEMANTIC_FSHR);
    pub const UADD_SAT: Self = checked_intrinsic_id(generated::SEMANTIC_UADD_SAT);
    pub const USUB_SAT: Self = checked_intrinsic_id(generated::SEMANTIC_USUB_SAT);
    pub const SADD_SAT: Self = checked_intrinsic_id(generated::SEMANTIC_SADD_SAT);
    pub const SSUB_SAT: Self = checked_intrinsic_id(generated::SEMANTIC_SSUB_SAT);
    pub const UMIN: Self = checked_intrinsic_id(generated::SEMANTIC_UMIN);
    pub const UMAX: Self = checked_intrinsic_id(generated::SEMANTIC_UMAX);
    pub const SMIN: Self = checked_intrinsic_id(generated::SEMANTIC_SMIN);
    pub const SMAX: Self = checked_intrinsic_id(generated::SEMANTIC_SMAX);
    pub const VECTOR_REDUCE_ADD: Self = checked_intrinsic_id(generated::SEMANTIC_VECTOR_REDUCE_ADD);
    pub const PTRMASK: Self = checked_intrinsic_id(generated::SEMANTIC_PTRMASK);
    pub const VSCALE: Self = checked_intrinsic_id(generated::SEMANTIC_VSCALE);

    pub fn lookup(name: &str) -> Option<Self> {
        let _ = generated_table_anchor();
        let target_set = target_set_for_name(name)?;
        lookup_in_target_set(name, target_set)
    }

    pub fn resolve_name(name: &str) -> IntrinsicNameResolution {
        if !name.starts_with("llvm.") {
            return IntrinsicNameResolution::NonIntrinsic;
        }
        match Self::lookup(name) {
            Some(id) => IntrinsicNameResolution::Known(id),
            None => IntrinsicNameResolution::UnknownIntrinsic,
        }
    }
    pub fn all() -> impl ExactSizeIterator<Item = Self> {
        (1..generated::NUM_INTRINSICS).map(|raw| {
            let Some(id) = Self::from_raw(raw) else {
                unreachable!("generated intrinsic id iterator stays within generated bounds");
            };
            id
        })
    }

    pub const fn raw(self) -> u32 {
        self.0.get()
    }

    pub fn enum_name(self) -> &'static str {
        self.record().enum_name
    }

    pub fn base_name(self) -> &'static str {
        self.record().base_name
    }

    pub fn target_prefix(self) -> Option<&'static str> {
        let prefix = self.record().target_prefix;
        if prefix.is_empty() {
            None
        } else {
            Some(prefix)
        }
    }

    pub fn is_target(self) -> bool {
        self.target_prefix().is_some()
    }

    pub fn is_overloaded(self) -> bool {
        self.record().is_overloaded
    }

    pub fn has_pretty_printed_args(self) -> bool {
        !self.record().pretty_print.is_empty()
    }

    pub fn is_commutative(self) -> bool {
        self.record().fn_attrs.commutative
    }

    pub fn may_throw(self) -> bool {
        !self.record().fn_attrs.no_unwind
    }

    pub fn memory_effects(self) -> MemoryEffects {
        self.record().memory_effects
    }

    pub fn as_binary_intrinsic(self) -> Option<BinaryIntrinsic> {
        BinaryIntrinsic::from_intrinsic_id(self)
    }

    pub fn from_raw(raw: u32) -> Option<Self> {
        if raw >= generated::NUM_INTRINSICS {
            return None;
        }
        NonZeroU32::new(raw).map(Self)
    }

    pub(crate) fn semantic_kind(self) -> Option<IntrinsicSemantic> {
        match self.raw() {
            generated::SEMANTIC_LIFETIME_START => Some(IntrinsicSemantic::LifetimeStart),
            generated::SEMANTIC_LIFETIME_END => Some(IntrinsicSemantic::LifetimeEnd),
            generated::SEMANTIC_MEMCPY => Some(IntrinsicSemantic::Memcpy),
            generated::SEMANTIC_MEMMOVE => Some(IntrinsicSemantic::Memmove),
            generated::SEMANTIC_MEMSET => Some(IntrinsicSemantic::Memset),
            generated::SEMANTIC_EXPECT => Some(IntrinsicSemantic::Expect),
            generated::SEMANTIC_TRAP => Some(IntrinsicSemantic::Trap),
            generated::SEMANTIC_DONOTHING => Some(IntrinsicSemantic::Donothing),
            generated::SEMANTIC_READCYCLECOUNTER => Some(IntrinsicSemantic::ReadCycleCounter),
            generated::SEMANTIC_READ_REGISTER => Some(IntrinsicSemantic::ReadRegister),
            generated::SEMANTIC_WRITE_REGISTER => Some(IntrinsicSemantic::WriteRegister),
            generated::SEMANTIC_ASSUME => Some(IntrinsicSemantic::Assume),
            generated::SEMANTIC_ABS => Some(IntrinsicSemantic::Abs),
            generated::SEMANTIC_BSWAP => Some(IntrinsicSemantic::BSwap),
            generated::SEMANTIC_BITREVERSE => Some(IntrinsicSemantic::BitReverse),
            generated::SEMANTIC_CTLZ => Some(IntrinsicSemantic::Ctlz),
            generated::SEMANTIC_CTTZ => Some(IntrinsicSemantic::Cttz),
            generated::SEMANTIC_CTPOP => Some(IntrinsicSemantic::Ctpop),
            generated::SEMANTIC_FSHL => Some(IntrinsicSemantic::FShl),
            generated::SEMANTIC_FSHR => Some(IntrinsicSemantic::FShr),
            generated::SEMANTIC_UMAX => Some(IntrinsicSemantic::UMax),
            generated::SEMANTIC_UMIN => Some(IntrinsicSemantic::UMin),
            generated::SEMANTIC_SMAX => Some(IntrinsicSemantic::SMax),
            generated::SEMANTIC_SMIN => Some(IntrinsicSemantic::SMin),
            generated::SEMANTIC_UADD_SAT => Some(IntrinsicSemantic::UAddSat),
            generated::SEMANTIC_USUB_SAT => Some(IntrinsicSemantic::USubSat),
            generated::SEMANTIC_SADD_SAT => Some(IntrinsicSemantic::SAddSat),
            generated::SEMANTIC_SSUB_SAT => Some(IntrinsicSemantic::SSubSat),
            generated::SEMANTIC_VECTOR_REDUCE_ADD => Some(IntrinsicSemantic::VectorReduceAdd),
            generated::SEMANTIC_PTRMASK => Some(IntrinsicSemantic::PtrMask),
            generated::SEMANTIC_VSCALE => Some(IntrinsicSemantic::VScale),
            _ => None,
        }
    }

    pub fn function_type<'ctx, B, S>(
        self,
        module: &Module<'ctx, B, S>,
        overloads: &[Type<'ctx, B>],
    ) -> IrResult<FunctionType<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        IntrinsicDescriptor::new(self, overloads.to_vec())?.function_type(module)
    }

    pub fn match_signature<'ctx, B>(
        self,
        module: ModuleRef<'ctx, B>,
        fn_ty: FunctionType<'ctx, B>,
    ) -> IrResult<IntrinsicDescriptor<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        let overloads = match_intrinsic_signature(self, module, fn_ty)?;
        IntrinsicDescriptor::new(self, overloads)
    }

    fn record(self) -> &'static IntrinsicRecord {
        let Some(index) = usize::try_from(self.raw().saturating_sub(1)).ok() else {
            return &INVALID_INTRINSIC_RECORD;
        };
        match generated::INTRINSIC_RECORDS.get(index) {
            Some(record) => record,
            None => &INVALID_INTRINSIC_RECORD,
        }
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> IntrinsicDescriptor<'ctx, B> {
    pub fn new<Overloads>(id: IntrinsicId, overloads: Overloads) -> IrResult<Self>
    where
        Overloads: Into<Box<[Type<'ctx, B>]>>,
    {
        let overloads = overloads.into();
        let descriptors = iit_descriptors(id.record())?;
        if overloads.len() != overload_slot_count(&descriptors) {
            return Err(intrinsic_mismatch_for_id(id));
        }
        validate_intrinsic_overload_constraints(&descriptors, &overloads)
            .and_then(|()| {
                if let Some(first) = overloads.first() {
                    generated_function_type_from_descriptors(first.module, &descriptors, &overloads)
                        .map(|_| ())
                } else {
                    Ok(())
                }
            })
            .map_err(|_| intrinsic_mismatch_for_id(id))?;
        Ok(Self { id, overloads })
    }

    pub const fn id(&self) -> IntrinsicId {
        self.id
    }

    pub fn overloads(&self) -> &[Type<'ctx, B>] {
        &self.overloads
    }

    pub fn enum_name(&self) -> &'static str {
        self.id.enum_name()
    }

    pub fn base_name(&self) -> &'static str {
        self.id.base_name()
    }

    pub fn target_prefix(&self) -> Option<&'static str> {
        self.id.target_prefix()
    }

    pub fn is_overloaded(&self) -> bool {
        self.id.is_overloaded()
    }

    pub fn mangled_name(&self) -> IrResult<String> {
        if !self.id.is_overloaded() {
            return Ok(self.id.base_name().to_owned());
        }
        let mut name = self.id.base_name().to_owned();
        for overload in &self.overloads {
            name.push('.');
            append_mangled_type(*overload, &mut name)?;
        }
        Ok(name)
    }

    pub fn function_type<S>(&self, module: &Module<'ctx, B, S>) -> IrResult<FunctionType<'ctx, B>> {
        self.function_type_ref(module.module_ref())
    }

    pub(crate) fn function_type_ref(
        &self,
        module: ModuleRef<'ctx, B>,
    ) -> IrResult<FunctionType<'ctx, B>> {
        let descriptors = iit_descriptors(self.id.record())?;
        generated_function_type_from_descriptors(module, &descriptors, &self.overloads)
    }

    pub(crate) fn to_function_data(&self) -> IntrinsicFunctionData {
        IntrinsicFunctionData {
            id: self.id,
            overloads: self.overloads.iter().map(|ty| ty.id()).collect(),
        }
    }

    pub fn declaration_attributes(
        &self,
        fn_ty: FunctionType<'ctx, B>,
    ) -> IrResult<AttributeStorage> {
        let record = self.id.record();
        let mut storage = AttributeStorage::new();
        add_function_attrs::<B>(&mut storage, record);
        for indexed in record.arg_attrs {
            add_indexed_attr::<B>(&mut storage, *indexed, fn_ty)?;
        }
        Ok(storage)
    }

    pub(crate) fn argument_names(&self) -> impl Iterator<Item = (u32, &'static str)> + '_ {
        self.id
            .record()
            .pretty_print
            .iter()
            .filter(|arg| !arg.name.is_empty())
            .map(|arg| (arg.arg_index, arg.name))
    }

    pub(crate) fn pretty_print_arg(&self, arg_index: usize) -> Option<PrettyPrintArg> {
        let arg_index = u32::try_from(arg_index).ok()?;
        self.id
            .record()
            .pretty_print
            .iter()
            .copied()
            .find(|arg| arg.arg_index == arg_index)
    }

    pub fn immarg_operand_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.id
            .record()
            .arg_attrs
            .iter()
            .filter_map(|indexed| match indexed.attr {
                IntrinsicArgAttr::ImmArg => usize::try_from(indexed.index.saturating_sub(1)).ok(),
                _ => None,
            })
    }
}

fn add_function_attrs<B: ModuleBrand>(storage: &mut AttributeStorage, record: &IntrinsicRecord) {
    let attrs = record.fn_attrs;
    add_function_attr_if::<B>(storage, attrs.no_unwind, AttrKind::NoUnwind);
    add_function_attr_if::<B>(storage, attrs.no_return, AttrKind::NoReturn);
    add_function_attr_if::<B>(storage, attrs.no_callback, AttrKind::NoCallback);
    add_function_attr_if::<B>(storage, attrs.no_sync, AttrKind::NoSync);
    add_function_attr_if::<B>(storage, attrs.no_free, AttrKind::NoFree);
    add_function_attr_if::<B>(storage, attrs.will_return, AttrKind::WillReturn);
    add_function_attr_if::<B>(storage, attrs.cold, AttrKind::Cold);
    add_function_attr_if::<B>(storage, attrs.no_duplicate, AttrKind::NoDuplicate);
    add_function_attr_if::<B>(storage, attrs.no_merge, AttrKind::NoMerge);
    add_function_attr_if::<B>(storage, attrs.convergent, AttrKind::Convergent);
    add_function_attr_if::<B>(storage, attrs.speculatable, AttrKind::Speculatable);
    add_function_attr_if::<B>(storage, attrs.strict_fp, AttrKind::StrictFP);
    add_function_attr_if::<B>(
        storage,
        attrs.no_create_undef_or_poison,
        AttrKind::NoCreateUndefOrPoison,
    );
    if record.memory_effects != MemoryEffects::unknown() {
        storage.add(
            AttrIndex::Function,
            Attribute::<B>::memory_for_brand(record.memory_effects),
        );
    }
}

fn add_function_attr_if<B: ModuleBrand>(
    storage: &mut AttributeStorage,
    enabled: bool,
    kind: AttrKind,
) {
    if enabled {
        storage.add(AttrIndex::Function, Attribute::<B>::Enum(kind));
    }
}

fn add_indexed_attr<B: ModuleBrand>(
    storage: &mut AttributeStorage,
    indexed: IntrinsicIndexedAttr,
    fn_ty: FunctionType<'_, B>,
) -> IrResult<()> {
    let index = attribute_index(indexed.index);
    let attr = match indexed.attr {
        IntrinsicArgAttr::NoCapture => Attribute::<B>::Enum(AttrKind::NoCapture),
        IntrinsicArgAttr::NoAlias => Attribute::<B>::Enum(AttrKind::NoAlias),
        IntrinsicArgAttr::NoUndef => Attribute::<B>::Enum(AttrKind::NoUndef),
        IntrinsicArgAttr::NonNull => Attribute::<B>::Enum(AttrKind::NonNull),
        IntrinsicArgAttr::Returned => Attribute::<B>::Enum(AttrKind::Returned),
        IntrinsicArgAttr::ReadOnly => Attribute::<B>::Enum(AttrKind::ReadOnly),
        IntrinsicArgAttr::WriteOnly => Attribute::<B>::Enum(AttrKind::WriteOnly),
        IntrinsicArgAttr::ReadNone => Attribute::<B>::Enum(AttrKind::ReadNone),
        IntrinsicArgAttr::ImmArg => Attribute::<B>::Enum(AttrKind::ImmArg),
        IntrinsicArgAttr::Alignment(bytes) => Attribute::<B>::Int(AttrKind::Alignment, bytes),
        IntrinsicArgAttr::Dereferenceable(bytes) => {
            Attribute::<B>::Int(AttrKind::Dereferenceable, bytes)
        }
        IntrinsicArgAttr::Range { lower, upper } => {
            let ty = type_for_attribute_index(fn_ty, indexed.index)?;
            let TypeData::Integer { bits } = ty.data() else {
                return Err(intrinsic_mismatch());
            };
            let lower = ap_int_from_i64(*bits, lower)?;
            let upper = ap_int_from_i64(*bits, upper)?;
            Attribute::<B>::range(ty, lower, upper).ok_or_else(intrinsic_mismatch)?
        }
    };
    storage.add(index, attr);
    Ok(())
}

fn attribute_index(index: u32) -> AttrIndex {
    if index == 0 {
        AttrIndex::Return
    } else {
        AttrIndex::Param(index - 1)
    }
}

fn type_for_attribute_index<'ctx, B: ModuleBrand + 'ctx>(
    fn_ty: FunctionType<'ctx, B>,
    index: u32,
) -> IrResult<Type<'ctx, B>> {
    if index == 0 {
        return Ok(fn_ty.return_type());
    }
    let param_index = usize::try_from(index - 1).map_err(|_| intrinsic_mismatch())?;
    fn_ty
        .params()
        .nth(param_index)
        .ok_or_else(intrinsic_mismatch)
}

fn ap_int_from_i64(bits: u32, value: i64) -> IrResult<crate::ApInt> {
    crate::ApInt::new(
        bits,
        u64::from_ne_bytes(value.to_ne_bytes()),
        crate::ApIntSignedness::Signed,
        crate::ApIntTruncation::Truncate,
    )
}

fn append_mangled_type<'ctx, B: ModuleBrand + 'ctx>(
    ty: Type<'ctx, B>,
    out: &mut String,
) -> IrResult<()> {
    match ty.data() {
        TypeData::Void => out.push_str("isVoid"),
        TypeData::Half => out.push_str("f16"),
        TypeData::BFloat => out.push_str("bf16"),
        TypeData::Float => out.push_str("f32"),
        TypeData::Double => out.push_str("f64"),
        TypeData::X86Fp80 => out.push_str("f80"),
        TypeData::Fp128 => out.push_str("f128"),
        TypeData::PpcFp128 => out.push_str("ppcf128"),
        TypeData::X86Amx => out.push_str("x86amx"),
        TypeData::WasmExnRef => out.push_str("exnref"),
        TypeData::Metadata => out.push_str("Metadata"),
        TypeData::Token => out.push_str("token"),
        TypeData::Label => out.push_str("label"),
        TypeData::Integer { bits } => {
            out.push('i');
            out.push_str(&bits.to_string());
        }
        TypeData::Pointer { addr_space } => {
            out.push('p');
            out.push_str(&addr_space.to_string());
        }
        TypeData::TypedPointer { addr_space, .. } => {
            out.push('p');
            out.push_str(&addr_space.to_string());
        }
        TypeData::Array { elem, n } => {
            out.push('a');
            out.push_str(&n.to_string());
            append_mangled_type(Type::new(*elem, ty.module), out)?;
        }
        TypeData::FixedVector { elem, n } => {
            out.push('v');
            out.push_str(&n.to_string());
            append_mangled_type(Type::new(*elem, ty.module), out)?;
        }
        TypeData::ScalableVector { elem, min } => {
            out.push_str("nxv");
            out.push_str(&min.to_string());
            append_mangled_type(Type::new(*elem, ty.module), out)?;
        }
        TypeData::Function {
            ret,
            params,
            is_var_arg,
        } => {
            out.push_str("f_");
            append_mangled_type(Type::new(*ret, ty.module), out)?;
            for param in params {
                append_mangled_type(Type::new(*param, ty.module), out)?;
            }
            if *is_var_arg {
                out.push_str("vararg");
            }
            out.push('f');
        }
        TypeData::Struct(data) => {
            if let Some(name) = &data.name {
                out.push_str("s_");
                out.push_str(name);
                out.push('s');
                return Ok(());
            }
            let Some(body) = data.body.borrow().clone() else {
                return Err(IrError::InvalidOperation {
                    message: "unnamed intrinsic overload type requires unique module naming",
                });
            };
            out.push_str("sl_");
            for elem in &body.elements {
                append_mangled_type(Type::new(*elem, ty.module), out)?;
            }
            out.push('s');
        }
        TypeData::TargetExt(data) => {
            out.push('t');
            out.push_str(&data.name);
            for param in &data.type_params {
                out.push('_');
                append_mangled_type(Type::new(*param, ty.module), out)?;
            }
            for param in &data.int_params {
                out.push('_');
                out.push_str(&param.to_string());
            }
            out.push('t');
        }
    }
    Ok(())
}

pub(crate) fn match_intrinsic_signature<'ctx, B: ModuleBrand + 'ctx>(
    id: IntrinsicId,
    module: ModuleRef<'ctx, B>,
    fn_ty: FunctionType<'ctx, B>,
) -> IrResult<Box<[Type<'ctx, B>]>> {
    let descriptors = iit_descriptors(id.record())?;
    let mut cursor = descriptors.as_slice();
    let mut overloads = vec![None; overload_slot_count(&descriptors)];
    match_fixed_type(module, &mut cursor, &mut overloads, fn_ty.return_type())?;

    let params: Vec<_> = fn_ty.params().collect();
    let mut param_index = 0;
    let mut is_var_arg = false;
    while !cursor.is_empty() {
        if matches!(cursor[0], IitDescriptor::VarArg) {
            cursor = &cursor[1..];
            is_var_arg = true;
            break;
        }
        let Some(param) = params.get(param_index).copied() else {
            return Err(intrinsic_mismatch_for_id(id));
        };
        match_fixed_type(module, &mut cursor, &mut overloads, param)?;
        param_index += 1;
    }
    if !cursor.is_empty() || param_index != params.len() || fn_ty.is_var_arg() != is_var_arg {
        return Err(intrinsic_mismatch_for_id(id));
    }
    let mut resolved = Vec::with_capacity(overloads.len());
    for overload in overloads {
        let Some(overload) = overload else {
            return Err(intrinsic_mismatch_for_id(id));
        };
        resolved.push(overload);
    }
    validate_intrinsic_overload_constraints(&descriptors, &resolved)?;
    let expected = generated_function_type_from_descriptors(module, &descriptors, &resolved)?;
    if expected != fn_ty {
        return Err(intrinsic_mismatch_for_id(id));
    }
    Ok(resolved.into_boxed_slice())
}

fn match_fixed_type<'ctx, B: ModuleBrand + 'ctx>(
    module: ModuleRef<'ctx, B>,
    descriptors: &mut &[IitDescriptor],
    overloads: &mut [Option<Type<'ctx, B>>],
    actual: Type<'ctx, B>,
) -> IrResult<()> {
    let Some((&descriptor, rest)) = descriptors.split_first() else {
        return Err(intrinsic_mismatch());
    };
    *descriptors = rest;
    match descriptor {
        IitDescriptor::Void | IitDescriptor::VarArg => require_type(actual, void_type(module)),
        IitDescriptor::Mmx => {
            require_type(actual, fixed_vector_type(module, int_type(module, 64), 1))
        }
        IitDescriptor::Amx => require_type(
            actual,
            Type::new(module.module().context().x86_amx(), module),
        ),
        IitDescriptor::ExnRef => require_type(
            actual,
            Type::new(module.module().context().wasm_exnref(), module),
        ),
        IitDescriptor::Token => {
            require_type(actual, Type::new(module.module().context().token(), module))
        }
        IitDescriptor::Metadata => require_type(actual, metadata_type(module)),
        IitDescriptor::Half => {
            require_type(actual, Type::new(module.module().context().half(), module))
        }
        IitDescriptor::BFloat => require_type(
            actual,
            Type::new(module.module().context().bfloat(), module),
        ),
        IitDescriptor::Float => {
            require_type(actual, Type::new(module.module().context().float(), module))
        }
        IitDescriptor::Double => require_type(
            actual,
            Type::new(module.module().context().double(), module),
        ),
        IitDescriptor::Quad => {
            require_type(actual, Type::new(module.module().context().fp128(), module))
        }
        IitDescriptor::PpcQuad => require_type(
            actual,
            Type::new(module.module().context().ppc_fp128(), module),
        ),
        IitDescriptor::AArch64Svcount => {
            require_type(actual, target_ext_type(module, "aarch64.svcount", [], []))
        }
        IitDescriptor::Integer(bits) => require_type(actual, int_type(module, bits)),
        IitDescriptor::Pointer(addr_space) => require_type(actual, ptr_type(module, addr_space)),
        IitDescriptor::Vector { width, scalable } => {
            let elem = match actual.data() {
                TypeData::FixedVector { elem, n } if !scalable && *n == width => *elem,
                TypeData::ScalableVector { elem, min } if scalable && *min == width => *elem,
                _ => return Err(intrinsic_mismatch()),
            };
            match_fixed_type(module, descriptors, overloads, Type::new(elem, module))
        }
        IitDescriptor::Struct { elements } => {
            let TypeData::Struct(data) = actual.data() else {
                return Err(intrinsic_mismatch());
            };
            let Some(body) = data.body.borrow().clone() else {
                return Err(intrinsic_mismatch());
            };
            if body.packed || body.elements.len() != elements {
                return Err(intrinsic_mismatch());
            }
            for elem in &body.elements {
                match_fixed_type(module, descriptors, overloads, Type::new(*elem, module))?;
            }
            Ok(())
        }
        IitDescriptor::Argument { index, kind } => {
            validate_overload_kind(module, actual, kind)?;
            match_overload_slot(overloads, index, actual)
        }
        IitDescriptor::ExtendArgument(index) => {
            match_transformed_overload(module, overloads, index, actual, extend_integer_type)
        }
        IitDescriptor::TruncArgument(index) => {
            match_transformed_overload(module, overloads, index, actual, trunc_argument_type)
        }
        IitDescriptor::SameVecWidthArgument(index) => {
            match_same_vec_width_argument(module, descriptors, overloads, index, actual)
        }
        IitDescriptor::VecElementArgument(index) => {
            match_transformed_overload(module, overloads, index, actual, vector_element_or_self)
        }
        IitDescriptor::VecOfAnyPtrsToElt {
            overload,
            reference,
        } => {
            match_vector_of_any_ptrs_to_ref(overloads, overload, reference, actual)?;
            match_overload_slot(overloads, overload, actual)
        }
        IitDescriptor::OneNthEltsVecArgument { divisor, argument } => {
            match_transformed_overload(module, overloads, argument, actual, |module, ty| {
                one_nth_vector_type(module, ty, divisor)
            })
        }
        IitDescriptor::Subdivide2Argument(index) => {
            match_transformed_overload(module, overloads, index, actual, |module, ty| {
                subdivide_vector_type(module, ty, 1)
            })
        }
        IitDescriptor::Subdivide4Argument(index) => {
            match_transformed_overload(module, overloads, index, actual, |module, ty| {
                subdivide_vector_type(module, ty, 2)
            })
        }
        IitDescriptor::VecOfBitcastsToInt(index) => {
            match_transformed_overload(module, overloads, index, actual, vector_of_bitcasts_to_int)
        }
    }
}

fn match_overload_slot<'ctx, B: ModuleBrand + 'ctx>(
    overloads: &mut [Option<Type<'ctx, B>>],
    index: usize,
    actual: Type<'ctx, B>,
) -> IrResult<()> {
    let Some(slot) = overloads.get_mut(index) else {
        return Err(intrinsic_mismatch());
    };
    if let Some(existing) = *slot {
        require_type(actual, existing)
    } else {
        *slot = Some(actual);
        Ok(())
    }
}

fn match_vector_of_any_ptrs_to_ref<'ctx, B: ModuleBrand + 'ctx>(
    overloads: &[Option<Type<'ctx, B>>],
    overload: usize,
    reference: usize,
    actual: Type<'ctx, B>,
) -> IrResult<()> {
    require_pointer_vector(actual)?;
    let Some(Some(reference_ty)) = overloads.get(reference).copied() else {
        return Ok(());
    };
    require_same_vector_width(actual, reference_ty)?;
    let Some(slot) = overloads.get(overload) else {
        return Err(intrinsic_mismatch());
    };
    if slot.is_some() {
        return Err(intrinsic_mismatch());
    }
    Ok(())
}

fn validate_intrinsic_overload_constraints<'ctx, B: ModuleBrand + 'ctx>(
    descriptors: &[IitDescriptor],
    overloads: &[Type<'ctx, B>],
) -> IrResult<()> {
    for descriptor in descriptors {
        if let IitDescriptor::VecOfAnyPtrsToElt {
            overload,
            reference,
        } = *descriptor
        {
            let Some(actual) = overloads.get(overload).copied() else {
                return Err(intrinsic_mismatch());
            };
            let Some(reference_ty) = overloads.get(reference).copied() else {
                return Err(intrinsic_mismatch());
            };
            require_pointer_vector(actual)?;
            require_same_vector_width(actual, reference_ty)?;
        }
    }
    Ok(())
}

fn require_pointer_vector<'ctx, B: ModuleBrand + 'ctx>(ty: Type<'ctx, B>) -> IrResult<()> {
    let elem = match ty.data() {
        TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => *elem,
        _ => return Err(intrinsic_mismatch()),
    };
    match Type::new(elem, ty.module).data() {
        TypeData::Pointer { .. } => Ok(()),
        _ => Err(intrinsic_mismatch()),
    }
}

fn require_same_vector_width<'ctx, B: ModuleBrand + 'ctx>(
    actual: Type<'ctx, B>,
    reference: Type<'ctx, B>,
) -> IrResult<()> {
    match (actual.data(), reference.data()) {
        (
            TypeData::FixedVector {
                n: actual_lanes, ..
            },
            TypeData::FixedVector {
                n: reference_lanes, ..
            },
        ) if actual_lanes == reference_lanes => Ok(()),
        (
            TypeData::ScalableVector {
                min: actual_lanes, ..
            },
            TypeData::ScalableVector {
                min: reference_lanes,
                ..
            },
        ) if actual_lanes == reference_lanes => Ok(()),
        _ => Err(intrinsic_mismatch()),
    }
}

fn match_same_vec_width_argument<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    descriptors: &mut &[IitDescriptor],
    overloads: &mut [Option<Type<'ctx, B>>],
    index: usize,
    actual: Type<'ctx, B>,
) -> IrResult<()>
where
    B: ModuleBrand + 'ctx,
{
    let Some(reference) = overloads.get(index).copied() else {
        return Err(intrinsic_mismatch());
    };
    let Some(reference) = reference else {
        skip_fixed_type_descriptor(descriptors)?;
        return Ok(());
    };

    let actual_elem = match (reference.data(), actual.data()) {
        (
            TypeData::FixedVector {
                n: reference_lanes, ..
            },
            TypeData::FixedVector {
                elem,
                n: actual_lanes,
            },
        ) if reference_lanes == actual_lanes => Type::new(*elem, module),
        (
            TypeData::ScalableVector {
                min: reference_lanes,
                ..
            },
            TypeData::ScalableVector {
                elem,
                min: actual_lanes,
            },
        ) if reference_lanes == actual_lanes => Type::new(*elem, module),
        (TypeData::FixedVector { .. } | TypeData::ScalableVector { .. }, _)
        | (_, TypeData::FixedVector { .. } | TypeData::ScalableVector { .. }) => {
            return Err(intrinsic_mismatch());
        }
        _ => actual,
    };
    match_fixed_type(module, descriptors, overloads, actual_elem)
}

fn skip_fixed_type_descriptor(descriptors: &mut &[IitDescriptor]) -> IrResult<()> {
    let Some((&descriptor, rest)) = descriptors.split_first() else {
        return Err(intrinsic_mismatch());
    };
    *descriptors = rest;
    match descriptor {
        IitDescriptor::Vector { .. } | IitDescriptor::SameVecWidthArgument(_) => {
            skip_fixed_type_descriptor(descriptors)
        }
        IitDescriptor::Struct { elements } => {
            for _ in 0..elements {
                skip_fixed_type_descriptor(descriptors)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn match_transformed_overload<'ctx, B, F>(
    module: ModuleRef<'ctx, B>,
    overloads: &mut [Option<Type<'ctx, B>>],
    index: usize,
    actual: Type<'ctx, B>,
    transform: F,
) -> IrResult<()>
where
    B: ModuleBrand + 'ctx,
    F: FnOnce(ModuleRef<'ctx, B>, Type<'ctx, B>) -> IrResult<Type<'ctx, B>>,
{
    let Some(Some(source)) = overloads.get(index).copied() else {
        return Ok(());
    };
    require_type(actual, transform(module, source)?)
}

fn require_type<'ctx, B: ModuleBrand + 'ctx>(
    actual: Type<'ctx, B>,
    expected: Type<'ctx, B>,
) -> IrResult<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(intrinsic_mismatch())
    }
}

impl BinaryIntrinsic {
    pub fn from_intrinsic_id(id: IntrinsicId) -> Option<Self> {
        Self::from_semantic(id.semantic_kind()?)
    }

    pub fn from_intrinsic_name(name: &str) -> Option<Self> {
        let id = IntrinsicId::lookup(name)?;
        let binary = Self::from_intrinsic_id(id)?;
        let descriptor_matches_name = Module::with_new("intrinsic-name-resolution", |module| {
            descriptor_for_name(module.module_ref(), id, name).is_ok()
        });
        descriptor_matches_name.then_some(binary)
    }

    fn from_semantic(semantic: IntrinsicSemantic) -> Option<Self> {
        match semantic {
            IntrinsicSemantic::FShl => Some(Self::FShl),
            IntrinsicSemantic::FShr => Some(Self::FShr),
            IntrinsicSemantic::UMax => Some(Self::UMax),
            IntrinsicSemantic::UMin => Some(Self::UMin),
            IntrinsicSemantic::SMax => Some(Self::SMax),
            IntrinsicSemantic::SMin => Some(Self::SMin),
            IntrinsicSemantic::UAddSat => Some(Self::UAddSat),
            IntrinsicSemantic::USubSat => Some(Self::USubSat),
            IntrinsicSemantic::SAddSat => Some(Self::SAddSat),
            IntrinsicSemantic::SSubSat => Some(Self::SSubSat),
            _ => None,
        }
    }
}

fn intrinsic_mismatch_for_id(id: IntrinsicId) -> IrError {
    IrError::IntrinsicSignatureMismatch {
        name: id.base_name().to_owned(),
    }
}

fn target_set_for_name(name: &str) -> Option<&'static IntrinsicTargetSet> {
    let rest = name.strip_prefix("llvm.")?;
    let first_component = rest.split('.').next()?;
    if let Some(target) = generated::INTRINSIC_TARGET_SETS
        .iter()
        .find(|set| !set.prefix.is_empty() && set.prefix == first_component)
    {
        return Some(target);
    }
    generated::INTRINSIC_TARGET_SETS
        .iter()
        .find(|set| set.prefix.is_empty())
}

fn lookup_in_target_set(name: &str, set: &IntrinsicTargetSet) -> Option<IntrinsicId> {
    let records = records_for_target_set(set)?;
    let mut candidate = name;
    loop {
        if let Some(relative_index) = base_name_index(records, candidate) {
            let record = records.get(relative_index)?;
            if record_matches_name(record, name) {
                return intrinsic_id_for_relative_index(set, relative_index);
            }
        }
        let dot_index = candidate.rfind('.')?;
        candidate = &candidate[..dot_index];
        if candidate == "llvm" {
            return None;
        }
    }
}

fn records_for_target_set(set: &IntrinsicTargetSet) -> Option<&'static [IntrinsicRecord]> {
    let start = usize::try_from(set.offset).ok()?;
    let count = usize::try_from(set.count).ok()?;
    let end = start.checked_add(count)?;
    generated::INTRINSIC_RECORDS.get(start..end)
}

fn base_name_index(records: &[IntrinsicRecord], name: &str) -> Option<usize> {
    records
        .binary_search_by(|record| record.base_name.cmp(name))
        .ok()
}

fn record_matches_name(record: &IntrinsicRecord, name: &str) -> bool {
    if name == record.base_name {
        return true;
    }
    record
        .is_overloaded
        .then(|| name.strip_prefix(record.base_name))
        .flatten()
        .and_then(|suffix| suffix.strip_prefix('.'))
        .is_some_and(|suffix| !suffix.is_empty())
}

fn intrinsic_id_for_relative_index(
    set: &IntrinsicTargetSet,
    relative_index: usize,
) -> Option<IntrinsicId> {
    let relative = u32::try_from(relative_index).ok()?;
    let raw = set.offset.checked_add(relative)?.checked_add(1)?;
    IntrinsicId::from_raw(raw)
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

pub(crate) fn descriptor_for_name<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    id: IntrinsicId,
    name: &str,
) -> IrResult<IntrinsicDescriptor<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
{
    if IntrinsicId::lookup(name) != Some(id) {
        return Err(intrinsic_mismatch_for_id(id));
    }
    let record = id.record();
    let descriptors = iit_descriptors(record)?;
    if !record.is_overloaded {
        if name != record.base_name {
            return Err(intrinsic_mismatch_for_id(id));
        }
        return IntrinsicDescriptor::new(id, Vec::<Type<'ctx, B>>::new());
    }

    let suffix = name
        .strip_prefix(record.base_name)
        .and_then(|rest| rest.strip_prefix('.'))
        .filter(|rest| !rest.is_empty())
        .ok_or_else(|| intrinsic_mismatch_for_id(id))?;
    let overloads = parse_mangled_overload_types(module, suffix)?;
    if overloads.len() != overload_slot_count(&descriptors) {
        return Err(intrinsic_mismatch_for_id(id));
    }
    IntrinsicDescriptor::new(id, overloads)
}

fn generated_function_type_from_descriptors<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    descriptors: &[IitDescriptor],
    overloads: &[Type<'ctx, B>],
) -> IrResult<FunctionType<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
{
    let mut descriptors = descriptors;
    let ret = decode_fixed_type(module, &mut descriptors, overloads)?;
    let mut params = Vec::new();
    while !descriptors.is_empty() {
        params.push(decode_fixed_type(module, &mut descriptors, overloads)?);
    }

    let is_var_arg = params
        .last()
        .is_some_and(|ty| matches!(ty.data(), TypeData::Void));
    if is_var_arg {
        params.pop();
    }
    fn_type_with_var_arg(module, ret, params, is_var_arg)
}

fn iit_descriptors(record: &IntrinsicRecord) -> IrResult<Vec<IitDescriptor>> {
    let entries = iit_entries(record).ok_or_else(intrinsic_mismatch)?;
    let mut pos = 0;
    let mut descriptors = Vec::new();
    decode_iit_type(&entries, &mut pos, 0, &mut descriptors)?;
    while pos < entries.len() && entries[pos] != 0 {
        decode_iit_type(&entries, &mut pos, 0, &mut descriptors)?;
    }
    Ok(descriptors)
}

fn overload_slot_count(descriptors: &[IitDescriptor]) -> usize {
    descriptors
        .iter()
        .filter_map(|descriptor| match *descriptor {
            IitDescriptor::Argument { index, .. }
            | IitDescriptor::ExtendArgument(index)
            | IitDescriptor::TruncArgument(index)
            | IitDescriptor::SameVecWidthArgument(index)
            | IitDescriptor::VecElementArgument(index)
            | IitDescriptor::Subdivide2Argument(index)
            | IitDescriptor::Subdivide4Argument(index)
            | IitDescriptor::VecOfBitcastsToInt(index) => Some(index),
            IitDescriptor::OneNthEltsVecArgument { argument, .. } => Some(argument),
            IitDescriptor::VecOfAnyPtrsToElt {
                overload,
                reference,
            } => Some(overload.max(reference)),
            _ => None,
        })
        .max()
        .map_or(0, |index| index + 1)
}

fn parse_mangled_overload_types<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    suffix: &str,
) -> IrResult<Vec<Type<'ctx, B>>>
where
    B: ModuleBrand + 'ctx,
{
    let mut rest = suffix;
    let mut overloads = Vec::new();
    loop {
        let (ty, consumed) = parse_mangled_type_prefix(module, rest)?;
        overloads.push(ty);
        rest = rest.get(consumed..).ok_or_else(intrinsic_mismatch)?;
        if rest.is_empty() {
            return Ok(overloads);
        }
        rest = rest
            .strip_prefix('.')
            .filter(|tail| !tail.is_empty())
            .ok_or_else(intrinsic_mismatch)?;
    }
}

fn parse_mangled_type_prefix<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    text: &str,
) -> IrResult<(Type<'ctx, B>, usize)>
where
    B: ModuleBrand + 'ctx,
{
    for (spelling, ty) in [
        (
            "ppcf128",
            Type::new(module.module().context().ppc_fp128(), module),
        ),
        (
            "x86amx",
            Type::new(module.module().context().x86_amx(), module),
        ),
        ("Metadata", metadata_type(module)),
        ("isVoid", void_type(module)),
        (
            "bf16",
            Type::new(module.module().context().bfloat(), module),
        ),
        ("f128", Type::new(module.module().context().fp128(), module)),
        (
            "f80",
            Type::new(module.module().context().x86_fp80(), module),
        ),
        ("f64", Type::new(module.module().context().double(), module)),
        ("f32", Type::new(module.module().context().float(), module)),
        ("f16", Type::new(module.module().context().half(), module)),
    ] {
        if text.starts_with(spelling) {
            return Ok((ty, spelling.len()));
        }
    }

    if let Some(consumed) = text.strip_prefix("taarch64.svcountt").map(str::len) {
        return Ok((target_ext_type(module, "aarch64.svcount", [], []), consumed));
    }

    if let Some(parsed) = parse_function_type_prefix(module, text)? {
        return Ok(parsed);
    }
    if let Some(parsed) = parse_struct_type_prefix(module, text)? {
        return Ok(parsed);
    }

    if let Some(parsed) = parse_target_ext_type_prefix(module, text)? {
        return Ok(parsed);
    }

    if let Some((bits, consumed)) = parse_prefixed_u32(text, "i", false) {
        return Ok((int_type(module, bits), consumed));
    }
    if let Some((addr_space, consumed)) = parse_prefixed_u32(text, "p", true) {
        return Ok((ptr_type(module, addr_space), consumed));
    }
    if let Some((lanes, consumed)) = parse_prefixed_u32(text, "nxv", false) {
        let (elem, elem_consumed) = parse_mangled_type_prefix(module, &text[consumed..])?;
        return Ok((
            scalable_vector_type(module, elem, lanes),
            consumed + elem_consumed,
        ));
    }
    if let Some((lanes, consumed)) = parse_prefixed_u32(text, "v", false) {
        let (elem, elem_consumed) = parse_mangled_type_prefix(module, &text[consumed..])?;
        return Ok((
            fixed_vector_type(module, elem, lanes),
            consumed + elem_consumed,
        ));
    }
    if let Some((elements, consumed)) = parse_prefixed_u64(text, "a", false) {
        let (elem, elem_consumed) = parse_mangled_type_prefix(module, &text[consumed..])?;
        return Ok((array_type(module, elem, elements), consumed + elem_consumed));
    }
    Err(intrinsic_mismatch())
}

fn parse_target_ext_type_prefix<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    text: &str,
) -> IrResult<Option<(Type<'ctx, B>, usize)>>
where
    B: ModuleBrand + 'ctx,
{
    let Some(body_and_rest) = text.strip_prefix('t') else {
        return Ok(None);
    };
    for (end, ch) in body_and_rest.char_indices().rev() {
        if ch != 't' {
            continue;
        }
        let body = body_and_rest.get(..end).ok_or_else(intrinsic_mismatch)?;
        if let Some(ty) = parse_target_ext_type_body(module, body)? {
            return Ok(Some((ty, end + 2)));
        }
    }
    Ok(None)
}

fn parse_target_ext_type_body<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    body: &str,
) -> IrResult<Option<Type<'ctx, B>>>
where
    B: ModuleBrand + 'ctx,
{
    let (name, mut params) = match body.split_once('_') {
        Some((name, params)) => (name, Some(params)),
        None => (body, None),
    };
    if name.is_empty() {
        return Ok(None);
    }

    let mut type_params = Vec::new();
    let mut int_params = Vec::new();
    while let Some(rest) = params {
        if rest.is_empty() {
            return Ok(None);
        }
        if let Some((value, consumed)) = parse_prefixed_u32(rest, "", true)
            && (consumed == rest.len() || rest.as_bytes().get(consumed) == Some(&b'_'))
        {
            int_params.push(value);
            params = rest.get(consumed..).and_then(|tail| tail.strip_prefix('_'));
            continue;
        }

        let (ty, consumed) = match parse_mangled_type_prefix(module, rest) {
            Ok(parsed) => parsed,
            Err(_) => return Ok(None),
        };
        if consumed == rest.len() {
            type_params.push(ty);
            break;
        }
        if rest.as_bytes().get(consumed) != Some(&b'_') {
            return Ok(None);
        }
        type_params.push(ty);
        params = rest.get(consumed + 1..);
    }
    Ok(Some(target_ext_type(module, name, type_params, int_params)))
}

fn parse_function_type_prefix<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    text: &str,
) -> IrResult<Option<(Type<'ctx, B>, usize)>>
where
    B: ModuleBrand + 'ctx,
{
    let Some(body_and_rest) = text.strip_prefix("f_") else {
        return Ok(None);
    };
    for (end, ch) in body_and_rest.char_indices().rev() {
        if ch != 'f' {
            continue;
        }
        let body = body_and_rest.get(..end).ok_or_else(intrinsic_mismatch)?;
        let (body, is_var_arg) = match body.strip_suffix("vararg") {
            Some(body) => (body, true),
            None => (body, false),
        };
        let (ret, consumed) = match parse_mangled_type_prefix(module, body) {
            Ok(parsed) => parsed,
            Err(_) => continue,
        };
        let params = match body.get(consumed..) {
            Some(params) => params,
            None => continue,
        };
        let Some(params) = parse_mangled_type_sequence(module, params)? else {
            continue;
        };
        let fn_ty = fn_type_with_var_arg(module, ret, params, is_var_arg)?;
        return Ok(Some((fn_ty.as_type(), end + 3)));
    }
    Ok(None)
}

fn parse_struct_type_prefix<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    text: &str,
) -> IrResult<Option<(Type<'ctx, B>, usize)>>
where
    B: ModuleBrand + 'ctx,
{
    if let Some(parsed) = parse_literal_struct_type_prefix(module, text)? {
        return Ok(Some(parsed));
    }
    let Some(body_and_rest) = text.strip_prefix("s_") else {
        return Ok(None);
    };
    for (end, ch) in body_and_rest.char_indices().rev() {
        if ch != 's' {
            continue;
        }
        let name = body_and_rest.get(..end).ok_or_else(intrinsic_mismatch)?;
        if name.is_empty() {
            continue;
        }
        let (id, _) = module.module().context().get_or_create_named_struct(name);
        return Ok(Some((Type::new(id, module), end + 3)));
    }
    Ok(None)
}

fn parse_literal_struct_type_prefix<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    text: &str,
) -> IrResult<Option<(Type<'ctx, B>, usize)>>
where
    B: ModuleBrand + 'ctx,
{
    let Some(body_and_rest) = text.strip_prefix("sl_") else {
        return Ok(None);
    };
    for (end, ch) in body_and_rest.char_indices().rev() {
        if ch != 's' {
            continue;
        }
        let body = body_and_rest.get(..end).ok_or_else(intrinsic_mismatch)?;
        let Some(elements) = parse_mangled_type_sequence(module, body)? else {
            continue;
        };
        let elements = elements.iter().map(|ty| ty.id()).collect::<Box<[_]>>();
        return Ok(Some((
            Type::new(
                module
                    .module()
                    .context()
                    .literal_struct_type(elements, false),
                module,
            ),
            end + 4,
        )));
    }
    Ok(None)
}

fn parse_mangled_type_sequence<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    mut text: &str,
) -> IrResult<Option<Vec<Type<'ctx, B>>>>
where
    B: ModuleBrand + 'ctx,
{
    let mut types = Vec::new();
    while !text.is_empty() {
        let (ty, consumed) = match parse_mangled_type_prefix(module, text) {
            Ok(parsed) => parsed,
            Err(_) => return Ok(None),
        };
        if consumed == 0 {
            return Ok(None);
        }
        types.push(ty);
        let Some(rest) = text.get(consumed..) else {
            return Ok(None);
        };
        text = rest;
    }
    Ok(Some(types))
}

fn parse_prefixed_u32(text: &str, prefix: &str, allow_zero: bool) -> Option<(u32, usize)> {
    let digits = text.strip_prefix(prefix)?;
    let len = digits.bytes().take_while(u8::is_ascii_digit).count();
    if len == 0 {
        return None;
    }
    let value = digits.get(..len)?.parse().ok()?;
    if !allow_zero && value == 0 {
        return None;
    }
    Some((value, prefix.len() + len))
}

fn parse_prefixed_u64(text: &str, prefix: &str, allow_zero: bool) -> Option<(u64, usize)> {
    let digits = text.strip_prefix(prefix)?;
    let len = digits.bytes().take_while(u8::is_ascii_digit).count();
    if len == 0 {
        return None;
    }
    let value = digits.get(..len)?.parse().ok()?;
    if !allow_zero && value == 0 {
        return None;
    }
    Some((value, prefix.len() + len))
}

fn iit_entries(record: &IntrinsicRecord) -> Option<Cow<'static, [u8]>> {
    let index = usize::try_from(record.iit_table_index).ok()?;
    let mut value = *generated::IIT_TABLE.get(index)?;
    if value >> 15 != 0 {
        let start = usize::from(value & 0x7fff);
        return generated::IIT_LONG_ENCODING_TABLE
            .get(start..)
            .map(Cow::Borrowed);
    }

    let mut entries = Vec::new();
    loop {
        entries.push(u8::try_from(value & 0x000f).ok()?);
        value >>= 4;
        if value == 0 {
            break;
        }
    }
    Some(Cow::Owned(entries))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum IitDescriptor {
    Void,
    VarArg,
    Mmx,
    ExnRef,
    Amx,
    Token,
    Metadata,
    Half,
    BFloat,
    Float,
    Double,
    Quad,
    PpcQuad,
    AArch64Svcount,
    Integer(u32),
    Vector { width: u32, scalable: bool },
    Pointer(u32),
    Struct { elements: usize },
    Argument { index: usize, kind: IitArgKind },
    ExtendArgument(usize),
    TruncArgument(usize),
    OneNthEltsVecArgument { divisor: u32, argument: usize },
    SameVecWidthArgument(usize),
    VecElementArgument(usize),
    VecOfAnyPtrsToElt { overload: usize, reference: usize },
    Subdivide2Argument(usize),
    Subdivide4Argument(usize),
    VecOfBitcastsToInt(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum IitArgKind {
    Any,
    AnyInteger,
    AnyFloat,
    AnyVector,
    AnyPointer,
    MatchType,
}

fn decode_argument_info(info: usize) -> IrResult<(usize, IitArgKind)> {
    let kind = match info & 7 {
        0 => IitArgKind::Any,
        1 => IitArgKind::AnyInteger,
        2 => IitArgKind::AnyFloat,
        3 => IitArgKind::AnyVector,
        4 => IitArgKind::AnyPointer,
        7 => IitArgKind::MatchType,
        _ => return Err(intrinsic_mismatch()),
    };
    Ok((info >> 3, kind))
}

fn decode_argument_index(info: usize) -> IrResult<usize> {
    decode_argument_info(info).map(|(index, _)| index)
}

fn decode_iit_type(
    entries: &[u8],
    pos: &mut usize,
    last_info: u8,
    out: &mut Vec<IitDescriptor>,
) -> IrResult<()> {
    let info = *entries.get(*pos).ok_or_else(intrinsic_mismatch)?;
    *pos += 1;
    let scalable = last_info == 35;
    match info {
        0 => out.push(IitDescriptor::Void),
        1 => out.push(IitDescriptor::Integer(1)),
        2 => out.push(IitDescriptor::Integer(8)),
        3 => out.push(IitDescriptor::Integer(16)),
        4 => out.push(IitDescriptor::Integer(32)),
        5 => out.push(IitDescriptor::Integer(64)),
        6 => out.push(IitDescriptor::Half),
        7 => out.push(IitDescriptor::Float),
        8 => out.push(IitDescriptor::Double),
        9 => decode_iit_vector(entries, pos, info, out, 2, scalable)?,
        10 => decode_iit_vector(entries, pos, info, out, 4, scalable)?,
        11 => decode_iit_vector(entries, pos, info, out, 8, scalable)?,
        12 => decode_iit_vector(entries, pos, info, out, 16, scalable)?,
        13 => decode_iit_vector(entries, pos, info, out, 32, scalable)?,
        14 => out.push(IitDescriptor::Pointer(0)),
        15 => {
            let (index, kind) = decode_argument_info(next_iit_usize_or_zero(entries, pos))?;
            out.push(IitDescriptor::Argument { index, kind });
        }
        16 => decode_iit_vector(entries, pos, info, out, 64, scalable)?,
        17 => out.push(IitDescriptor::Mmx),
        18 => out.push(IitDescriptor::Token),
        19 => out.push(IitDescriptor::Metadata),
        20 => out.push(IitDescriptor::Struct { elements: 0 }),
        21 => {
            let elements = usize::from(next_iit_byte(entries, pos)?) + 2;
            out.push(IitDescriptor::Struct { elements });
            for _ in 0..elements {
                decode_iit_type(entries, pos, info, out)?;
            }
        }
        22 => out.push(IitDescriptor::ExtendArgument(decode_argument_index(
            next_iit_usize_or_zero(entries, pos),
        )?)),
        23 => out.push(IitDescriptor::TruncArgument(decode_argument_index(
            next_iit_usize_or_zero(entries, pos),
        )?)),
        24 => out.push(IitDescriptor::Pointer(u32::from(next_iit_byte(
            entries, pos,
        )?))),
        25 => decode_iit_vector(entries, pos, info, out, 1, scalable)?,
        26 => out.push(IitDescriptor::VarArg),
        27 => {
            let argument = next_iit_usize_or_zero(entries, pos);
            let divisor = u32::from(next_iit_byte_or_zero(entries, pos));
            out.push(IitDescriptor::OneNthEltsVecArgument { divisor, argument });
        }
        28 => out.push(IitDescriptor::SameVecWidthArgument(decode_argument_index(
            next_iit_usize_or_zero(entries, pos),
        )?)),
        29 => out.push(IitDescriptor::VecOfAnyPtrsToElt {
            overload: next_iit_usize_or_zero(entries, pos),
            reference: next_iit_usize_or_zero(entries, pos),
        }),
        30 => out.push(IitDescriptor::Integer(128)),
        31 => decode_iit_vector(entries, pos, info, out, 512, scalable)?,
        32 => decode_iit_vector(entries, pos, info, out, 1024, scalable)?,
        33 => out.push(IitDescriptor::Quad),
        34 => out.push(IitDescriptor::VecElementArgument(decode_argument_index(
            next_iit_usize_or_zero(entries, pos),
        )?)),
        35 => decode_iit_type(entries, pos, info, out)?,
        36 => out.push(IitDescriptor::Subdivide2Argument(decode_argument_index(
            next_iit_usize_or_zero(entries, pos),
        )?)),
        37 => out.push(IitDescriptor::Subdivide4Argument(decode_argument_index(
            next_iit_usize_or_zero(entries, pos),
        )?)),
        38 => out.push(IitDescriptor::VecOfBitcastsToInt(decode_argument_index(
            next_iit_usize_or_zero(entries, pos),
        )?)),
        39 => decode_iit_vector(entries, pos, info, out, 128, scalable)?,
        40 => out.push(IitDescriptor::BFloat),
        41 => decode_iit_vector(entries, pos, info, out, 256, scalable)?,
        42 => out.push(IitDescriptor::Amx),
        43 => out.push(IitDescriptor::PpcQuad),
        44 => decode_iit_vector(entries, pos, info, out, 3, scalable)?,
        45 => out.push(IitDescriptor::Pointer(10)),
        46 => out.push(IitDescriptor::Pointer(20)),
        47 => out.push(IitDescriptor::Integer(2)),
        48 => out.push(IitDescriptor::Integer(4)),
        49 => out.push(IitDescriptor::AArch64Svcount),
        50 => decode_iit_vector(entries, pos, info, out, 6, scalable)?,
        51 => decode_iit_vector(entries, pos, info, out, 10, scalable)?,
        52 => decode_iit_vector(entries, pos, info, out, 2048, scalable)?,
        53 => decode_iit_vector(entries, pos, info, out, 4096, scalable)?,
        code if code == generated::IIT_WASM_EXNREF => out.push(IitDescriptor::ExnRef),
        _ => return Err(intrinsic_mismatch()),
    }
    Ok(())
}

fn decode_iit_vector(
    entries: &[u8],
    pos: &mut usize,
    info: u8,
    out: &mut Vec<IitDescriptor>,
    width: u32,
    scalable: bool,
) -> IrResult<()> {
    out.push(IitDescriptor::Vector { width, scalable });
    decode_iit_type(entries, pos, info, out)
}

fn next_iit_byte(entries: &[u8], pos: &mut usize) -> IrResult<u8> {
    let value = *entries.get(*pos).ok_or_else(intrinsic_mismatch)?;
    *pos += 1;
    Ok(value)
}

fn next_iit_byte_or_zero(entries: &[u8], pos: &mut usize) -> u8 {
    let Some(value) = entries.get(*pos).copied() else {
        return 0;
    };
    *pos += 1;
    value
}

fn next_iit_usize_or_zero(entries: &[u8], pos: &mut usize) -> usize {
    usize::from(next_iit_byte_or_zero(entries, pos))
}

fn decode_fixed_type<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    descriptors: &mut &[IitDescriptor],
    overloads: &[Type<'ctx, B>],
) -> IrResult<Type<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
{
    let Some((&descriptor, rest)) = descriptors.split_first() else {
        return Err(intrinsic_mismatch());
    };
    *descriptors = rest;

    match descriptor {
        IitDescriptor::Void | IitDescriptor::VarArg => Ok(void_type(module)),
        IitDescriptor::Mmx => Ok(fixed_vector_type(module, int_type(module, 64), 1)),
        IitDescriptor::Amx => Ok(Type::new(module.module().context().x86_amx(), module)),
        IitDescriptor::ExnRef => Ok(Type::new(module.module().context().wasm_exnref(), module)),
        IitDescriptor::Token => Ok(Type::new(module.module().context().token(), module)),
        IitDescriptor::Metadata => Ok(metadata_type(module)),
        IitDescriptor::Half => Ok(Type::new(module.module().context().half(), module)),
        IitDescriptor::BFloat => Ok(Type::new(module.module().context().bfloat(), module)),
        IitDescriptor::Float => Ok(Type::new(module.module().context().float(), module)),
        IitDescriptor::Double => Ok(Type::new(module.module().context().double(), module)),
        IitDescriptor::Quad => Ok(Type::new(module.module().context().fp128(), module)),
        IitDescriptor::PpcQuad => Ok(Type::new(module.module().context().ppc_fp128(), module)),
        IitDescriptor::AArch64Svcount => Ok(target_ext_type(module, "aarch64.svcount", [], [])),
        IitDescriptor::Integer(bits) => Ok(int_type(module, bits)),
        IitDescriptor::Pointer(addr_space) => Ok(ptr_type(module, addr_space)),
        IitDescriptor::Vector { width, scalable } => {
            let elem = decode_fixed_type(module, descriptors, overloads)?;
            if scalable {
                Ok(scalable_vector_type(module, elem, width))
            } else {
                Ok(fixed_vector_type(module, elem, width))
            }
        }
        IitDescriptor::Struct { elements } => {
            let mut fields = Vec::with_capacity(elements);
            for _ in 0..elements {
                fields.push(decode_fixed_type(module, descriptors, overloads)?.id());
            }
            Ok(Type::new(
                module
                    .module()
                    .context()
                    .literal_struct_type(fields.into_boxed_slice(), false),
                module,
            ))
        }
        IitDescriptor::Argument { index, kind } => {
            let ty = *overloads.get(index).ok_or_else(intrinsic_mismatch)?;
            validate_overload_kind(module, ty, kind)?;
            Ok(ty)
        }
        IitDescriptor::ExtendArgument(index) => {
            let ty = *overloads.get(index).ok_or_else(intrinsic_mismatch)?;
            extend_integer_type(module, ty)
        }
        IitDescriptor::TruncArgument(index) => {
            let ty = *overloads.get(index).ok_or_else(intrinsic_mismatch)?;
            trunc_argument_type(module, ty)
        }
        IitDescriptor::SameVecWidthArgument(index) => {
            let elem = decode_fixed_type(module, descriptors, overloads)?;
            let ty = *overloads.get(index).ok_or_else(intrinsic_mismatch)?;
            same_vector_width_type(module, ty, elem)
        }
        IitDescriptor::VecElementArgument(index) => {
            let ty = *overloads.get(index).ok_or_else(intrinsic_mismatch)?;
            vector_element_or_self(module, ty)
        }
        IitDescriptor::VecOfAnyPtrsToElt { overload, .. } => {
            let ty = *overloads.get(overload).ok_or_else(intrinsic_mismatch)?;
            require_pointer_vector(ty)?;
            Ok(ty)
        }
        IitDescriptor::OneNthEltsVecArgument { divisor, argument } => {
            let ty = *overloads.get(argument).ok_or_else(intrinsic_mismatch)?;
            one_nth_vector_type(module, ty, divisor)
        }
        IitDescriptor::Subdivide2Argument(index) => {
            let ty = *overloads.get(index).ok_or_else(intrinsic_mismatch)?;
            subdivide_vector_type(module, ty, 1)
        }
        IitDescriptor::Subdivide4Argument(index) => {
            let ty = *overloads.get(index).ok_or_else(intrinsic_mismatch)?;
            subdivide_vector_type(module, ty, 2)
        }
        IitDescriptor::VecOfBitcastsToInt(index) => {
            let ty = *overloads.get(index).ok_or_else(intrinsic_mismatch)?;
            vector_of_bitcasts_to_int(module, ty)
        }
    }
}

fn validate_overload_kind<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    ty: Type<'ctx, B>,
    kind: IitArgKind,
) -> IrResult<()>
where
    B: ModuleBrand + 'ctx,
{
    let ok = match kind {
        IitArgKind::Any | IitArgKind::MatchType => true,
        IitArgKind::AnyInteger => is_integer_or_integer_vector(module, ty),
        IitArgKind::AnyFloat => is_float_or_float_vector(module, ty),
        IitArgKind::AnyVector => is_vector(ty),
        IitArgKind::AnyPointer => matches!(ty.data(), TypeData::Pointer { .. }),
    };
    if ok {
        Ok(())
    } else {
        Err(intrinsic_mismatch())
    }
}

fn is_integer_or_integer_vector<'ctx, B>(module: ModuleRef<'ctx, B>, ty: Type<'ctx, B>) -> bool
where
    B: ModuleBrand + 'ctx,
{
    matches!(scalar_type_data(module, ty), TypeData::Integer { .. })
}

fn is_float_or_float_vector<'ctx, B>(module: ModuleRef<'ctx, B>, ty: Type<'ctx, B>) -> bool
where
    B: ModuleBrand + 'ctx,
{
    matches!(
        scalar_type_data(module, ty),
        TypeData::Half
            | TypeData::BFloat
            | TypeData::Float
            | TypeData::Double
            | TypeData::X86Fp80
            | TypeData::Fp128
            | TypeData::PpcFp128
    )
}

fn is_vector<'ctx, B>(ty: Type<'ctx, B>) -> bool
where
    B: ModuleBrand + 'ctx,
{
    matches!(
        ty.data(),
        TypeData::FixedVector { .. } | TypeData::ScalableVector { .. }
    )
}

fn scalar_type_data<'ctx, B>(module: ModuleRef<'ctx, B>, ty: Type<'ctx, B>) -> &'ctx TypeData
where
    B: ModuleBrand + 'ctx,
{
    match ty.data() {
        TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => {
            Type::new(*elem, module).data()
        }
        data => data,
    }
}

fn fn_type_with_var_arg<'ctx, B, I>(
    module: ModuleRef<'ctx, B>,
    ret: Type<'ctx, B>,
    params: I,
    is_var_arg: bool,
) -> IrResult<FunctionType<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
    I: IntoIterator<Item = Type<'ctx, B>>,
{
    let param_ids: Vec<_> = params.into_iter().map(Type::id).collect();
    let id =
        module
            .module()
            .context()
            .function_type(ret.id(), param_ids.into_boxed_slice(), is_var_arg);
    Ok(FunctionType::new(id, module))
}

fn scalable_vector_type<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    elem: Type<'ctx, B>,
    min: u32,
) -> Type<'ctx, B>
where
    B: ModuleBrand + 'ctx,
{
    Type::new(
        module
            .module()
            .context()
            .scalable_vector_type(elem.id(), min),
        module,
    )
}

fn array_type<'ctx, B>(module: ModuleRef<'ctx, B>, elem: Type<'ctx, B>, n: u64) -> Type<'ctx, B>
where
    B: ModuleBrand + 'ctx,
{
    Type::new(module.module().context().array_type(elem.id(), n), module)
}

fn target_ext_type<'ctx, B, Types, Ints>(
    module: ModuleRef<'ctx, B>,
    name: &str,
    type_params: Types,
    int_params: Ints,
) -> Type<'ctx, B>
where
    B: ModuleBrand + 'ctx,
    Types: IntoIterator<Item = Type<'ctx, B>>,
    Ints: IntoIterator<Item = u32>,
{
    let type_ids: Box<[_]> = type_params.into_iter().map(Type::id).collect();
    let int_params: Box<[_]> = int_params.into_iter().collect();
    Type::new(
        module
            .module()
            .context()
            .target_ext_type(name.to_owned(), type_ids, int_params),
        module,
    )
}

fn extend_integer_type<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    ty: Type<'ctx, B>,
) -> IrResult<Type<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
{
    match ty.data() {
        TypeData::Integer { bits } => bits
            .checked_mul(2)
            .map(|width| int_type(module, width))
            .ok_or_else(intrinsic_mismatch),
        TypeData::FixedVector { elem, n } => {
            let elem_ty = Type::new(*elem, module);
            Ok(fixed_vector_type(
                module,
                extend_integer_type(module, elem_ty)?,
                *n,
            ))
        }
        TypeData::ScalableVector { elem, min } => {
            let elem_ty = Type::new(*elem, module);
            Ok(scalable_vector_type(
                module,
                extend_integer_type(module, elem_ty)?,
                *min,
            ))
        }
        _ => Err(intrinsic_mismatch()),
    }
}

fn trunc_argument_type<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    ty: Type<'ctx, B>,
) -> IrResult<Type<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
{
    match ty.data() {
        TypeData::Integer { bits } if bits % 2 == 0 => Ok(int_type(module, bits / 2)),
        TypeData::FixedVector { elem, n } => {
            let elem_ty = Type::new(*elem, module);
            Ok(fixed_vector_type(
                module,
                truncated_vector_element_type(module, elem_ty)?,
                *n,
            ))
        }
        TypeData::ScalableVector { elem, min } => {
            let elem_ty = Type::new(*elem, module);
            Ok(scalable_vector_type(
                module,
                truncated_vector_element_type(module, elem_ty)?,
                *min,
            ))
        }
        _ => Err(intrinsic_mismatch()),
    }
}

fn same_vector_width_type<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    reference: Type<'ctx, B>,
    elem: Type<'ctx, B>,
) -> IrResult<Type<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
{
    match reference.data() {
        TypeData::FixedVector { n, .. } => Ok(fixed_vector_type(module, elem, *n)),
        TypeData::ScalableVector { min, .. } => Ok(scalable_vector_type(module, elem, *min)),
        _ => Ok(elem),
    }
}

fn vector_element_or_self<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    ty: Type<'ctx, B>,
) -> IrResult<Type<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
{
    match ty.data() {
        TypeData::FixedVector { elem, .. } | TypeData::ScalableVector { elem, .. } => {
            Ok(Type::new(*elem, module))
        }
        _ => Err(intrinsic_mismatch()),
    }
}

fn primitive_size_in_bits<'ctx, B>(module: ModuleRef<'ctx, B>, ty: Type<'ctx, B>) -> Option<u32>
where
    B: ModuleBrand + 'ctx,
{
    match ty.data() {
        TypeData::Integer { bits } => Some(*bits),
        TypeData::Half | TypeData::BFloat => Some(16),
        TypeData::Float => Some(32),
        TypeData::Double => Some(64),
        TypeData::X86Fp80 => Some(80),
        TypeData::PpcFp128 | TypeData::Fp128 => Some(128),
        TypeData::X86Amx => Some(8192),
        TypeData::FixedVector { elem, n } | TypeData::ScalableVector { elem, min: n } => {
            primitive_size_in_bits(module, Type::new(*elem, module))?.checked_mul(*n)
        }
        _ => None,
    }
}

fn integer_type_matching_primitive_size<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    ty: Type<'ctx, B>,
) -> IrResult<Type<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
{
    let Some(bits) = primitive_size_in_bits(module, ty) else {
        return Err(intrinsic_mismatch());
    };
    if bits == 0 {
        return Err(intrinsic_mismatch());
    }
    Ok(int_type(module, bits))
}

fn vector_of_bitcasts_to_int<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    ty: Type<'ctx, B>,
) -> IrResult<Type<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
{
    match ty.data() {
        TypeData::FixedVector { elem, n } => {
            let elem_ty = Type::new(*elem, module);
            Ok(fixed_vector_type(
                module,
                integer_type_matching_primitive_size(module, elem_ty)?,
                *n,
            ))
        }
        TypeData::ScalableVector { elem, min } => {
            let elem_ty = Type::new(*elem, module);
            Ok(scalable_vector_type(
                module,
                integer_type_matching_primitive_size(module, elem_ty)?,
                *min,
            ))
        }
        _ => Err(intrinsic_mismatch()),
    }
}

fn one_nth_vector_type<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    ty: Type<'ctx, B>,
    divisor: u32,
) -> IrResult<Type<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
{
    if divisor == 0 {
        return Err(intrinsic_mismatch());
    }
    match ty.data() {
        TypeData::FixedVector { elem, n } if n % divisor == 0 => Ok(fixed_vector_type(
            module,
            Type::new(*elem, module),
            n / divisor,
        )),
        TypeData::ScalableVector { elem, min } if min % divisor == 0 => Ok(scalable_vector_type(
            module,
            Type::new(*elem, module),
            min / divisor,
        )),
        _ => Err(intrinsic_mismatch()),
    }
}

fn truncated_vector_element_type<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    elem: Type<'ctx, B>,
) -> IrResult<Type<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
{
    match elem.data() {
        TypeData::Double => Ok(Type::new(module.module().context().float(), module)),
        TypeData::Float => Ok(Type::new(module.module().context().half(), module)),
        TypeData::Half
        | TypeData::BFloat
        | TypeData::X86Fp80
        | TypeData::PpcFp128
        | TypeData::Fp128 => Err(intrinsic_mismatch()),
        _ => {
            let Some(bits) = primitive_size_in_bits(module, elem) else {
                return Err(intrinsic_mismatch());
            };
            if bits == 0 || bits % 2 != 0 {
                return Err(intrinsic_mismatch());
            }
            Ok(int_type(module, bits / 2))
        }
    }
}

fn subdivide_vector_type<'ctx, B>(
    module: ModuleRef<'ctx, B>,
    ty: Type<'ctx, B>,
    subdivisions: u32,
) -> IrResult<Type<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
{
    let (mut elem, mut lanes, scalable) = match ty.data() {
        TypeData::FixedVector { elem, n } => (Type::new(*elem, module), *n, false),
        TypeData::ScalableVector { elem, min } => (Type::new(*elem, module), *min, true),
        _ => return Err(intrinsic_mismatch()),
    };
    for _ in 0..subdivisions {
        lanes = lanes.checked_mul(2).ok_or_else(intrinsic_mismatch)?;
        elem = truncated_vector_element_type(module, elem)?;
    }
    if scalable {
        Ok(scalable_vector_type(module, elem, lanes))
    } else {
        Ok(fixed_vector_type(module, elem, lanes))
    }
}

fn intrinsic_mismatch() -> IrError {
    IrError::InvalidOperation {
        message: "intrinsic signature mismatch",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_type<'ctx, B>(
        module: ModuleRef<'ctx, B>,
        sample: &IntrinsicSampleType,
    ) -> Type<'ctx, B>
    where
        B: ModuleBrand + 'ctx,
    {
        match sample {
            IntrinsicSampleType::Int(bits) => int_type(module, *bits),
            IntrinsicSampleType::Float(name) => match *name {
                "f16" => Type::new(module.module().context().half(), module),
                "bf16" => Type::new(module.module().context().bfloat(), module),
                "f32" => Type::new(module.module().context().float(), module),
                "f64" => Type::new(module.module().context().double(), module),
                "f80" => Type::new(module.module().context().x86_fp80(), module),
                "f128" => Type::new(module.module().context().fp128(), module),
                "ppcf128" => Type::new(module.module().context().ppc_fp128(), module),
                other => panic!("unsupported generated sample float `{other}`"),
            },
            IntrinsicSampleType::Pointer(addr_space) => ptr_type(module, *addr_space),
            IntrinsicSampleType::FixedVector { lanes, element } => {
                fixed_vector_type(module, sample_type(module, element), *lanes)
            }
        }
    }

    fn preview_entries(entries: &[u8]) -> &[u8] {
        let preview_len = entries.len().min(32);
        &entries[..preview_len]
    }

    /// Mirrors `llvm/lib/IR/Intrinsics.cpp::matchIntrinsicType`:
    /// `IITDescriptor::VecElementArgument` compares against the element type
    /// for vector overloads and rejects scalar overloads.
    #[test]
    fn vec_element_argument_requires_vector_overload() -> IrResult<()> {
        Module::with_new("intrinsic-vec-element", |module| {
            let module_ref = module.module_ref();
            let i32_ty = module.i32_type().as_type();
            let vector_ty = fixed_vector_type(module_ref, i32_ty, 4);

            assert_eq!(vector_element_or_self(module_ref, vector_ty)?, i32_ty);
            assert!(vector_element_or_self(module_ref, i32_ty).is_err());
            Ok(())
        })
    }

    /// Mirrors `llvm/include/llvm/IR/DerivedTypes.h::getSubdividedVectorType`:
    /// subdivision doubles the lane count while halving the element bit width.
    #[test]
    fn subdivide_argument_halves_element_width_and_doubles_lanes() -> IrResult<()> {
        Module::with_new("intrinsic-subdivide", |module| {
            let module_ref = module.module_ref();
            let vector_ty = fixed_vector_type(module_ref, module.i64_type().as_type(), 4);
            let subdivided_once = subdivide_vector_type(module_ref, vector_ty, 1)?;
            let subdivided_twice = subdivide_vector_type(module_ref, vector_ty, 2)?;

            assert_eq!(format!("{subdivided_once}"), "<8 x i32>");
            assert_eq!(format!("{subdivided_twice}"), "<16 x i16>");
            Ok(())
        })
    }

    /// Mirrors `llvm/lib/IR/Intrinsics.cpp::matchIntrinsicType`:
    /// `IITDescriptor::Subdivide2Argument` applies one subdivision and
    /// `IITDescriptor::Subdivide4Argument` applies two subdivisions.
    #[test]
    fn subdivide_argument_matcher_uses_llvm_subdivision_counts() -> IrResult<()> {
        Module::with_new("intrinsic-subdivide-match", |module| {
            let module_ref = module.module_ref();
            let vector_ty = fixed_vector_type(module_ref, module.i64_type().as_type(), 4);
            let mut overloads = vec![Some(vector_ty)];

            let subdivide2 = [IitDescriptor::Subdivide2Argument(0)];
            let mut subdivide2_descriptors = subdivide2.as_slice();
            match_fixed_type(
                module_ref,
                &mut subdivide2_descriptors,
                &mut overloads,
                fixed_vector_type(module_ref, module.i32_type().as_type(), 8),
            )?;

            let subdivide4 = [IitDescriptor::Subdivide4Argument(0)];
            let mut subdivide4_descriptors = subdivide4.as_slice();
            match_fixed_type(
                module_ref,
                &mut subdivide4_descriptors,
                &mut overloads,
                fixed_vector_type(module_ref, module.i16_type().as_type(), 16),
            )?;
            Ok(())
        })
    }

    /// Mirrors `llvm/include/llvm/IR/DerivedTypes.h::getInteger`:
    /// `IITDescriptor::VecOfBitcastsToInt` preserves vector shape and replaces
    /// each element with an integer of the same primitive bit width.
    #[test]
    fn vec_of_bitcasts_to_int_preserves_shape_and_integerizes_element() -> IrResult<()> {
        Module::with_new("intrinsic-vector-bitcast-int", |module| {
            let module_ref = module.module_ref();
            let f32_vec = fixed_vector_type(module_ref, module.f32_type().as_type(), 4);
            let i32_vec = vector_of_bitcasts_to_int(module_ref, f32_vec)?;

            assert_eq!(format!("{i32_vec}"), "<4 x i32>");
            assert!(vector_of_bitcasts_to_int(module_ref, module.f32_type().as_type()).is_err());
            Ok(())
        })
    }

    /// Mirrors `llvm/lib/IR/Intrinsics.cpp::matchIntrinsicType`:
    /// `IITDescriptor::VecOfBitcastsToInt` compares against
    /// `VectorType::getInteger` for the referenced vector overload.
    #[test]
    fn vec_of_bitcasts_to_int_matcher_compares_integer_vector() -> IrResult<()> {
        Module::with_new("intrinsic-vector-bitcast-int-match", |module| {
            let module_ref = module.module_ref();
            let f32_vec = fixed_vector_type(module_ref, module.f32_type().as_type(), 4);
            let i32_vec = fixed_vector_type(module_ref, module.i32_type().as_type(), 4);
            let mut overloads = vec![Some(f32_vec)];

            let descriptors = [IitDescriptor::VecOfBitcastsToInt(0)];
            let mut descriptors = descriptors.as_slice();
            match_fixed_type(module_ref, &mut descriptors, &mut overloads, i32_vec)?;
            Ok(())
        })
    }

    /// Mirrors `llvm/lib/IR/Intrinsics.cpp::getIntrinsicInfoTableEntries`:
    /// every generated intrinsic ID has decodable IIT entries, and every
    /// non-overloaded intrinsic can materialize a concrete function type.
    #[test]
    fn all_generated_intrinsics_decode_iit_entries() -> IrResult<()> {
        Module::with_new("generated-all", |module| {
            for id in IntrinsicId::all() {
                let raw_entries = iit_entries(id.record()).unwrap_or_else(|| {
                    panic!("{}#{} has no IIT entries", id.enum_name(), id.raw())
                });
                let descriptors = iit_descriptors(id.record()).unwrap_or_else(|err| {
                    panic!(
                        "{}#{} descriptor decode failed for entries {:?}: {err}",
                        id.enum_name(),
                        id.raw(),
                        preview_entries(raw_entries.as_ref())
                    )
                });
                if !id.is_overloaded() {
                    let descriptor = IntrinsicDescriptor::new(id, Vec::<Type>::new())
                        .unwrap_or_else(|err| {
                            panic!("{}#{} descriptor failed: {err}", id.enum_name(), id.raw())
                        });
                    descriptor.function_type(&module).unwrap_or_else(|err| {
                        panic!(
                            "{}#{} function type failed for descriptors {:?}: {err}",
                            id.enum_name(),
                            id.raw(),
                            descriptors
                        )
                    });
                }
            }
            Ok(())
        })
    }

    /// Mirrors `llvm/lib/IR/Intrinsics.cpp::lookupIntrinsicID`,
    /// `llvm/lib/IR/Intrinsics.cpp::getIntrinsicInfoTableEntries`, and
    /// `llvm/utils/TableGen/Basic/IntrinsicEmitter.cpp`: every generated
    /// intrinsic ID resolves from its base name, every fixed-signature intrinsic
    /// can be declared, and every generated sample overload can be declared.
    #[test]
    fn generated_all_intrinsic_names_lookup_and_decode() -> IrResult<()> {
        Module::with_new("generated-all-names", |module| {
            for id in IntrinsicId::all() {
                let resolved = resolve_intrinsic_name(id.base_name());
                assert_eq!(
                    resolved,
                    IntrinsicNameResolution::Known(id),
                    "{}#{} base name `{}` resolved as {resolved:?}",
                    id.enum_name(),
                    id.raw(),
                    id.base_name()
                );

                if !id.is_overloaded() {
                    let descriptor = IntrinsicDescriptor::new(id, Vec::<Type>::new())
                        .unwrap_or_else(|err| {
                            panic!("{}#{} descriptor failed: {err}", id.enum_name(), id.raw())
                        });
                    descriptor.function_type(&module).unwrap_or_else(|err| {
                        panic!(
                            "{}#{} function type failed before declaration insertion: {err}",
                            id.enum_name(),
                            id.raw()
                        )
                    });
                    module
                        .get_or_insert_intrinsic_declaration(&descriptor)
                        .unwrap_or_else(|err| {
                            panic!(
                                "{}#{} declaration insertion failed: {err}",
                                id.enum_name(),
                                id.raw()
                            )
                        });
                }
            }

            for sample in generated::SAMPLE_OVERLOADS {
                let id = IntrinsicId::from_raw(sample.raw_id).ok_or(IrError::InvalidOperation {
                    message: "generated sample intrinsic id is out of range",
                })?;
                let overloads = sample
                    .overloads
                    .iter()
                    .map(|sample| sample_type(module.module_ref(), sample))
                    .collect::<Vec<_>>();
                let descriptor = IntrinsicDescriptor::new(id, overloads).unwrap_or_else(|err| {
                    panic!(
                        "{}#{} sample descriptor failed for {} overload(s): {err}",
                        id.enum_name(),
                        id.raw(),
                        sample.overloads.len()
                    )
                });
                descriptor.function_type(&module).unwrap_or_else(|err| {
                    panic!(
                        "{}#{} sample function type failed before declaration insertion: {err}",
                        id.enum_name(),
                        id.raw()
                    )
                });
                module
                    .get_or_insert_intrinsic_declaration(&descriptor)
                    .unwrap_or_else(|err| {
                        panic!(
                            "{}#{} sample declaration insertion failed: {err}",
                            id.enum_name(),
                            id.raw()
                        )
                    });
            }

            module.verify_borrowed()
        })
    }

    /// Mirrors `llvm/utils/TableGen/Basic/IntrinsicEmitter.cpp` sample overload
    /// emission and `llvm/lib/IR/Intrinsics.cpp::getIntrinsicInfoTableEntries`:
    /// every generated sample overload decodes, mangles, and matches back to the
    /// same intrinsic descriptor.
    #[test]
    fn generated_sample_overloads_decode_and_match() -> IrResult<()> {
        Module::with_new("generated-samples", |module| {
            assert!(!generated::SAMPLE_OVERLOADS.is_empty());
            for sample in generated::SAMPLE_OVERLOADS {
                let id = IntrinsicId::from_raw(sample.raw_id).ok_or(IrError::InvalidOperation {
                    message: "generated sample intrinsic id is out of range",
                })?;
                let overloads = sample
                    .overloads
                    .iter()
                    .map(|sample| sample_type(module.module_ref(), sample))
                    .collect::<Vec<_>>();
                let raw_entries = iit_entries(id.record()).unwrap_or_else(|| {
                    panic!("{}#{} has no IIT entries", id.enum_name(), id.raw())
                });
                let descriptors = iit_descriptors(id.record()).unwrap_or_else(|err| {
                    panic!(
                        "{}#{} descriptor decode failed for entries {:?}: {err}",
                        id.enum_name(),
                        id.raw(),
                        preview_entries(raw_entries.as_ref())
                    )
                });
                let expected_slots = overload_slot_count(&descriptors);
                let descriptor = IntrinsicDescriptor::new(id, overloads).unwrap_or_else(|err| {
                    panic!(
                        "{}#{} descriptor failed for {} sample overload(s), expected {expected_slots}: {err}",
                        id.enum_name(),
                        id.raw(),
                        sample.overloads.len()
                    )
                });
                let fn_ty = descriptor.function_type(&module).unwrap_or_else(|err| {
                    panic!(
                        "{}#{} function type failed for entries {:?} and descriptors {:?}: {err}",
                        id.enum_name(),
                        id.raw(),
                        preview_entries(raw_entries.as_ref()),
                        descriptors
                    )
                });
                let name = descriptor
                    .mangled_name()
                    .unwrap_or_else(|err| panic!("{} mangled name failed: {err}", id.enum_name()));
                let matched = module
                    .intrinsic_descriptor_from_signature(&name, fn_ty)
                    .unwrap_or_else(|err| panic!("{name} signature match failed: {err}"));
                assert_eq!(matched, descriptor);
            }
            Ok(())
        })
    }
}
