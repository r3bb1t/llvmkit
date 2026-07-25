//! Storable, module-tagged value ids and the [`Module`](crate::Module)
//! view-minting API (llvmkit 2.0, cycle A).
//!
//! Every lifetime-borrowing value handle in this crate ([`Value`],
//! [`IntValue`], [`FunctionValue`],
//! ...) borrows its owning module for `'ctx` and so cannot outlive it or be
//! stored past the construction closure. This module introduces the *dual*
//! representation: a family of minimal, `Copy`, lifetime-free **ids** that a
//! caller can store freely, plus [`Module::view`](crate::Module::view) /
//! [`Module::try_view`](crate::Module::try_view) to resolve an id back into a
//! borrowing handle.
//!
//! Cycle A was purely **additive**: every handle gained a
//! [`id`](crate::Value::id)-style accessor that mints its id, while the
//! builders kept returning borrowing handles. Cycle B rewires the builders to
//! return ids, one family at a time — arithmetic, casts, comparisons, memory,
//! aggregates, vectors, calls, the module-level declarations, blocks/branches
//! and phi have all flipped, so no builder hands back a borrowing handle any
//! more. [`IRBuilder::view`](crate::IRBuilder::view) is the builder-side twin
//! of [`Module::view`](crate::Module::view) for reading at a build site.
//!
//! One naming rule holds across the whole surface: `handle.id()` mints the
//! storable, module-tagged id, and `handle.slot()` is the bare arena index
//! (crate-internal side tables only).
//!
//! Two shapes of id live here: the **value ids** ([`ValueId`], [`IntValueId`],
//! ...), which name a value and nothing more, and the **instruction ids**
//! ([`CallInstId`], [`AtomicRMWInstId`], ...), whose [`ViewIn::View`] is an
//! opcode handle so that a builder returning one keeps the opcode's typed API
//! reachable through a single view.
//!
//! # Representation
//!
//! An id is `{ tag: ModuleId, slot: ValueSlot, <type markers>, _brand }`. It
//! carries **no** cached IR type — the type is recovered from the arena when
//! the id is viewed, keeping the id as small as a tagged index. The `tag` is
//! the process-unique [`ModuleId`] of the owning module;
//! [`Module::view`](crate::Module::view) checks it before touching the arena
//! so an id from a foreign module can never mis-resolve against an in-range
//! slot.
//!
//! The `_brand` phantom is always `Invariant<B>` (`PhantomData<fn(B) -> B>`):
//! `Send`-neutral and invariant in `B`, exactly like the borrowing handles.
//! During cycle A `B` is the generative lifetime brand `Brand<'brand>`, so
//! the ids are storable *within* the `with_new` closure; they become `'static`
//! automatically in cycle C when brands become types (hence no `'static` bound
//! is imposed here).

use core::marker::PhantomData;

use crate::basic_block::BasicBlockLabel;
use crate::block_params::{BlockParams, BlockParamsDyn};
use crate::error::{IrError, IrResult};
use crate::float_kind::{FloatKind, IntoFloatValue, into_float_value_sealed};
use crate::function::{FunctionValue, signature_matches_marker};
use crate::function_signature::{
    FunctionParamList, FunctionReturn, TypedFunctionValue, TypedVarArgsFunctionValue,
};
use crate::global_alias::GlobalAlias;
use crate::global_ifunc::GlobalIFunc;
use crate::global_variable::GlobalVariable;
use crate::instruction::InstructionKindData;
use crate::instructions::{
    AtomicCmpXchgInst, AtomicRMWInst, CallInst, FpPhiInst, FreezeInst, OtherPhiInst, PhiInst,
    PointerPhiInst, TypedCallInst, VAArgInst,
};
use crate::int_width::{IntWidth, IntoIntValue, into_int_value_sealed};
use crate::intrinsic_inst::IntrinsicInst;
use crate::marker::ReturnMarker;
use crate::module::{Invariant, ModuleBrand, ModuleId, ModuleRef};
use crate::r#type::{Type, TypeData, TypeSlot};
use crate::value::{
    FloatValue, IntValue, IntoErasedValue, IntoPointerValue, IsValue, PointerValue, Value,
    ValueKindData, ValueSlot, into_erased_value_sealed, into_pointer_value_sealed,
};

// --------------------------------------------------------------------------
// The id family
// --------------------------------------------------------------------------

