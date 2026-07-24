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
//! This slice is purely **additive**: builders still return the borrowing
//! handles; each handle merely gains a [`to_id`](crate::Value::to_id)-style
//! accessor that mints its id. Cycle B rewires the builders to return ids.
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
use crate::float_kind::FloatKind;
use crate::function::{FunctionValue, signature_matches_marker};
use crate::global_variable::GlobalVariable;
use crate::int_width::IntWidth;
use crate::marker::ReturnMarker;
use crate::module::{Invariant, ModuleBrand, ModuleId, ModuleRef};
use crate::r#type::TypeData;
use crate::value::{FloatValue, IntValue, PointerValue, Value, ValueKindData, ValueSlot};

// --------------------------------------------------------------------------
// The id family
// --------------------------------------------------------------------------

/// Declare a minimal, `Copy` value id with an optional single leading type
/// marker (before the always-present brand `B`). Generates the struct plus
/// manual `Copy`/`Clone`/`Eq`/`PartialEq`/`Hash`/`Debug` impls — manual
/// because a `derive` would propagate a `Marker: Trait` bound onto the impl
/// that callers should never have to spell (the `FunctionValue` precedent),
/// and because `Debug` must print `tag`/`slot` only, never the phantoms.
macro_rules! decl_value_id {
    (
        $(#[$attr:meta])*
        $name:ident $([$mk:ident : $mkb:path => $mf:ident])?
    ) => {
        $(#[$attr])*
        pub struct $name<$($mk: $mkb,)? B: ModuleBrand> {
            tag: ModuleId,
            slot: ValueSlot,
            $($mf: PhantomData<$mk>,)?
            _brand: Invariant<B>,
        }

        impl<$($mk: $mkb,)? B: ModuleBrand> $name<$($mk,)? B> {
            /// Crate-internal: mint an id from an already-resolved tag + slot.
            /// The only callers are the value handles' `to_id` accessors, which
            /// pass their owning [`ModuleId`] and arena slot.
            #[inline]
            pub(crate) fn from_raw(tag: ModuleId, slot: ValueSlot) -> Self {
                Self {
                    tag,
                    slot,
                    $($mf: PhantomData,)?
                    _brand: PhantomData,
                }
            }
        }

        impl<$($mk: $mkb,)? B: ModuleBrand> Clone for $name<$($mk,)? B> {
            #[inline]
            fn clone(&self) -> Self {
                *self
            }
        }
        impl<$($mk: $mkb,)? B: ModuleBrand> Copy for $name<$($mk,)? B> {}
        impl<$($mk: $mkb,)? B: ModuleBrand> PartialEq for $name<$($mk,)? B> {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                self.tag == other.tag && self.slot == other.slot
            }
        }
        impl<$($mk: $mkb,)? B: ModuleBrand> Eq for $name<$($mk,)? B> {}
        impl<$($mk: $mkb,)? B: ModuleBrand> core::hash::Hash for $name<$($mk,)? B> {
            #[inline]
            fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
                self.tag.hash(state);
                self.slot.hash(state);
            }
        }
        impl<$($mk: $mkb,)? B: ModuleBrand> core::fmt::Debug for $name<$($mk,)? B> {
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
    /// Storable, module-tagged id for a module-level global variable, resolved
    /// into a [`GlobalVariable`].
    GlobalId
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

impl<B: ModuleBrand> sealed::Sealed for GlobalId<B> {}
impl<'ctx, B: ModuleBrand + 'ctx> ViewIn<'ctx, B> for GlobalId<B> {
    type View = GlobalVariable<'ctx, B>;

    #[inline]
    fn resolve_in(self, module: ModuleRef<'ctx, B>) -> Option<Self::View> {
        if self.tag != module.id() {
            return None;
        }
        let data = module.value_data(self.slot);
        if !matches!(data.kind, ValueKindData::GlobalVariable(_)) {
            return None;
        }
        Some(GlobalVariable::from_parts_unchecked(
            self.slot, module, data.ty,
        ))
    }
}

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