/// Declare a minimal, `Copy` value id with an optional list of leading type
/// markers (before the always-present brand `B`). Generates the struct plus
/// manual `Copy`/`Clone`/`Eq`/`PartialEq`/`Hash`/`Debug` impls — manual
/// because a `derive` would propagate a `Marker: Trait` bound onto the impl
/// that callers should never have to spell (the `FunctionValue` precedent),
/// and because `Debug` must print `tag`/`slot` only, never the phantoms.
macro_rules! decl_value_id {
    (
        $(#[$attr:meta])*
        $name:ident $([$($mk:ident : $mkb:path => $mf:ident),+ $(,)?])?
    ) => {
        $(#[$attr])*
        pub struct $name<$($($mk: $mkb,)+)? B: ModuleBrand> {
            tag: ModuleId,
            slot: ValueSlot,
            $($($mf: PhantomData<$mk>,)+)?
            _brand: Invariant<B>,
        }

        impl<$($($mk: $mkb,)+)? B: ModuleBrand> $name<$($($mk,)+)? B> {
            /// Crate-internal: mint an id from an already-resolved tag + slot.
            /// The only callers are the value handles' `id` accessors, which
            /// pass their owning [`ModuleId`] and arena slot.
            #[inline]
            pub(crate) fn from_raw(tag: ModuleId, slot: ValueSlot) -> Self {
                Self {
                    tag,
                    slot,
                    $($($mf: PhantomData,)+)?
                    _brand: PhantomData,
                }
            }
        }

        impl<$($($mk: $mkb,)+)? B: ModuleBrand> Clone for $name<$($($mk,)+)? B> {
            #[inline]
            fn clone(&self) -> Self {
                *self
            }
        }
        impl<$($($mk: $mkb,)+)? B: ModuleBrand> Copy for $name<$($($mk,)+)? B> {}
        impl<$($($mk: $mkb,)+)? B: ModuleBrand> PartialEq for $name<$($($mk,)+)? B> {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                self.tag == other.tag && self.slot == other.slot
            }
        }
        impl<$($($mk: $mkb,)+)? B: ModuleBrand> Eq for $name<$($($mk,)+)? B> {}
        impl<$($($mk: $mkb,)+)? B: ModuleBrand> core::hash::Hash for $name<$($($mk,)+)? B> {
            #[inline]
            fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
                self.tag.hash(state);
                self.slot.hash(state);
            }
        }
        impl<$($($mk: $mkb,)+)? B: ModuleBrand> core::fmt::Debug for $name<$($($mk,)+)? B> {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.debug_struct(stringify!($name))
                    .field("tag", &self.tag)
                    .field("slot", &self.slot)
                    .finish()
            }
        }
    };
}

decl_value_id! {
    /// Storable, module-tagged id for any IR value — the erased id, resolved
    /// by [`Module::view`](crate::Module::view) back into a [`Value`].
    ///
    /// The id analogue of the erased [`Value`] handle: it carries no type
    /// marker, so [`Module::view`](crate::Module::view) recovers the value's
    /// cached type from the arena when minting the handle.
    ValueId
}

decl_value_id! {
    /// Storable, module-tagged id for an `iN` value, resolved into an
    /// [`IntValue<W>`](crate::IntValue). The width marker `W` is preserved on
    /// the id and re-attached on view.
    IntValueId [W: IntWidth => _w]
}

decl_value_id! {
    /// Storable, module-tagged id for a floating-point value, resolved into a
    /// [`FloatValue<K>`](crate::FloatValue). The float-kind marker `K` is
    /// preserved on the id and re-attached on view.
    FloatValueId [K: FloatKind => _k]
}

decl_value_id! {
    /// Storable, module-tagged id for a pointer value, resolved into a
    /// [`PointerValue`].
    PointerValueId
}

decl_value_id! {
    /// Storable, module-tagged id for a function value, resolved into a
    /// [`FunctionValue<R>`](crate::FunctionValue). The return-shape marker `R`
    /// is preserved on the id; the signature is recovered from the arena on
    /// view.
    FunctionId [R: ReturnMarker => _r]
}

decl_value_id! {
    /// Storable, module-tagged id for a function whose return *and* parameter
    /// schema are known statically, resolved into a
    /// [`TypedFunctionValue<Ret, Params>`](crate::TypedFunctionValue).
    ///
    /// The full schema — not just the return marker — rides on the id, the way
    /// [`TypedCallInstId`] carries its `Ret`, so viewing it recovers the
    /// infallible typed surface ([`TypedFunctionValue::params`],
    /// [`TypedFunctionValue::as_function`]) without a re-narrowing step.
    TypedFunctionId [Ret: FunctionReturn => _ret, Params: FunctionParamList => _params]
}

decl_value_id! {
    /// Storable, module-tagged id for a variadic function whose fixed-prefix
    /// schema is known statically, resolved into a
    /// [`TypedVarArgsFunctionValue<Ret, Params>`](crate::TypedVarArgsFunctionValue).
    /// The variadic twin of [`TypedFunctionId`].
    TypedVarArgsFunctionId [Ret: FunctionReturn => _ret, Params: FunctionParamList => _params]
}

decl_value_id! {
    /// Storable, module-tagged id for a module-level global variable, resolved
    /// into a [`GlobalVariable`].
    GlobalId
}

decl_value_id! {
    /// Storable, module-tagged id for a module-level global alias, resolved
    /// into a [`GlobalAlias`].
    GlobalAliasId
}

decl_value_id! {
    /// Storable, module-tagged id for a module-level `ifunc`, resolved into a
    /// [`GlobalIFunc`].
    GlobalIFuncId
}

/// Storable, module-tagged id for a basic block, resolved into a copyable
/// [`BasicBlockLabel<R, B, Params>`](crate::BasicBlockLabel) (never the linear
/// [`BasicBlock`](crate::BasicBlock)).
///
/// Hand-written rather than macro-generated because — unlike the other ids —
/// its parameter marker `Params` follows the brand `B` in the generic list (to
/// carry the `= BlockParamsDyn` default in trailing position), matching the
/// handle it resolves to.
pub struct BlockId<R: ReturnMarker, B: ModuleBrand, Params: BlockParams = BlockParamsDyn> {
    tag: ModuleId,
    slot: ValueSlot,
    _r: PhantomData<R>,
    _params: PhantomData<Params>,
    _brand: Invariant<B>,
}

impl<R: ReturnMarker, B: ModuleBrand, Params: BlockParams> BlockId<R, B, Params> {
    /// Crate-internal: mint a block id from an already-resolved tag + slot.
    #[inline]
    pub(crate) fn from_raw(tag: ModuleId, slot: ValueSlot) -> Self {
        Self {
            tag,
            slot,
            _r: PhantomData,
            _params: PhantomData,
            _brand: PhantomData,
        }
    }

    /// Crate-internal: the arena slot this id names, **without** the module-tag
    /// check [`ViewIn::resolve_in`] performs. Reserved for the two places that
    /// key raw slot maps and have no [`ModuleRef`] in hand — the dominator
    /// tree's block ids and the Braun SSA engine's block-keyed maps — both of
    /// which were already slot-keyed and unchecked before ids existed. Every
    /// other consumer resolves through [`ViewIn`] so a foreign id is rejected
    /// before the arena is touched.
    #[inline]
    pub(crate) fn slot(self) -> ValueSlot {
        self.slot
    }

    /// Crate-internal: drop the typed parameter marker, yielding the
    /// parameter-erased ([`BlockParamsDyn`]) id. The typed branch builders
    /// lower a [`BlockCall`](crate::BlockCall) to this erased form before
    /// reusing the erased phi-seeding path, mirroring
    /// `BasicBlockLabel::erase_params`.
    #[inline]
    pub(crate) fn erase_params(self) -> BlockId<R, B> {
        BlockId::from_raw(self.tag, self.slot)
    }
}

impl<R: ReturnMarker, B: ModuleBrand, Params: BlockParams> Clone for BlockId<R, B, Params> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<R: ReturnMarker, B: ModuleBrand, Params: BlockParams> Copy for BlockId<R, B, Params> {}
impl<R: ReturnMarker, B: ModuleBrand, Params: BlockParams> PartialEq for BlockId<R, B, Params> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.tag == other.tag && self.slot == other.slot
    }
}
impl<R: ReturnMarker, B: ModuleBrand, Params: BlockParams> Eq for BlockId<R, B, Params> {}
impl<R: ReturnMarker, B: ModuleBrand, Params: BlockParams> core::hash::Hash
    for BlockId<R, B, Params>
{
    #[inline]
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.tag.hash(state);
        self.slot.hash(state);
    }
}
impl<R: ReturnMarker, B: ModuleBrand, Params: BlockParams> core::fmt::Debug
    for BlockId<R, B, Params>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BlockId")
            .field("tag", &self.tag)
            .field("slot", &self.slot)
            .finish()
    }
}

// --------------------------------------------------------------------------
// The instruction-id family
// --------------------------------------------------------------------------
//
// Most builders hand back a *value*, and the ids above say everything there is
// to say about one. A few builders instead hand back an **opcode handle** that
// carries its own typed API — [`CallInst::classify_callee`] /
// `return_int_value`, [`TypedCallInst::result`], [`AtomicRMWInst::operation`],
// ... Collapsing those to the erased [`ValueId`] would throw that API away *at
// the return position*, and recovering it would cost a view plus an
// [`InstructionView`](crate::InstructionView) narrowing plus a `kind()` match.
//
// So each such opcode handle gets its own id, exactly the move
// [`FunctionId`] (`View = FunctionValue`) and [`BlockId`]
// (`View = BasicBlockLabel`) already make: a read becomes
// `b.view(call).return_int_value()` — one view, which a read needed anyway —
// and the typed opcode API survives intact.
//
// Each resolver checks the arena's *instruction kind* before minting, the same
// way [`GlobalId`] / [`BlockId`] check their value category: the opcode handles
// reach their payload through an `unreachable!` on a wrong kind, so the kind
// check is what keeps a foreign or repurposed slot from turning into a panic.
// The result type is recovered from the arena rather than cached on the id,
// mirroring [`FunctionId`]'s signature recovery.

decl_value_id! {
    /// Storable, module-tagged id for a `call` instruction, resolved into a
    /// [`CallInst<R>`](crate::CallInst). The return-shape marker `R` is
    /// preserved on the id, so viewing it recovers the marker-gated
    /// `return_int_value` / `return_float_value` / `return_pointer_value`
    /// accessors; the call's result type is recovered from the arena.
    ///
    /// Deliberately **not** an [`IntoErasedValue`] operand: a call may be
    /// void, so its result is reached through
    /// [`CallInst::return_value`](crate::CallInst::return_value) or a
    /// marker-gated typed accessor — never by widening the instruction itself.
    CallInstId [R: ReturnMarker => _r]
}

decl_value_id! {
    /// Storable, module-tagged id for a schema-typed `call` instruction,
    /// resolved into a [`TypedCallInst<Ret>`](crate::TypedCallInst). The full
    /// return schema `Ret` — not just its marker — rides on the id, so viewing
    /// it recovers the infallible [`TypedCallInst::result`] whose type is
    /// `Ret::CallResult`.
    TypedCallInstId [Ret: FunctionReturn => _ret]
}

decl_value_id! {
    /// Storable, module-tagged id for a call to a generated intrinsic
    /// declaration, resolved into an [`IntrinsicInst<R>`](crate::IntrinsicInst).
    /// The intrinsic identity is re-derived from the callee on view, the way
    /// [`IntrinsicInst::from_call`] derives it, so the id stays a tagged index.
    IntrinsicInstId [R: ReturnMarker => _r]
}

decl_value_id! {
    /// Storable, module-tagged id for a `freeze` instruction, resolved into a
    /// [`FreezeInst`].
    FreezeInstId
}

decl_value_id! {
    /// Storable, module-tagged id for a `va_arg` instruction, resolved into a
    /// [`VAArgInst`].
    VAArgInstId
}

decl_value_id! {
    /// Storable, module-tagged id for an `atomicrmw` instruction, resolved into
    /// an [`AtomicRMWInst`].
    AtomicRMWInstId
}

decl_value_id! {
    /// Storable, module-tagged id for a `cmpxchg` instruction, resolved into an
    /// [`AtomicCmpXchgInst`].
    AtomicCmpXchgInstId
}

decl_value_id! {
    /// Storable, module-tagged id for an integer-typed `phi`, resolved into a
    /// [`PhiInst<W>`](crate::PhiInst). The width marker `W` is preserved on the
    /// id and re-attached on view, so the typed phi surface
    /// ([`PhiInst::as_int_value`](crate::PhiInst::as_int_value),
    /// [`PhiInst::incomings`](crate::PhiInst::incomings),
    /// [`PhiInst::remove_incoming`](crate::PhiInst::remove_incoming)) survives a
    /// single [`view`](crate::IRBuilder::view).
    PhiInstId [W: IntWidth => _w]
}

decl_value_id! {
    /// Storable, module-tagged id for a floating-point `phi`, resolved into an
    /// [`FpPhiInst<K>`](crate::FpPhiInst). The float-kind marker `K` is
    /// preserved on the id and re-attached on view.
    FpPhiInstId [K: FloatKind => _k]
}

decl_value_id! {
    /// Storable, module-tagged id for a pointer-typed `phi`, resolved into a
    /// [`PointerPhiInst`].
    PointerPhiInstId
}

decl_value_id! {
    /// Storable, module-tagged id for a `phi` whose result type is neither
    /// integer, float, nor pointer (vector / array / struct), resolved into an
    /// [`OtherPhiInst`] — the erased phi handle
    /// [`PhiKind::Other`](crate::PhiKind) also surfaces.
    OtherPhiInstId
}

// --------------------------------------------------------------------------
// ViewIn: the resolution boundary
// --------------------------------------------------------------------------

mod sealed {
    pub trait Sealed {}
}

/// Resolve a value id back into its borrowing handle within a module.
///
/// This is the shared machinery behind [`Module::view`](crate::Module::view)
/// and [`Module::try_view`](crate::Module::try_view): each id type maps to the
/// handle type it mints via [`ViewIn::View`], and [`resolve_in`](ViewIn::resolve_in)
/// performs the module-tag check (the choke point that keeps a foreign id from
/// mis-resolving) before recovering the handle from the arena.
///
/// The trait is **sealed** — the closed set of id types is part of the IR
/// vocabulary, not an extension point. It is `'ctx`-parameterised (rather than
/// using a GAT) so [`View`](ViewIn::View) can name a `'ctx`-borrowing handle
/// while the id itself stays lifetime-free.
pub trait ViewIn<'ctx, B: ModuleBrand>: Copy + sealed::Sealed {
    /// The borrowing handle this id resolves into.
    type View;

    /// Resolve against `module`, yielding `Some(handle)` when the id's tag
    /// matches `module` and its slot is present, else `None`. Callers should
    /// prefer [`Module::view`](crate::Module::view) /
    /// [`Module::try_view`](crate::Module::try_view).
    #[doc(hidden)]
    fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View>;
}

impl<B: ModuleBrand> sealed::Sealed for ValueId<B> {}
impl<'ctx, B: ModuleBrand + 'ctx> ViewIn<'ctx, B> for ValueId<B> {
    type View = Value<'ctx, B>;

    #[inline]
    fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View> {
        if self.tag != module.id() {
            return None;
        }
        let ty = module.value_data(self.slot).ty;
        Some(Value::from_parts(self.slot, module, ty))
    }
}

impl<W: IntWidth, B: ModuleBrand> sealed::Sealed for IntValueId<W, B> {}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> ViewIn<'ctx, B> for IntValueId<W, B> {
    type View = IntValue<'ctx, W, B>;

    #[inline]
    fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View> {
        if self.tag != module.id() {
            return None;
        }
        let ty = module.value_data(self.slot).ty;
        debug_assert!(
            matches!(
                module.type_data(ty),
                TypeData::Integer { bits } if W::static_bits().is_none_or(|w| w == *bits)
            ),
            "IntValueId width marker does not match the arena type at its slot",
        );
        Some(IntValue::from_value_unchecked(Value::from_parts(
            self.slot, module, ty,
        )))
    }
}

impl<K: FloatKind, B: ModuleBrand> sealed::Sealed for FloatValueId<K, B> {}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> ViewIn<'ctx, B> for FloatValueId<K, B> {
    type View = FloatValue<'ctx, K, B>;

    #[inline]
    fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View> {
        if self.tag != module.id() {
            return None;
        }
        let ty = module.value_data(self.slot).ty;
        debug_assert!(
            matches!(
                module.type_data(ty),
                TypeData::Half
                    | TypeData::BFloat
                    | TypeData::Float
                    | TypeData::Double
                    | TypeData::X86Fp80
                    | TypeData::Fp128
                    | TypeData::PpcFp128
            ),
            "FloatValueId kind marker does not match the arena type at its slot",
        );
        Some(FloatValue::from_value_unchecked(Value::from_parts(
            self.slot, module, ty,
        )))
    }
}

impl<B: ModuleBrand> sealed::Sealed for PointerValueId<B> {}
impl<'ctx, B: ModuleBrand + 'ctx> ViewIn<'ctx, B> for PointerValueId<B> {
    type View = PointerValue<'ctx, B>;

    #[inline]
    fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View> {
        if self.tag != module.id() {
            return None;
        }
        let ty = module.value_data(self.slot).ty;
        debug_assert!(
            matches!(module.type_data(ty), TypeData::Pointer { .. }),
            "PointerValueId points at a non-pointer arena type at its slot",
        );
        Some(PointerValue::from_value_unchecked(Value::from_parts(
            self.slot, module, ty,
        )))
    }
}

impl<R: ReturnMarker, B: ModuleBrand> sealed::Sealed for FunctionId<R, B> {}
impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> ViewIn<'ctx, B> for FunctionId<R, B> {
    type View = FunctionValue<'ctx, R, B>;

    #[inline]
    fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View> {
        if self.tag != module.id() {
            return None;
        }
        let signature = match &module.value_data(self.slot).kind {
            ValueKindData::Function(f) => f.signature,
            _ => return None,
        };
        debug_assert!(
            module
                .type_data(signature)
                .as_function()
                .is_some_and(|(ret, ..)| signature_matches_marker::<R>(module.type_data(ret))),
            "FunctionId return marker does not match the arena signature at its slot",
        );
        Some(FunctionValue::from_parts_unchecked(self.slot, module))
    }
}

/// Resolve a *typed function facade* id: reuse [`FunctionId`]'s resolver for
/// the tag check, the `Function` value-category check and the signature
/// recovery, then re-validate the full schema through the facade's own
/// `try_from_function`. Shared by [`TypedFunctionId`] and
/// [`TypedVarArgsFunctionId`], which differ only in that facade — and it is
/// the facade check that separates them (fixed-arity vs `...`), so it cannot
/// be skipped.
macro_rules! impl_view_in_for_typed_function_id {
    ($( $name:ident => $facade:ident ),+ $(,)?) => { $(
        impl<Ret: FunctionReturn, Params: FunctionParamList, B: ModuleBrand> sealed::Sealed
            for $name<Ret, Params, B>
        {
        }
        impl<'ctx, Ret, Params, B> ViewIn<'ctx, B> for $name<Ret, Params, B>
        where
            Ret: FunctionReturn,
            Params: FunctionParamList,
            B: ModuleBrand + 'ctx,
        {
            type View = $facade<'ctx, Ret, Params, B>;

            #[inline]
            fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View> {
                let function =
                    FunctionId::<Ret::Marker, B>::from_raw(self.tag, self.slot).resolve_in(module)?;
                $facade::try_from_function(function).ok()
            }
        }

        impl<Ret: FunctionReturn, Params: FunctionParamList, B: ModuleBrand>
            $name<Ret, Params, B>
        {
            /// Drop the parameter schema, yielding the underlying
            #[doc = concat!("[`FunctionId<Ret::Marker>`](FunctionId) — the id-side mirror of\n\
                             [`", stringify!($facade), "::as_function`](crate::", stringify!($facade), "::as_function).")]
            ///
            /// A pure retag of the same `(tag, slot)`: no view, no arena
            /// access, and no schema re-validation — the id could only have
            /// been minted from an already-validated facade.
            #[inline]
            pub fn as_function(self) -> FunctionId<Ret::Marker, B> {
                FunctionId::from_raw(self.tag, self.slot)
            }
        }
    )+ };
}

impl_view_in_for_typed_function_id!(
    TypedFunctionId => TypedFunctionValue,
    TypedVarArgsFunctionId => TypedVarArgsFunctionValue,
);

/// Implement [`ViewIn`] for a module-level global-value id whose handle is the
/// `{ id, module, ty }` scaffold: tag-check, confirm the arena slot really
/// holds that global category, then rebuild the handle with the pointer type
/// recovered from the arena. The category check is what keeps a foreign or
/// repurposed slot from reaching the handle's `unreachable!` payload accessor.
macro_rules! impl_view_in_for_global_id {
    ($( $name:ident => $handle:ident [$kind:ident] ),+ $(,)?) => { $(
        impl<B: ModuleBrand> sealed::Sealed for $name<B> {}
        impl<'ctx, B: ModuleBrand + 'ctx> ViewIn<'ctx, B> for $name<B> {
            type View = $handle<'ctx, B>;

            #[inline]
            fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View> {
                if self.tag != module.id() {
                    return None;
                }
                let data = module.value_data(self.slot);
                if !matches!(data.kind, ValueKindData::$kind(_)) {
                    return None;
                }
                Some($handle::from_parts_unchecked(self.slot, module, data.ty))
            }
        }
    )+ };
}

impl_view_in_for_global_id!(
    GlobalId => GlobalVariable [GlobalVariable],
    GlobalAliasId => GlobalAlias [GlobalAlias],
    GlobalIFuncId => GlobalIFunc [GlobalIFunc],
);

impl<R: ReturnMarker, B: ModuleBrand, Params: BlockParams> sealed::Sealed
    for BlockId<R, B, Params>
{
}
impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx, Params: BlockParams> ViewIn<'ctx, B>
    for BlockId<R, B, Params>
{
    type View = BasicBlockLabel<'ctx, R, B, Params>;

    #[inline]
    fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View> {
        if self.tag != module.id() {
            return None;
        }
        let data = module.value_data(self.slot);
        if !matches!(data.kind, ValueKindData::BasicBlock(_)) {
            return None;
        }
        debug_assert!(
            matches!(module.type_data(data.ty), TypeData::Label),
            "BlockId points at a non-label arena type at its slot",
        );
        Some(BasicBlockLabel {
            id: self.slot,
            module,
            ty: data.ty,
            _r: PhantomData,
            _params: PhantomData,
        })
    }
}

/// Tag-check `tag`/`slot` against `module`, confirm the slot really holds a
/// `call` instruction, and yield the call's result type from the arena.
/// Shared by the three call-shaped instruction ids, which differ only in the
/// handle they wrap around the same `(slot, module, result-type)` triple.
fn call_result_type_in<B: ModuleBrand>(
    tag: ModuleId,
    slot: ValueSlot,
    module: ModuleRef<'_, B>,
) -> Option<TypeSlot> {
    if tag != module.id() {
        return None;
    }
    let data = module.value_data(slot);
    let ValueKindData::Instruction(inst) = &data.kind else {
        return None;
    };
    if !matches!(inst.kind, InstructionKindData::Call(_)) {
        return None;
    }
    Some(data.ty)
}

impl<R: ReturnMarker, B: ModuleBrand> sealed::Sealed for CallInstId<R, B> {}
impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> ViewIn<'ctx, B> for CallInstId<R, B> {
    type View = CallInst<'ctx, R, B>;

    #[inline]
    fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View> {
        let ty = call_result_type_in(self.tag, self.slot, module)?;
        debug_assert!(
            signature_matches_marker::<R>(module.type_data(ty)),
            "CallInstId return marker does not match the arena result type at its slot",
        );
        Some(CallInst::from_raw(self.slot, module, ty))
    }
}

impl<Ret: FunctionReturn, B: ModuleBrand> sealed::Sealed for TypedCallInstId<Ret, B> {}
impl<'ctx, Ret: FunctionReturn, B: ModuleBrand + 'ctx> ViewIn<'ctx, B> for TypedCallInstId<Ret, B> {
    type View = TypedCallInst<'ctx, Ret, B>;

    #[inline]
    fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View> {
        let ty = call_result_type_in(self.tag, self.slot, module)?;
        debug_assert!(
            Ret::matches_ir_type(Type::new(ty, module)),
            "TypedCallInstId return schema does not match the arena result type at its slot",
        );
        Some(TypedCallInst::from_call(CallInst::from_raw(
            self.slot, module, ty,
        )))
    }
}

impl<R: ReturnMarker, B: ModuleBrand> sealed::Sealed for IntrinsicInstId<R, B> {}
impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> ViewIn<'ctx, B> for IntrinsicInstId<R, B> {
    type View = IntrinsicInst<'ctx, R, B>;

    #[inline]
    fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View> {
        let ty = call_result_type_in(self.tag, self.slot, module)?;
        debug_assert!(
            signature_matches_marker::<R>(module.type_data(ty)),
            "IntrinsicInstId return marker does not match the arena result type at its slot",
        );
        // `None` when the callee is not a generated intrinsic declaration —
        // the same rejection [`IntrinsicInst::from_call`] performs.
        IntrinsicInst::from_call(CallInst::from_raw(self.slot, module, ty))
    }
}

/// Implement [`ViewIn`] for a marker-free instruction id whose handle is the
/// `{ id, module, ty }` opcode scaffold: tag-check, confirm the arena slot
/// really holds that opcode, then rebuild the handle through its crate-private
/// constructor with the result type recovered from the arena.
macro_rules! impl_view_in_for_instruction_id {
    ($( $name:ident => $handle:ident [$kind:ident] ),+ $(,)?) => { $(
        impl<B: ModuleBrand> sealed::Sealed for $name<B> {}
        impl<'ctx, B: ModuleBrand + 'ctx> ViewIn<'ctx, B> for $name<B> {
            type View = $handle<'ctx, B>;

            #[inline]
            fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View> {
                if self.tag != module.id() {
                    return None;
                }
                let data = module.value_data(self.slot);
                let ValueKindData::Instruction(inst) = &data.kind else {
                    return None;
                };
                if !matches!(inst.kind, InstructionKindData::$kind(_)) {
                    return None;
                }
                Some($handle::from_raw(self.slot, module, data.ty))
            }
        }
    )+ };
}

impl_view_in_for_instruction_id!(
    FreezeInstId => FreezeInst [Freeze],
    VAArgInstId => VAArgInst [VAArg],
    AtomicRMWInstId => AtomicRMWInst [AtomicRMW],
    AtomicCmpXchgInstId => AtomicCmpXchgInst [AtomicCmpXchg],
    OtherPhiInstId => OtherPhiInst [Phi],
);

/// Tag-check `tag`/`slot` against `module`, confirm the slot really holds a
/// `phi`, and yield the phi's result type from the arena. The phi twin of
/// [`call_result_type_in`], shared by the three *typed* phi ids (the erased
/// [`OtherPhiInstId`] goes through [`impl_view_in_for_instruction_id`], which
/// performs the same two checks).
fn phi_result_type_in<B: ModuleBrand>(
    tag: ModuleId,
    slot: ValueSlot,
    module: ModuleRef<'_, B>,
) -> Option<TypeSlot> {
    if tag != module.id() {
        return None;
    }
    let data = module.value_data(slot);
    let ValueKindData::Instruction(inst) = &data.kind else {
        return None;
    };
    if !matches!(inst.kind, InstructionKindData::Phi(_)) {
        return None;
    }
    Some(data.ty)
}

impl<W: IntWidth, B: ModuleBrand> sealed::Sealed for PhiInstId<W, B> {}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> ViewIn<'ctx, B> for PhiInstId<W, B> {
    type View = PhiInst<'ctx, W, B>;

    #[inline]
    fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View> {
        let ty = phi_result_type_in(self.tag, self.slot, module)?;
        debug_assert!(
            matches!(
                module.type_data(ty),
                TypeData::Integer { bits } if W::static_bits().is_none_or(|w| w == *bits)
            ),
            "PhiInstId width marker does not match the arena result type at its slot",
        );
        Some(PhiInst::from_raw(self.slot, module, ty))
    }
}

impl<K: FloatKind, B: ModuleBrand> sealed::Sealed for FpPhiInstId<K, B> {}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> ViewIn<'ctx, B> for FpPhiInstId<K, B> {
    type View = FpPhiInst<'ctx, K, B>;

    #[inline]
    fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View> {
        let ty = phi_result_type_in(self.tag, self.slot, module)?;
        debug_assert!(
            matches!(
                module.type_data(ty),
                TypeData::Half
                    | TypeData::BFloat
                    | TypeData::Float
                    | TypeData::Double
                    | TypeData::X86Fp80
                    | TypeData::Fp128
                    | TypeData::PpcFp128
            ),
            "FpPhiInstId kind marker does not match the arena result type at its slot",
        );
        Some(FpPhiInst::from_raw(self.slot, module, ty))
    }
}

impl<B: ModuleBrand> sealed::Sealed for PointerPhiInstId<B> {}
impl<'ctx, B: ModuleBrand + 'ctx> ViewIn<'ctx, B> for PointerPhiInstId<B> {
    type View = PointerPhiInst<'ctx, B>;

    #[inline]
    fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View> {
        let ty = phi_result_type_in(self.tag, self.slot, module)?;
        debug_assert!(
            matches!(module.type_data(ty), TypeData::Pointer { .. }),
            "PointerPhiInstId points at a non-pointer phi result type at its slot",
        );
        Some(PointerPhiInst::from_raw(self.slot, module, ty))
    }
}

// --------------------------------------------------------------------------
// Into*-id: typed ids as builder operands
// --------------------------------------------------------------------------
//
// The three *typed-value* ids lift into their handle at an operand position,
// so that (in cycle B) `build_int_add(some_int_id, 2i32, "x")` compiles. Each
// conversion is *fallible* on a foreign module tag — unlike
// [`Module::view`](crate::Module::view), which panics — because the operand
// path is already `IrResult` and a foreign id is a recoverable caller error
// ([`IrError::ForeignValueId`]).
//
// The body delegates to the same [`ViewIn::resolve_in`] resolver the
// view-minting API uses (keeping one tag-check + arena-recovery path, including
// its debug-assert marker checks) and maps its `None` — reached *only* on a
// foreign tag for these three typed ids — to [`IrError::ForeignValueId`].
//
// Deliberately NOT implemented for the erased [`ValueId`]: erased -> typed must
// stay a *spelled* narrowing ([`Module::try_view`](crate::Module::try_view) /
// `TryFrom`), never a silent operand lift. See the compile-fail fixture
// `tests/compile_fail/erased_id_not_int_operand.rs`.

impl<W: IntWidth, B: ModuleBrand> into_int_value_sealed::Sealed for IntValueId<W, B> {}
impl<'ctx, W: IntWidth, B: ModuleBrand + 'ctx> IntoIntValue<'ctx, W, B> for IntValueId<W, B> {
    #[inline]
    fn into_int_value(self, module: ModuleRef<'ctx, B>) -> IrResult<IntValue<'ctx, W, B>> {
        self.resolve_in(module).ok_or(IrError::ForeignValueId)
    }
}

impl<K: FloatKind, B: ModuleBrand> into_float_value_sealed::Sealed for FloatValueId<K, B> {}
impl<'ctx, K: FloatKind, B: ModuleBrand + 'ctx> IntoFloatValue<'ctx, K, B> for FloatValueId<K, B> {
    #[inline]
    fn into_float_value(self, module: ModuleRef<'ctx, B>) -> IrResult<FloatValue<'ctx, K, B>> {
        self.resolve_in(module).ok_or(IrError::ForeignValueId)
    }
}

impl<B: ModuleBrand> into_pointer_value_sealed::Sealed for PointerValueId<B> {}
impl<'ctx, B: ModuleBrand + 'ctx> IntoPointerValue<'ctx, B> for PointerValueId<B> {
    #[inline]
    fn into_pointer_value(self, module: ModuleRef<'ctx, B>) -> IrResult<PointerValue<'ctx, B>> {
        self.resolve_in(module).ok_or(IrError::ForeignValueId)
    }
}

// --------------------------------------------------------------------------
// IntoErasedValue: every id at an erased-by-design operand slot
// --------------------------------------------------------------------------
//
// The counterpart to the block above for operand slots whose declared type is
// the *erased* [`Value`] — `build_store`'s stored value, the call-argument
// lists, the aggregate element slots, ... Every id lifts here, including the
// erased [`ValueId`], because widening an erased id to an erased operand is
// not the erased -> typed narrowing the `Into*Value` traits forbid; the
// compile-fail fixture `tests/compile_fail/erased_id_not_int_operand.rs` keeps
// that narrowing rejected at the *typed* positions.
//
// Each body reuses the same [`ViewIn::resolve_in`] resolver (one tag-check +
// arena-recovery path, debug-assert marker checks included) and maps its
// `None` to [`IrError::ForeignValueId`]. For the four value-shaped ids `None`
// is reached only on a foreign module tag; [`FunctionId`] and the three
// module-level global ids additionally return `None` on a value-category
// mismatch, which their minting accessors make unreachable — a foreign tag is
// likewise the only error a caller can actually provoke.
//
// [`BlockId`] is absent: its handle [`BasicBlockLabel`] is not an [`IsValue`]
// and a block is never an operand at these slots — it reaches a terminator
// through `IntoBasicBlockLabel`, not through the erased value path.
// [`TypedFunctionId`] / [`TypedVarArgsFunctionId`] are absent for the mirror
// reason: their handles are schema *facades*, not [`IsValue`]s, so an erased
// operand is reached through the underlying function
// ([`TypedFunctionId::as_function`]), never by widening the facade.

/// Implement [`IntoErasedValue`] for an id whose [`ViewIn`] handle is an
/// [`IsValue`], by resolving then widening. Optional square-bracketed marker
/// parameters are emitted ahead of the brand `B`, matching the id declarations.
macro_rules! impl_into_erased_value_for_id {
    ($( $name:ident $([$($mk:ident : $mkb:path),+ $(,)?])? ),+ $(,)?) => { $(
        impl<$($($mk: $mkb,)+)? B: ModuleBrand> into_erased_value_sealed::Sealed
            for $name<$($($mk,)+)? B>
        {
        }
        impl<'ctx, $($($mk: $mkb,)+)? B: ModuleBrand + 'ctx> IntoErasedValue<'ctx, B>
            for $name<$($($mk,)+)? B>
        {
            #[inline]
            fn into_erased_value(
                self,
                module: ModuleRef<'ctx, B>,
            ) -> IrResult<Value<'ctx, B>> {
                self.resolve_in(module)
                    .map(IsValue::into_erased)
                    .ok_or(IrError::ForeignValueId)
            }
        }
    )+ };
}

impl_into_erased_value_for_id!(
    ValueId,
    IntValueId[W: IntWidth],
    FloatValueId[K: FloatKind],
    PointerValueId,
    FunctionId[R: ReturnMarker],
    GlobalId,
    GlobalAliasId,
    GlobalIFuncId,
);

/// Implement [`IntoErasedValue`] for an instruction id whose opcode handle is
/// not an [`IsValue`] — the opcode scaffold widens through its inherent
/// `to_erased` instead.
///
/// Only for opcodes that *always* define a value. The call-shaped ids are
/// deliberately absent: a `call` may be void, which is exactly why
/// [`CallInst`] is not an [`IsValue`] either, and the compile-fail fixtures
/// `typed_call_void_result_use` / `call_void_no_return_accessor` keep that
/// rejection locked.
macro_rules! impl_into_erased_value_for_instruction_id {
    ($( $name:ident $([$($mk:ident : $mkb:path),+ $(,)?])? ),+ $(,)?) => { $(
        impl<$($($mk: $mkb,)+)? B: ModuleBrand> into_erased_value_sealed::Sealed
            for $name<$($($mk,)+)? B>
        {
        }
        impl<'ctx, $($($mk: $mkb,)+)? B: ModuleBrand + 'ctx> IntoErasedValue<'ctx, B>
            for $name<$($($mk,)+)? B>
        {
            #[inline]
            fn into_erased_value(
                self,
                module: ModuleRef<'ctx, B>,
            ) -> IrResult<Value<'ctx, B>> {
                self.resolve_in(module)
                    .map(|inst| inst.to_erased())
                    .ok_or(IrError::ForeignValueId)
            }
        }
    )+ };
}

impl_into_erased_value_for_instruction_id!(
    FreezeInstId,
    VAArgInstId,
    AtomicRMWInstId,
    AtomicCmpXchgInstId,
    PhiInstId[W: IntWidth],
    FpPhiInstId[K: FloatKind],
    PointerPhiInstId,
    OtherPhiInstId,
);
