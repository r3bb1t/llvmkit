//! Top-level IR container. Mirrors `llvm/include/llvm/IR/Module.h` and
//! `llvm/lib/IR/Module.cpp`.
//!
//! Top-level container: module identity/name, source filename, data
//! layout, target triple, and module-level asm; the full type-constructor
//! surface required by `IrBuilder` and the `.ll` parser; and functions,
//! globals, aliases, ifuncs, comdats, named metadata, and
//! use-list-order records.
//!
//! ## Identity and verification model
//!
//! A [`Module`] is a linear token that **owns** its `ModuleCore` storage and
//! borrows nothing, so it has no lifetime parameter: it is an ordinary movable
//! value. The token carries a [`ModuleBrand`] and a verification state:
//! [`Unverified`] while IR is still being built and [`Verified`] after
//! structural verification succeeds. Handles store a state-erased
//! [`ModuleRef`] with the same brand, so same-brand APIs reject cross-module
//! values statically and erased/parser paths can still fall back to
//! [`ModuleId`] checks.
//!
//! ## Choosing a brand
//!
//! | Constructor | Brand | Live modules per brand |
//! |---|---|---|
//! | [`Module::branded::<B, _>`](Module::branded) | a type you name | one at a time |
//! | [`Module::branded_once::<B, _>`](Module::branded_once) | a type you name | one, ever |
//! | [`module_new!`](crate::module_new) | fresh, unnameable, per expansion site | one at a time |
//! | [`Module::dynamic`] | [`DynBrand`] | unlimited (registry-exempt) |
//!
//! The first three are kept distinct by a process-global registry; see
//! [`ModuleBrand`]. [`DynBrand`] opts out of the compile-time half of identity
//! and relies on the [`ModuleId`] tag alone.
//!
//! Public handle accessors expose [`ModuleView`], a branded view of the
//! storage. It carries every *type* constructor — interning a type is
//! preservation-neutral, so it needs no mutation authority, which is what lets
//! the schema traits ([`crate::IrField`], [`crate::StructSchema`]) be declared
//! against it. Module-structural work — declaring functions, globals, aliases,
//! ifuncs, and the typestate struct-body setters — requires the unverified
//! [`Module`] token instead.

use crate::Branded;
use core::any::TypeId;
use core::hash::{Hash, Hasher};
use core::iter::FusedIterator;
use core::marker::PhantomData;
use core::num::NonZeroU64;
use core::sync::atomic::{AtomicU64, Ordering};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

use super::align::MaybeAlign;
use super::array_len::{ArrLen, ArrLenDyn};
use super::attributes::AttributeStorage;
use super::basic_block::BasicBlock;
use super::comdat::{ComdatData, ComdatId, ComdatRef, SelectionKind};
use super::constant::{
    Constant, ConstantExprFlags, ConstantExprOpcode, ForwardRefValue, IntoConstantValue, IsConstant,
};
use super::constant_range::metadata_constant_int;
use super::constants::ConstantExprOptions;
use super::data_layout::DataLayout;
use super::derived_types::{
    ArrayType, FloatType, FunctionType, IntType, LabelType, MetadataType, PointerType, StructType,
    TargetExtType, TokenType, VectorType, VoidType,
};
use super::element::{ElemDyn, StaticVecElem};
use super::error::{IrError, IrResult, TypeKindLabel};
use super::float_kind::{Bfloat, Fp128, Half, PpcFp128, X86Fp80};
use super::function::FunctionData;
use super::function::{FunctionBuilder, FunctionValue};
use super::function_signature::TypedVarArgsFunctionValue;
use super::function_signature::{
    FunctionParamList, FunctionReturn, FunctionSignature, TypedFunctionValue,
};
use super::global_alias::{GlobalAlias, GlobalAliasBuilder};
use super::global_ifunc::{GlobalIfunc, GlobalIfuncBuilder};
use super::global_value::{DllStorageClass, Linkage, ThreadLocalMode, Visibility};
use super::global_variable::{GlobalBuilder, GlobalVariable};
use super::inline_asm::{InlineAsm, InlineAsmData, InlineAsmOptions};
use super::int_width::{IntDyn, Width};
use super::intrinsics::IntrinsicFunctionData;
use super::intrinsics::{
    IntrinsicDescriptor, IntrinsicId, IntrinsicNameResolution, descriptor_for_name,
    resolve_intrinsic_name,
};
use super::llvm_context::Context;
use super::marker::Dyn;
use super::marker::ReturnMarker;
use super::metadata::{
    MetadataAttachmentSet, MetadataId, MetadataKind, MetadataSlot, MetadataStore,
    SpecializedMetadataNode, StoredBrand,
};
use super::module_flags::{
    ModuleFlagBehavior, ModuleFlagEntry, ModuleFlagKey, module_flag_tuple, resolve_metadata_ref,
};
use super::named_md_node::{
    NamedMetadataId, NamedMetadataName, NamedMetadataNode, NamedMetadataSlot,
};
use super::pass_context::{FunctionView, ModuleFunctionViews};
use super::struct_body_state::StructBodyDyn;
use super::struct_body_state::{BodySet, Opaque};
use super::struct_schema::StructSchema;
use super::r#type::{MAX_INT_BITS, MIN_INT_BITS, StructBody, Type, TypeData, TypeSlot};
use super::typed_pointer_type::TypedPointerType;
use super::unnamed_addr::UnnamedAddr;
use super::value::{GlobalFieldKind, Value, ValueData, ValueKindData, ValueSlot, ValueUse};
use super::value_id::{
    FunctionId, GlobalAliasId, GlobalId, GlobalIfuncId, TypedFunctionId, TypedVarArgsFunctionId,
    ValueId, ViewIn,
};
use super::vec_len::{Len, LenDyn};
use super::verifier::Verifier;

#[cfg(test)]
mod brand_registry_tests;

fn reject_reserved_intrinsic_name(name: &str) -> IrResult<()> {
    match resolve_intrinsic_name(name) {
        IntrinsicNameResolution::NonIntrinsic => Ok(()),
        IntrinsicNameResolution::UnknownIntrinsic => Err(IrError::UnknownIntrinsic {
            name: name.to_owned(),
        }),
        IntrinsicNameResolution::Known(_) => Err(IrError::ReservedIntrinsicName {
            name: name.to_owned(),
        }),
    }
}

// --------------------------------------------------------------------------
// ModuleId
// --------------------------------------------------------------------------

/// Globally-unique module identifier. Assigned at construction by an
/// atomic counter; never reused within a process.
///
/// The counter is 64-bit: id handles pack this tag alongside an arena
/// index, and a 64-bit tag can never be re-issued within a process (a
/// `u32` counter could in principle wrap after `u32::MAX` module
/// creations and hand a live successor the tag of a dropped module).
///
/// Ordered by allocation: the counter is monotone, so `a < b` means `a`'s
/// module was constructed first. That is what makes the id family's
/// `(tag, slot)` ordering deterministic enough to key a `BTreeMap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleId(NonZeroU64);

impl ModuleId {
    /// Allocate the next unused id. The counter starts at 1 so the
    /// underlying `NonZeroU64` always has its niche populated.
    fn fresh() -> Self {
        // `Relaxed` is fine: the counter only needs uniqueness, not
        // happens-before ordering with any other memory operation.
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let raw = NEXT.fetch_add(1, Ordering::Relaxed);
        let nz = NonZeroU64::new(raw).expect("ModuleId counter overflow (>u64::MAX modules)");
        Self(nz)
    }

    /// Raw integer value. Useful for diagnostics.
    #[inline]
    pub fn as_u64(self) -> u64 {
        self.0.get()
    }
}

// --------------------------------------------------------------------------
// Module brands and verification state
// --------------------------------------------------------------------------

/// Marker for a module identity brand.
///
/// A brand is the *compile-time* half of module identity: handles minted by a
/// [`Module`] carry its brand, so an API that takes two same-brand handles
/// rejects a cross-module mix-up at type-check time. The *runtime* half is the
/// [`ModuleId`] tag every stored id also carries, and it is checked
/// independently. A brand is therefore a **hygiene device, not a soundness
/// boundary**: the worst a strange brand type can do is collapse a compile-time
/// rejection into the runtime [`ModuleId`] check, which still refuses the
/// operation.
///
/// The trait is deliberately **empty and unsealed** — any type may be a brand:
///
/// - a *named* brand for a module a program builds exactly once
///   ([`Module::branded`]);
/// - a fresh unnameable brand per expansion site
///   ([`module_new!`](crate::module_new));
/// - [`DynBrand`] when a program needs many modules of the same static shape
///   and is content with the runtime tag alone.
///
/// No code path in this crate ever constructs a `B` or calls a method on one:
/// every occurrence of the brand in a data structure is
/// `Invariant<B>` = `PhantomData<fn(B) -> B>`, which is inhabited-free,
/// invariant in `B`, and `Send + Sync` whatever `B` is.
///
/// # No supertraits — a brand is a bare unit struct
///
/// ```
/// struct LiftedBin;
/// impl llvmkit_ir::ModuleBrand for LiftedBin {}
/// ```
///
/// The trait demands nothing of the type: no derives, no `Copy`, no `Debug`.
/// Until 0.0.4 froze, it carried `Copy + Debug + Eq + Hash` — not because any
/// code called those methods on a brand (none ever did), but because the
/// brand-generic containers used std `#[derive]`, and a std derive on a
/// generic type emits `where B: Clone` / `B: Debug` / … bounds whether or not
/// `B` appears in a position that needs them. Those containers now use the
/// `Branded` derive from `llvmkit-macros`, which copies each type's generics
/// verbatim and adds no bounds, so the requirement disappeared. Deriving
/// traits on your brand type remains legal — just never required.
///
/// # Why `'static` *is* a supertrait
///
/// The uniqueness registry behind [`Module::branded`] keys brands by
/// [`TypeId`], which requires `'static`. Every brand is therefore `'static`,
/// and the bound lives here rather than being repeated on each registering
/// constructor. A brand is a pure marker — it names a module, it does not
/// borrow one — so `'static` costs a user nothing.
pub trait ModuleBrand: 'static {}

/// Brand for modules that opt out of compile-time identity separation.
///
/// `DynBrand` is **exempt from the uniqueness registry**: arbitrarily many
/// `Module<DynBrand>` values may be live at once, they may be collected in
/// a `Vec`, and they are separated from one another only by the runtime
/// [`ModuleId`] tag. Reach for it when the module count is dynamic — a loop
/// over translation units, a worker pool, a `Vec` of modules — where no single
/// static type could name each module individually.
///
/// The trade is exactly the compile-time half of identity: a handle from one
/// `DynBrand` module and a handle from another have the *same* type, so a
/// mix-up surfaces as an [`IrError::ForeignValueId`] at run time instead of a
/// type error at compile time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct DynBrand;
impl ModuleBrand for DynBrand {}

/// Module state before successful structural verification.
#[derive(Debug)]
pub enum Unverified {}

/// Module state after successful structural verification.
#[derive(Debug)]
pub enum Verified {}

pub(super) type Invariant<T> = PhantomData<fn(T) -> T>;

// --------------------------------------------------------------------------
// Brand uniqueness registry
// --------------------------------------------------------------------------

/// Liveness of one registered brand type.
enum BrandState {
    /// A live [`Module`] holds this brand right now.
    InUse,
    /// A [`Module::branded_once`] module held this brand and has been dropped.
    /// Permanent: no successor may ever claim the brand again, so a `'static`
    /// id minted by the dead module can never be replayed against a live one.
    Retired,
}

/// Process-global brand registry: at most one live [`Module`] per brand type.
///
/// [`DynBrand`] never appears here — it is registry-exempt by construction,
/// because [`Module::dynamic`] does not call [`BrandGuard::claim`].
static BRANDS: LazyLock<Mutex<HashMap<TypeId, BrandState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Lock the registry, recovering from poisoning.
///
/// Recovery is sound here because every critical section is a *single* map
/// operation: there is no intermediate state in which the map could be
/// observed, so a panic elsewhere in the process cannot have left the map
/// half-updated. (In practice the sections cannot panic at all — hashing a
/// [`TypeId`] and inserting into a `HashMap` are infallible — but recovering
/// keeps a poisoned mutex from bricking every later module construction.)
fn lock_brands() -> MutexGuard<'static, HashMap<TypeId, BrandState>> {
    BRANDS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// RAII claim on one brand type, held by the [`Module`] that owns the brand.
///
/// The guard is created by [`claim`](Self::claim) and immediately **defused
/// into the module** — moved into `Module::registration`, so the module's own
/// storage is what keeps the claim alive. Releasing is therefore driven by the
/// guard's [`Drop`], which runs on a normal drop *and* on unwind.
///
/// `Drop` lives here rather than on [`Module`] on purpose: the typestate
/// transitions ([`Module::verify`], [`Module::unverify`]) move fields out of
/// `self`, and moving out of a type that implements `Drop` is `E0509`. A
/// `Drop` field is fine — only a `Drop` *impl on the outer struct* forbids the
/// move — so the guard rides along through every transition untouched.
///
/// Storing the brand as [`Invariant<B>`] (never `PhantomData<B>`) is what keeps
/// the guard — and hence [`Module`] — `Send` and `Sync` no matter what type the
/// user picked for `B`, while leaving `B` invariant.
struct BrandGuard<B> {
    /// The claimed key. Captured at claim time, so `Drop` needs no `B: 'static`.
    brand: TypeId,
    /// `true` for [`Module::branded_once`]: retire instead of releasing.
    retire_on_drop: bool,
    _brand: Invariant<B>,
}

impl<B> BrandGuard<B> {
    /// Claim `B`, or fail if it is already live or permanently retired.
    ///
    /// The critical section is exactly **one** map operation. The `entry`
    /// lookup *is* the check-and-set — there is no `contains_key` followed by a
    /// second `insert`, and therefore no window in which two threads could both
    /// observe the brand as free.
    ///
    /// No user code runs inside the section: `B` is only ever fed to
    /// [`TypeId::of`] and [`core::any::type_name`], both of which are compiler
    /// intrinsics, and both are evaluated *before* the lock is taken.
    fn claim(retire_on_drop: bool) -> IrResult<Self>
    where
        B: ModuleBrand,
    {
        let brand = TypeId::of::<B>();
        let name = core::any::type_name::<B>();
        match lock_brands().entry(brand) {
            Entry::Occupied(slot) => match slot.get() {
                BrandState::InUse => Err(IrError::BrandInUse { brand: name }),
                BrandState::Retired => Err(IrError::BrandRetired { brand: name }),
            },
            Entry::Vacant(slot) => {
                slot.insert(BrandState::InUse);
                Ok(Self {
                    brand,
                    retire_on_drop,
                    _brand: PhantomData,
                })
            }
        }
    }
}

impl<B> Drop for BrandGuard<B> {
    /// Release the claim — one map operation, same as [`claim`](Self::claim).
    ///
    /// There is deliberately **no** way to release a claim without dropping the
    /// guard. A force-unregister API would let a fresh module take a brand
    /// whose predecessor is still alive, so `'static` handles of two different
    /// generations would share one type — demoting the compile-time guarantee
    /// to the runtime [`ModuleId`] check. Correspondingly,
    /// [`core::mem::forget`]ting or otherwise leaking a module skips this
    /// `Drop` and leaves the brand `InUse` forever: leaking is an implicit
    /// [`Module::branded_once`], which is deterministic and safe.
    fn drop(&mut self) {
        let mut brands = lock_brands();
        if self.retire_on_drop {
            brands.insert(self.brand, BrandState::Retired);
        } else {
            brands.remove(&self.brand);
        }
    }
}

/// Construct a module under a brand type **generated at this expansion site**.
///
/// The macro expands to a block that declares a fresh `struct`, implements
/// [`ModuleBrand`] for it, and hands it to [`Module::branded`]. Because the
/// struct is declared *inside* the block it is unnameable from anywhere else,
/// so no other code can spell the brand. It is the ergonomic descendant of the
/// generative lifetime brand this crate used to mint from a callback, but on an
/// owned, movable token rather than one pinned to the callback's frame.
///
/// ```
/// use llvmkit_ir::{IrError, module_new};
///
/// let m = module_new!("lifted")?;
/// assert_eq!(m.name(), "lifted");
///
/// // A second expansion site is a *different* brand, so both are live at once.
/// let other = module_new!("other")?;
/// assert_ne!(m.id(), other.id());
/// # Ok::<(), IrError>(())
/// ```
///
/// # One brand per expansion *site*, not per evaluation
///
/// The brand is minted where the macro is *written*, not each time control
/// reaches it. A `module_new!` inside a loop therefore asks for the same brand
/// on every iteration, and the second iteration fails with
/// [`IrError::BrandInUse`] while the first module is still alive:
///
/// ```
/// use llvmkit_ir::{IrError, Module, module_new};
///
/// let mut held = Vec::new();
/// for i in 0..2 {
///     match module_new!(format!("m{i}")) {
///         Ok(m) => held.push(m),
///         Err(e) => assert!(matches!(e, IrError::BrandInUse { .. })),
///     }
/// }
/// assert_eq!(held.len(), 1);
///
/// // For a dynamic number of modules, use the registry-exempt brand instead.
/// let all: Vec<_> = (0..2).map(|i| Module::dynamic(format!("m{i}"))).collect();
/// assert_eq!(all.len(), 2);
/// ```
///
/// (Dropping each module before the next iteration also works, since a
/// non-retired brand is released on drop — but a `Vec` of them cannot.)
#[macro_export]
macro_rules! module_new {
    ($name:expr $(,)?) => {{
        // The doubled braces are load-bearing. `macro_rules!` hygiene protects
        // local *variables*, not *item* names, so this `struct` would otherwise
        // land in the caller's scope under a nameable, collision-prone name.
        // The block scope is the whole mechanism that makes the brand unnameable
        // and distinct per expansion site.
        struct __LlvmkitGeneratedBrand;
        impl $crate::ModuleBrand for __LlvmkitGeneratedBrand {}
        $crate::Module::branded::<__LlvmkitGeneratedBrand, _>($name)
    }};
}

// --------------------------------------------------------------------------
// ModuleRef helper
// --------------------------------------------------------------------------

/// State-erased reference to a module's storage.
///
/// The reference carries the invariant module brand `B`, but it points at
/// crate-private `ModuleCore` storage rather than a `Module<..., State>` token,
/// so handles do not borrow the verification state.
///
/// # Why this is not [`ModuleView`]
///
/// The two are **representationally identical** — both are
/// `(&'ctx ModuleCore, Invariant<B>)` — and cycle E considered merging them.
/// They are kept apart because they differ in *capability*, which in this crate
/// is what a type is for:
///
/// - `ModuleRef` is the **storage pointer**. It is the `module` field embedded
///   in every borrowing handle, and its entire public surface is
///   [`id`](Self::id). It says "I can find the arena", nothing more.
/// - [`ModuleView`] is the **read capability**. It is what a user receives from
///   a handle's `module()` accessor, and it carries the full read surface plus
///   all 35 type constructors.
///
/// A function that takes a `ModuleRef` is therefore stating that it only needs
/// to resolve slots — not that it may read the module or intern new types. That
/// is the same capability-by-type pattern the crate uses for
/// [`Instruction`](crate::Instruction) versus
/// [`InstructionView`](crate::InstructionView) (one value, two capabilities) and
/// for [`Module<B, Verified>`](Module) versus `Module<B, Unverified>` (one
/// storage, two capabilities). Identical layout with a different method set is
/// how Rust encodes a capability grade; it is not accidental duplication.
///
/// The "one concept, one representation" principle in the README is about not
/// implementing a *concept* twice, which is not what these do: there is exactly
/// one module-storage concept here, exposed at two capability grades.
pub struct ModuleRef<'ctx, B: ModuleBrand> {
    core: &'ctx ModuleCore,
    _brand: Invariant<B>,
}

impl<B: ModuleBrand> Clone for ModuleRef<'_, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<B: ModuleBrand> Copy for ModuleRef<'_, B> {}

impl<'ctx, B: ModuleBrand> ModuleRef<'ctx, B> {
    #[inline]
    pub(super) fn new(core: &'ctx ModuleCore) -> Self {
        Self {
            core,
            _brand: PhantomData,
        }
    }

    /// Borrow the underlying state-erased module storage.
    pub(super) fn module(self) -> &'ctx ModuleCore {
        self.core
    }

    /// Owning module's [`ModuleId`].
    #[inline]
    pub fn id(self) -> ModuleId {
        self.core.id
    }

    /// Crate-internal: resolve a [`TypeSlot`] to its payload via the
    /// owning module's context.
    #[inline]
    pub(super) fn type_data(self, id: TypeSlot) -> &'ctx TypeData {
        self.core.context().type_data(id)
    }

    /// Crate-internal: resolve a [`ValueSlot`](crate::value::ValueSlot) to its
    /// payload via the owning module's context.
    #[inline]
    pub(super) fn value_data(self, id: ValueSlot) -> &'ctx ValueData {
        self.core.context().value_data(id)
    }
}

impl<'ctx, B: ModuleBrand> From<&'ctx ModuleCore> for ModuleRef<'ctx, B> {
    #[inline]
    fn from(core: &'ctx ModuleCore) -> Self {
        ModuleRef::new(core)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, S> From<&'ctx Module<B, S>> for ModuleRef<'ctx, B> {
    #[inline]
    fn from(module: &'ctx Module<B, S>) -> Self {
        module.module_ref()
    }
}

impl<B: ModuleBrand> PartialEq for ModuleRef<'_, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.core.id == other.core.id
    }
}
impl<B: ModuleBrand> Eq for ModuleRef<'_, B> {}
impl<B: ModuleBrand> Hash for ModuleRef<'_, B> {
    #[inline]
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.core.id.hash(h);
    }
}
impl<B: ModuleBrand> core::fmt::Debug for ModuleRef<'_, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("ModuleRef").field(&self.core.id).finish()
    }
}

// --------------------------------------------------------------------------
// ModuleView helper
// --------------------------------------------------------------------------

/// Branded view of a module: everything reachable without mutation authority.
///
/// `ModuleView` lets handles report their owning module without exposing the
/// crate-private storage or the linear verification-state token. Beyond reads
/// it carries the full type-constructor surface (see the `Type constructors`
/// section below for why that is not a loosening), which is what the
/// user-implementable schema traits ([`IrField::ir_type`](crate::IrField),
/// [`StructSchema`], the `FunctionReturn` /
/// `FunctionParam` family) are declared against.
///
/// This is the **read capability** grade over a module's storage;
/// [`ModuleRef`] is the bare **storage pointer** grade. The two have the same
/// layout on purpose — see [`ModuleRef`]'s "Why this is not `ModuleView`"
/// section for why they stay distinct types.
#[derive(Branded)]
#[branded(Clone, Copy)]
pub struct ModuleView<'ctx, B: ModuleBrand> {
    core: &'ctx ModuleCore,
    _brand: Invariant<B>,
}

/// Read-only branded view of a global variable.
#[derive(Branded)]
pub struct GlobalVariableView<'ctx, B: ModuleBrand> {
    global: GlobalVariable<'ctx, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalVariableView<'ctx, B> {
    #[inline]
    pub(super) fn new(global: GlobalVariable<'ctx, B>) -> Self {
        Self { global }
    }

    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        self.global.module()
    }

    #[inline]
    pub fn ty(self) -> PointerType<'ctx, B> {
        self.global.ty()
    }

    #[inline]
    pub fn value_type(self) -> Type<'ctx, B> {
        self.global.value_type()
    }

    #[inline]
    pub fn address_space(self) -> u32 {
        self.global.address_space()
    }

    #[inline]
    pub fn name(self) -> &'ctx str {
        self.global.name()
    }

    #[inline]
    pub fn is_constant(self) -> bool {
        self.global.is_constant()
    }

    #[inline]
    pub fn is_externally_initialized(self) -> bool {
        self.global.is_externally_initialized()
    }

    #[inline]
    pub fn has_initializer(self) -> bool {
        self.global.has_initializer()
    }

    #[inline]
    pub fn initializer(self) -> Option<Constant<'ctx, B>> {
        self.global.initializer()
    }

    #[inline]
    pub fn linkage(self) -> Linkage {
        self.global.linkage()
    }

    #[inline]
    pub fn visibility(self) -> Visibility {
        self.global.visibility()
    }

    #[inline]
    pub fn dll_storage_class(self) -> DllStorageClass {
        self.global.dll_storage_class()
    }

    #[inline]
    pub fn thread_local_mode(self) -> ThreadLocalMode {
        self.global.thread_local_mode()
    }

    #[inline]
    pub fn unnamed_addr(self) -> UnnamedAddr {
        self.global.unnamed_addr()
    }

    #[inline]
    pub fn align(self) -> MaybeAlign {
        self.global.align()
    }

    #[inline]
    pub fn has_section(self) -> bool {
        self.global.has_section()
    }

    #[inline]
    pub fn section(self) -> Option<String> {
        self.global.section()
    }

    #[inline]
    pub fn partition(self) -> Option<String> {
        self.global.partition()
    }

    #[inline]
    pub fn comdat(self) -> Option<ComdatView<'ctx, B>> {
        self.global.comdat().map(ComdatView::new)
    }

    #[inline]
    pub fn metadata(self) -> MetadataAttachmentSet<B> {
        self.global.metadata()
    }
}

/// Read-only branded view of a global alias.
#[derive(Branded)]
pub struct GlobalAliasView<'ctx, B: ModuleBrand> {
    alias: GlobalAlias<'ctx, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalAliasView<'ctx, B> {
    #[inline]
    pub(super) fn new(alias: GlobalAlias<'ctx, B>) -> Self {
        Self { alias }
    }

    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        self.alias.module()
    }

    #[inline]
    pub fn ty(self) -> PointerType<'ctx, B> {
        self.alias.ty()
    }

    #[inline]
    pub fn value_type(self) -> Type<'ctx, B> {
        self.alias.value_type()
    }

    #[inline]
    pub fn address_space(self) -> u32 {
        self.alias.address_space()
    }

    #[inline]
    pub fn name(self) -> &'ctx str {
        self.alias.name()
    }

    #[inline]
    pub fn aliasee(self) -> Constant<'ctx, B> {
        self.alias.aliasee()
    }

    #[inline]
    pub fn linkage(self) -> Linkage {
        self.alias.linkage()
    }

    #[inline]
    pub fn visibility(self) -> Visibility {
        self.alias.visibility()
    }

    #[inline]
    pub fn dll_storage_class(self) -> DllStorageClass {
        self.alias.dll_storage_class()
    }

    #[inline]
    pub fn thread_local_mode(self) -> ThreadLocalMode {
        self.alias.thread_local_mode()
    }

    #[inline]
    pub fn unnamed_addr(self) -> UnnamedAddr {
        self.alias.unnamed_addr()
    }

    #[inline]
    pub fn metadata(self) -> MetadataAttachmentSet<B> {
        self.alias.metadata()
    }

    #[inline]
    pub fn partition(self) -> Option<String> {
        self.alias.partition()
    }
}

/// Read-only branded view of a global ifunc.
#[derive(Branded)]
pub struct GlobalIfuncView<'ctx, B: ModuleBrand> {
    ifunc: GlobalIfunc<'ctx, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> GlobalIfuncView<'ctx, B> {
    #[inline]
    pub(super) fn new(ifunc: GlobalIfunc<'ctx, B>) -> Self {
        Self { ifunc }
    }

    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        self.ifunc.module()
    }

    #[inline]
    pub fn ty(self) -> PointerType<'ctx, B> {
        self.ifunc.ty()
    }

    #[inline]
    pub fn value_type(self) -> Type<'ctx, B> {
        self.ifunc.value_type()
    }

    #[inline]
    pub fn address_space(self) -> u32 {
        self.ifunc.address_space()
    }

    #[inline]
    pub fn name(self) -> &'ctx str {
        self.ifunc.name()
    }

    #[inline]
    pub fn resolver(self) -> Constant<'ctx, B> {
        self.ifunc.resolver()
    }

    #[inline]
    pub fn linkage(self) -> Linkage {
        self.ifunc.linkage()
    }

    #[inline]
    pub fn visibility(self) -> Visibility {
        self.ifunc.visibility()
    }

    #[inline]
    pub fn metadata(self) -> MetadataAttachmentSet<B> {
        self.ifunc.metadata()
    }

    #[inline]
    pub fn partition(self) -> Option<String> {
        self.ifunc.partition()
    }
}

/// Read-only branded view of a COMDAT.
#[derive(Branded)]
pub struct ComdatView<'ctx, B: ModuleBrand> {
    comdat: ComdatRef<'ctx, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> ComdatView<'ctx, B> {
    #[inline]
    pub(super) fn new(comdat: ComdatRef<'ctx, B>) -> Self {
        Self { comdat }
    }

    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.comdat.module.module())
    }

    #[inline]
    pub fn name(self) -> &'ctx str {
        self.comdat.name()
    }

    #[inline]
    pub fn selection_kind(self) -> SelectionKind {
        self.comdat.selection_kind()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> ModuleView<'ctx, B> {
    #[inline]
    pub(super) fn new(core: &'ctx ModuleCore) -> Self {
        Self {
            core,
            _brand: PhantomData,
        }
    }

    #[inline]
    pub(super) fn core_ref(self) -> &'ctx ModuleCore {
        self.core
    }

    #[inline]
    pub(super) fn context(self) -> &'ctx Context {
        self.core.context()
    }

    /// Owning module's [`ModuleId`].
    #[inline]
    pub fn id(self) -> ModuleId {
        self.core.id()
    }

    /// Resolve a storable id back into its borrowing handle — the same
    /// module-tag choke point as [`Module::view`], reachable from a read-only
    /// [`ModuleView`].
    ///
    /// This is what lets a *pass* speak ids: the capability-graded pass surface
    /// hands out a `ModuleView` (never `&Module`, whose declaration surface no
    /// mutating rung's preservation floor accounts for), so without this the
    /// ids a pass stores would have no resolution path inside `run`.
    ///
    /// # Panics
    ///
    /// Panics if the id belongs to a different module (foreign tag) or its slot
    /// is absent, exactly as [`Module::view`] does. Use
    /// [`try_view`](Self::try_view) for the fallible form.
    #[inline]
    pub fn view<I>(self, id: I) -> I::View
    where
        I: ViewIn<'ctx, B>,
    {
        id.resolve_in(self.into()).unwrap_or_else(|| {
            panic!(
                "ModuleView::view: id does not resolve in this module \
                 (foreign module tag or absent/tombstoned slot)"
            )
        })
    }

    /// Fallible [`view`](Self::view): `None` when the id belongs to a different
    /// module (foreign tag) or its slot is absent. The `ModuleView` twin of
    /// [`Module::try_view`].
    #[inline]
    pub fn try_view<I>(self, id: I) -> Option<I::View>
    where
        I: ViewIn<'ctx, B>,
    {
        id.resolve_in(self.into())
    }

    /// Module identifier.
    #[inline]
    pub fn name(self) -> &'ctx str {
        self.core.name()
    }

    /// `source_filename = "..."` directive.
    #[inline]
    pub fn source_filename(self) -> Option<core::cell::Ref<'ctx, str>> {
        self.core.source_filename()
    }

    /// Parsed data layout.
    #[inline]
    pub fn data_layout(self) -> core::cell::Ref<'ctx, DataLayout> {
        self.core.data_layout()
    }

    /// Target triple directive.
    #[inline]
    pub fn target_triple(self) -> Option<String> {
        self.core.target_triple()
    }

    /// Module-level inline assembly.
    #[inline]
    pub fn module_asm(self) -> String {
        self.core.module_asm()
    }

    #[inline]
    pub(super) fn metadata_store(self) -> core::cell::Ref<'ctx, MetadataStore> {
        self.core.metadata_store()
    }

    // ---- Type constructors ----
    //
    // Constructing a type is preservation-**neutral**: it interns into the
    // context's type table and touches no function, block, or global, so it can
    // invalidate no analysis. That is why the read-only view carries the same
    // constructor set as the [`Module<Unverified>`] token — code holding only a
    // view (a pass at any capability rung, for instance) can still name the
    // types it needs. *Declarations* — [`Module::add_global`],
    // [`Module::add_function_dyn`], [`Module::set_struct_body`] and friends —
    // are module-structural and deliberately stay on the token alone.

    /// `void`.
    #[inline]
    pub fn void_type(self) -> VoidType<'ctx, B> {
        VoidType::new(self.core.ctx.void(), ModuleRef::new(self.core))
    }

    /// `label`.
    #[inline]
    pub fn label_type(self) -> LabelType<'ctx, B> {
        LabelType::new(self.core.ctx.label(), ModuleRef::new(self.core))
    }

    /// `metadata`.
    #[inline]
    pub fn metadata_type(self) -> MetadataType<'ctx, B> {
        MetadataType::new(self.core.ctx.metadata(), ModuleRef::new(self.core))
    }

    /// `token`.
    #[inline]
    pub fn token_type(self) -> TokenType<'ctx, B> {
        TokenType::new(self.core.ctx.token(), ModuleRef::new(self.core))
    }

    /// `half`.
    #[inline]
    pub fn half_type(self) -> FloatType<'ctx, Half, B> {
        FloatType::new(self.core.ctx.half(), ModuleRef::new(self.core))
    }

    /// `bfloat`.
    #[inline]
    pub fn bfloat_type(self) -> FloatType<'ctx, Bfloat, B> {
        FloatType::new(self.core.ctx.bfloat(), ModuleRef::new(self.core))
    }

    /// `float` (32-bit IEEE 754).
    #[inline]
    pub fn f32_type(self) -> FloatType<'ctx, f32, B> {
        FloatType::new(self.core.ctx.float(), ModuleRef::new(self.core))
    }

    /// `double` (64-bit IEEE 754).
    #[inline]
    pub fn f64_type(self) -> FloatType<'ctx, f64, B> {
        FloatType::new(self.core.ctx.double(), ModuleRef::new(self.core))
    }

    /// `fp128`.
    #[inline]
    pub fn fp128_type(self) -> FloatType<'ctx, Fp128, B> {
        FloatType::new(self.core.ctx.fp128(), ModuleRef::new(self.core))
    }

    /// `x86_fp80`.
    #[inline]
    pub fn x86_fp80_type(self) -> FloatType<'ctx, X86Fp80, B> {
        FloatType::new(self.core.ctx.x86_fp80(), ModuleRef::new(self.core))
    }

    /// `ppc_fp128`.
    #[inline]
    pub fn ppc_fp128_type(self) -> FloatType<'ctx, PpcFp128, B> {
        FloatType::new(self.core.ctx.ppc_fp128(), ModuleRef::new(self.core))
    }

    /// `x86_amx`.
    #[inline]
    pub fn x86_amx_type(self) -> Type<'ctx, B> {
        Type::new(self.core.ctx.x86_amx(), ModuleRef::new(self.core))
    }

    /// `exnref`.
    #[inline]
    pub fn wasm_exnref_type(self) -> Type<'ctx, B> {
        Type::new(self.core.ctx.wasm_exnref(), ModuleRef::new(self.core))
    }

    /// `i1`.
    #[inline]
    pub fn bool_type(self) -> IntType<'ctx, bool, B> {
        IntType::new(self.core.ctx.int_type(1), ModuleRef::new(self.core))
    }

    /// Alias for [`Self::bool_type`].
    #[inline]
    pub fn i1_type(self) -> IntType<'ctx, bool, B> {
        self.bool_type()
    }

    /// `i8`.
    #[inline]
    pub fn i8_type(self) -> IntType<'ctx, i8, B> {
        IntType::new(self.core.ctx.int_type(8), ModuleRef::new(self.core))
    }

    /// `i16`.
    #[inline]
    pub fn i16_type(self) -> IntType<'ctx, i16, B> {
        IntType::new(self.core.ctx.int_type(16), ModuleRef::new(self.core))
    }

    /// `i32`.
    #[inline]
    pub fn i32_type(self) -> IntType<'ctx, i32, B> {
        IntType::new(self.core.ctx.int_type(32), ModuleRef::new(self.core))
    }

    /// `i64`.
    #[inline]
    pub fn i64_type(self) -> IntType<'ctx, i64, B> {
        IntType::new(self.core.ctx.int_type(64), ModuleRef::new(self.core))
    }

    /// `i128`.
    #[inline]
    pub fn i128_type(self) -> IntType<'ctx, i128, B> {
        IntType::new(self.core.ctx.int_type(128), ModuleRef::new(self.core))
    }

    /// Run-time-width integer type. Errors when `bits` falls outside
    /// `MIN_INT_BITS..=MAX_INT_BITS`.
    #[inline]
    pub fn custom_width_int_type(self, bits: u32) -> IrResult<IntType<'ctx, IntDyn, B>> {
        if !(MIN_INT_BITS..=MAX_INT_BITS).contains(&bits) {
            return Err(IrError::InvalidIntegerWidth { bits });
        }
        Ok(IntType::new(
            self.core.ctx.int_type(bits),
            ModuleRef::new(self.core),
        ))
    }

    /// Const-generic integer type. Const-evaluated range check at
    /// monomorphisation: `N` outside `MIN_INT_BITS..=MAX_INT_BITS` is a
    /// compile error.
    #[inline]
    pub fn int_type_n<const N: u32>(self) -> IntType<'ctx, Width<N>, B> {
        const {
            assert!(
                N >= MIN_INT_BITS && N <= MAX_INT_BITS,
                "integer width N outside [MIN_INT_BITS, MAX_INT_BITS]",
            );
        }
        IntType::new(self.core.ctx.int_type(N), ModuleRef::new(self.core))
    }

    /// Opaque pointer in address space `addr_space` (`0` = default).
    #[inline]
    pub fn ptr_type(self, addr_space: u32) -> PointerType<'ctx, B> {
        PointerType::new(
            self.core.ctx.ptr_type(addr_space),
            ModuleRef::new(self.core),
        )
    }

    /// Legacy typed pointer `T*` in address space `addr_space`.
    #[inline]
    pub fn typed_pointer_type<T>(self, pointee: T, addr_space: u32) -> TypedPointerType<'ctx, B>
    where
        T: Into<Type<'ctx, B>>,
    {
        let pointee_id = pointee.into().id();
        TypedPointerType::new(
            self.core.ctx.typed_pointer_type(pointee_id, addr_space),
            ModuleRef::new(self.core),
        )
    }

    /// `[n x elem]`.
    #[inline]
    pub fn array_type<T>(self, elem: T, n: u64) -> ArrayType<'ctx, ElemDyn, ArrLenDyn, B>
    where
        T: Into<Type<'ctx, B>>,
    {
        let elem_id = elem.into().id();
        ArrayType::new(
            self.core.ctx.array_type(elem_id, n),
            ModuleRef::new(self.core),
        )
    }

    /// Const-generic typed array `[N x E]`. The element marker `E` projects
    /// the scalar element type and `N` pins the element count. Unlike
    /// [`vector_type_n`](Self::vector_type_n), `N == 0` is **not** rejected:
    /// LLVM permits zero-length arrays `[0 x T]`.
    #[inline]
    pub fn array_type_n<E, const N: u64>(self) -> ArrayType<'ctx, E, ArrLen<N>, B>
    where
        E: StaticVecElem<'ctx, B>,
    {
        let elem = E::element_ir_type(ModuleRef::new(self.core));
        let id = self.core.ctx.array_type(elem.id(), N);
        ArrayType::new(id, ModuleRef::new(self.core))
    }

    /// Fixed `<n x elem>` vector. Mirrors `FixedVectorType::get`.
    #[inline]
    pub fn vector_type<T>(self, elem: T, n: u32) -> VectorType<'ctx, ElemDyn, LenDyn, B>
    where
        T: Into<Type<'ctx, B>>,
    {
        let id = self.core.ctx.fixed_vector_type(elem.into().id(), n);
        VectorType::new(id, ModuleRef::new(self.core))
    }

    /// Scalable `<vscale x n x elem>` vector. Mirrors
    /// `ScalableVectorType::get`.
    #[inline]
    pub fn scalable_vector_type<T>(self, elem: T, n: u32) -> VectorType<'ctx, ElemDyn, LenDyn, B>
    where
        T: Into<Type<'ctx, B>>,
    {
        let id = self.core.ctx.scalable_vector_type(elem.into().id(), n);
        VectorType::new(id, ModuleRef::new(self.core))
    }

    /// Const-generic typed vector `<N x E>`. The element marker `E` projects
    /// the scalar element type and `N` pins the lane count.
    /// `const`-evaluated at monomorphisation: `N == 0` is a compile error.
    #[inline]
    pub fn vector_type_n<E, const N: u32>(self) -> VectorType<'ctx, E, Len<N>, B>
    where
        E: StaticVecElem<'ctx, B>,
    {
        const {
            assert!(N > 0, "vector length must be >= 1");
        }
        let elem = E::element_ir_type(ModuleRef::new(self.core));
        let id = self.core.ctx.fixed_vector_type(elem.id(), N);
        VectorType::new(id, ModuleRef::new(self.core))
    }

    /// Literal (unnamed) struct type `{ .. }`.
    #[inline]
    pub fn struct_type<I, T>(self, elements: I) -> StructType<'ctx, StructBodyDyn, B>
    where
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx, B>>,
    {
        self.literal_struct_type(elements, false)
    }

    /// Packed literal struct type `<{ .. }>`.
    #[inline]
    pub fn packed_struct_type<I, T>(self, elements: I) -> StructType<'ctx, StructBodyDyn, B>
    where
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx, B>>,
    {
        self.literal_struct_type(elements, true)
    }

    #[inline]
    fn literal_struct_type<I, T>(
        self,
        elements: I,
        packed: bool,
    ) -> StructType<'ctx, StructBodyDyn, B>
    where
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx, B>>,
    {
        let elems: Box<[TypeSlot]> = elements.into_iter().map(|t| t.into().id()).collect();
        StructType::new(
            self.core.ctx.literal_struct_type(elems, packed),
            ModuleRef::new(self.core),
        )
    }

    /// Function type `ret (params...)`.
    #[inline]
    pub fn function_type<I, R, T>(self, return_type: R, parameters: I) -> FunctionType<'ctx, B>
    where
        I: IntoIterator<Item = T>,
        R: Into<Type<'ctx, B>>,
        T: Into<Type<'ctx, B>>,
    {
        self.raw_function_type(return_type, parameters, false)
    }

    /// Variadic function type `ret (params..., ...)`.
    #[inline]
    pub fn variadic_function_type<I, R, T>(
        self,
        return_type: R,
        parameters: I,
    ) -> FunctionType<'ctx, B>
    where
        I: IntoIterator<Item = T>,
        R: Into<Type<'ctx, B>>,
        T: Into<Type<'ctx, B>>,
    {
        self.raw_function_type(return_type, parameters, true)
    }

    #[inline]
    fn raw_function_type<I, R, T>(
        self,
        return_type: R,
        parameters: I,
        is_var_arg: bool,
    ) -> FunctionType<'ctx, B>
    where
        I: IntoIterator<Item = T>,
        R: Into<Type<'ctx, B>>,
        T: Into<Type<'ctx, B>>,
    {
        let ret = return_type.into();
        let params: Box<[TypeSlot]> = parameters.into_iter().map(|t| t.into().id()).collect();
        FunctionType::new(
            self.core.ctx.function_type(ret.id(), params, is_var_arg),
            ModuleRef::new(self.core),
        )
    }

    /// A function type with no parameters. Avoids the empty-iterator
    /// inference cliff of [`function_type`](Self::function_type): with an
    /// empty iterator the element type `T` cannot be inferred, so callers
    /// otherwise have to spell it. This pins the element type for them —
    /// `function_type_no_parameters(ret)` is exactly
    /// `function_type(ret, [] as [Type; 0])`.
    #[inline]
    pub fn function_type_no_parameters<R>(self, return_type: R) -> FunctionType<'ctx, B>
    where
        R: Into<Type<'ctx, B>>,
    {
        self.function_type(return_type, core::iter::empty::<Type<'ctx, B>>())
    }

    /// Variadic sibling of
    /// [`function_type_no_parameters`](Self::function_type_no_parameters):
    /// `ret (...)`.
    #[inline]
    pub fn variadic_function_type_no_parameters<R>(self, return_type: R) -> FunctionType<'ctx, B>
    where
        R: Into<Type<'ctx, B>>,
    {
        self.variadic_function_type(return_type, core::iter::empty::<Type<'ctx, B>>())
    }

    /// Get or create the identified struct type `%name`, leaving its body
    /// unset. Pure type interning, so it belongs to the same
    /// preservation-neutral family as the primitive constructors above.
    #[inline]
    pub fn get_or_insert_named_struct(self, name: &str) -> StructType<'ctx, StructBodyDyn, B> {
        let (id, _existed) = self.core.ctx.get_or_create_named_struct(name);
        StructType::new(id, ModuleRef::new(self.core))
    }

    /// A fresh identified struct with no name — `%0 = type { i32 }`.
    /// Mirrors `StructType::create(Context)` called without a name.
    ///
    /// Never uniqued: two calls give two distinct types even at the same
    /// body, which is what makes `%0` and `%1` different types when both are
    /// written `type { i32 }`. Body-setting goes through the same
    /// `set_struct_body` path as a named one.
    #[inline]
    pub fn anonymous_identified_struct(self) -> StructType<'ctx, StructBodyDyn, B> {
        let id = self.core.ctx.create_anonymous_identified_struct();
        StructType::new(id, ModuleRef::new(self.core))
    }

    /// Look up an existing identified struct type by name, or `None`.
    #[inline]
    pub fn named_struct(self, name: &str) -> Option<StructType<'ctx, StructBodyDyn, B>> {
        self.core
            .ctx
            .get_named_struct(name)
            .map(|id| StructType::new(id, ModuleRef::new(self.core)))
    }

    /// Idempotently intern the named LLVM struct type described by schema `S`,
    /// filling its body the first time.
    ///
    /// This lives on the read-only view for the same reason the primitive
    /// constructors above do: it is preservation-**neutral**. It interns into
    /// the context's type table and either fills a body that was empty or
    /// confirms one that already matches — it never touches a function, block,
    /// or global, and a mismatch is refused with
    /// [`IrError::StructBodyMismatch`] rather than rewriting anything. That is
    /// what lets [`crate::StructSchema`] and [`crate::IrField`] be declared
    /// against a `ModuleView` instead of a `&Module<Unverified>` token —
    /// which in turn is what lets [`crate::IrBuilder`] answer schema queries
    /// without fabricating a module token it does not own.
    ///
    /// The *typestate* body setters — [`Module::set_struct_body`] and
    /// [`Module::set_struct_body_dyn`], which drive an `Opaque` struct handle
    /// to `BodySet` — stay on the token alone.
    pub fn get_or_insert_struct_of<S>(self) -> IrResult<StructType<'ctx, BodySet, B>>
    where
        S: StructSchema,
    {
        if S::NAME.is_empty() {
            return Err(IrError::InvalidOperation {
                message: "struct schema name must not be empty",
            });
        }
        let field_types = S::field_types(self)?;
        let elements: Box<[TypeSlot]> = field_types.iter().map(|t| t.id()).collect();
        let (id, _existed) = self.core.ctx.get_or_create_named_struct(S::NAME);
        let data = self
            .core
            .ctx
            .type_data(id)
            .as_struct()
            .unwrap_or_else(|| unreachable!("named struct id stores struct data"));
        {
            let body = data.body.borrow();
            if let Some(body) = body.as_ref() {
                if body.packed == S::PACKED && body.elements.as_ref() == elements.as_ref() {
                    return Ok(StructType::<BodySet, B>::new(id, ModuleRef::new(self.core)));
                }
                return Err(IrError::StructBodyMismatch {
                    name: S::NAME.to_owned(),
                });
            }
        }
        self.core.ctx.set_named_struct_body(
            id,
            StructBody {
                elements,
                packed: S::PACKED,
            },
        )?;
        Ok(StructType::<BodySet, B>::new(id, ModuleRef::new(self.core)))
    }

    /// Target extension type `target("name", type_params..., int_params...)`.
    #[inline]
    pub fn target_ext_type<Name, I, T, J>(
        self,
        name: Name,
        type_params: I,
        int_params: J,
    ) -> TargetExtType<'ctx, B>
    where
        Name: Into<String>,
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx, B>>,
        J: IntoIterator<Item = u32>,
    {
        let name: String = name.into();
        let type_params: Box<[TypeSlot]> = type_params.into_iter().map(|t| t.into().id()).collect();
        let int_params: Box<[u32]> = int_params.into_iter().collect();
        TargetExtType::new(
            self.core.ctx.target_ext_type(name, type_params, int_params),
            ModuleRef::new(self.core),
        )
    }

    /// Iterate functions in declaration order.
    #[inline]
    pub fn functions(
        self,
    ) -> impl ExactSizeIterator<Item = FunctionView<'ctx, B>> + DoubleEndedIterator + FusedIterator + 'ctx
    {
        self.core.iter_functions::<B>().map(FunctionView::new)
    }

    /// Iterate globals in declaration order.
    #[inline]
    pub fn globals(
        self,
    ) -> impl ExactSizeIterator<Item = GlobalVariableView<'ctx, B>>
    + DoubleEndedIterator
    + FusedIterator
    + 'ctx {
        self.core.iter_globals::<B>().map(GlobalVariableView::new)
    }

    /// Iterate aliases in declaration order.
    #[inline]
    pub fn aliases(
        self,
    ) -> impl ExactSizeIterator<Item = GlobalAliasView<'ctx, B>>
    + DoubleEndedIterator
    + FusedIterator
    + 'ctx {
        self.core.iter_aliases::<B>().map(GlobalAliasView::new)
    }

    /// Iterate ifuncs in declaration order.
    #[inline]
    pub fn ifuncs(
        self,
    ) -> impl ExactSizeIterator<Item = GlobalIfuncView<'ctx, B>>
    + DoubleEndedIterator
    + FusedIterator
    + 'ctx {
        self.core.iter_ifuncs::<B>().map(GlobalIfuncView::new)
    }

    /// Iterate COMDATs in insertion order.
    #[inline]
    pub fn comdats(
        self,
    ) -> impl ExactSizeIterator<Item = ComdatView<'ctx, B>> + DoubleEndedIterator + FusedIterator + 'ctx
    {
        self.core.iter_comdats::<B>().map(ComdatView::new)
    }
}

/// Iterating a module view yields its **functions** in declaration order —
/// matching LLVM's `for (Function &F : M)` — not its globals: functions are
/// the walk an optimizer loop wants, and globals/aliases/ifuncs/COMDATs keep
/// their named iterators ([`ModuleView::globals`], [`ModuleView::aliases`],
/// [`ModuleView::ifuncs`], [`ModuleView::comdats`]). Sugar beside the named
/// [`ModuleView::functions`], not a replacement.
///
/// One capability differs: this iterator is not [`DoubleEndedIterator`],
/// because it boxes its inner iterator to name a single concrete type. For
/// reverse iteration go through the named method — `functions().rev()`.
impl<'ctx, B: ModuleBrand + 'ctx> IntoIterator for ModuleView<'ctx, B> {
    type Item = FunctionView<'ctx, B>;
    type IntoIter = ModuleFunctionViews<'ctx, B>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        ModuleFunctionViews::new(self)
    }
}

impl<'ctx, B: ModuleBrand> From<ModuleView<'ctx, B>> for ModuleRef<'ctx, B> {
    #[inline]
    fn from(view: ModuleView<'ctx, B>) -> Self {
        ModuleRef::new(view.core)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, S> From<&'ctx Module<B, S>> for ModuleView<'ctx, B> {
    #[inline]
    fn from(module: &'ctx Module<B, S>) -> Self {
        module.as_view()
    }
}

impl<B: ModuleBrand> PartialEq for ModuleView<'_, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.core.id == other.core.id
    }
}

impl<B: ModuleBrand> Eq for ModuleView<'_, B> {}

impl<B: ModuleBrand> Hash for ModuleView<'_, B> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.core.id.hash(state);
    }
}

impl<B: ModuleBrand> core::fmt::Debug for ModuleView<'_, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ModuleView")
            .field("id", &self.core.id)
            .field("name", &self.core.name)
            .finish()
    }
}

impl<B: ModuleBrand> core::fmt::Display for ModuleView<'_, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::asm_writer::fmt_module(f, self.core)
    }
}

// --------------------------------------------------------------------------
// Module
// --------------------------------------------------------------------------

/// Structured `uselistorder Type Value, { ... }` record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UseListOrderRecord {
    value: ValueSlot,
    value_ty: TypeSlot,
    indexes: Box<[u32]>,
}

impl UseListOrderRecord {
    pub fn new<Indexes>(value: ValueSlot, value_ty: TypeSlot, indexes: Indexes) -> IrResult<Self>
    where
        Indexes: Into<Box<[u32]>>,
    {
        let indexes = indexes.into();
        validate_use_list_order_indexes(&indexes)?;
        Ok(Self {
            value,
            value_ty,
            indexes,
        })
    }

    pub fn value(&self) -> ValueSlot {
        self.value
    }

    pub fn value_type(&self) -> TypeSlot {
        self.value_ty
    }

    pub fn indexes(&self) -> &[u32] {
        &self.indexes
    }
}

/// Structured `uselistorder_bb @function, %block, { ... }` record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UseListOrderBbRecord {
    function: ValueSlot,
    block: ValueSlot,
    indexes: Box<[u32]>,
}

impl UseListOrderBbRecord {
    pub fn new<Indexes>(function: ValueSlot, block: ValueSlot, indexes: Indexes) -> IrResult<Self>
    where
        Indexes: Into<Box<[u32]>>,
    {
        let indexes = indexes.into();
        validate_use_list_order_indexes(&indexes)?;
        Ok(Self {
            function,
            block,
            indexes,
        })
    }

    pub fn function(&self) -> ValueSlot {
        self.function
    }

    pub fn block(&self) -> ValueSlot {
        self.block
    }

    pub fn indexes(&self) -> &[u32] {
        &self.indexes
    }
}

pub(super) fn validate_use_list_order_indexes(indexes: &[u32]) -> IrResult<()> {
    let identity = indexes
        .iter()
        .enumerate()
        .all(|(i, idx)| u32::try_from(i).is_ok_and(|i| i == *idx));
    if identity {
        return Err(IrError::InvalidOperation {
            message: "expected uselistorder indexes to change the order",
        });
    }
    Ok(())
}

/// Top-level IR container.
pub(super) struct ModuleCore {
    id: ModuleId,
    name: String,
    /// `source_filename = "..."` directive. Optional; upstream stores an
    /// empty string for absence, but `Option` keeps the missing directive
    /// explicit on the Rust side.
    source_filename: core::cell::RefCell<Option<String>>,
    ctx: Context,
    /// Functions defined in this module, in declaration order.
    /// Stored as a `RefCell<Vec<ValueSlot>>` so the function-declaring
    /// constructors can mutate while the same `&'ctx self` borrow is
    /// held by call sites.
    functions: core::cell::RefCell<Vec<ValueSlot>>,
    /// Module-level name -> function value-id table.
    function_by_name: core::cell::RefCell<std::collections::HashMap<String, ValueSlot>>,
    /// Globals defined in this module, in declaration order.
    /// Mirrors `Module::GlobalList`. Stored under the same shape as
    /// `functions` so the AsmWriter can iterate in source order.
    globals: core::cell::RefCell<Vec<ValueSlot>>,
    /// Module-level name -> global value-id table.
    global_by_name: core::cell::RefCell<std::collections::HashMap<String, ValueSlot>>,
    aliases: core::cell::RefCell<Vec<ValueSlot>>,
    alias_by_name: core::cell::RefCell<std::collections::HashMap<String, ValueSlot>>,
    ifuncs: core::cell::RefCell<Vec<ValueSlot>>,
    ifunc_by_name: core::cell::RefCell<std::collections::HashMap<String, ValueSlot>>,
    /// Module-level COMDAT entries. Mirrors `Module::ComdatSymTab`.
    /// Stored in a `boxcar::Vec` for stable `&ComdatData` references
    /// under `&self`, so [`ComdatRef`](ComdatRef) can
    /// hand out borrows without runtime cell juggling.
    comdats: boxcar::Vec<ComdatData>,
    /// Name -> comdat-id table. Mirrors
    /// `Module::ComdatSymTab` lookup.
    comdat_by_name: core::cell::RefCell<std::collections::HashMap<String, ComdatId>>,
    /// Parsed `target datalayout = "..."` directive. Default
    /// (empty string) when the module has no directive. Mirrors
    /// `Module::DL` in `IR/Module.h`.
    data_layout: core::cell::RefCell<DataLayout>,
    /// `target triple = "..."` directive. Optional.
    target_triple: core::cell::RefCell<Option<String>>,
    /// Module-level inline assembly. Mirrors `Module::ModuleAsm`.
    /// Stored as a single `String` joined by newlines (one entry
    /// per `module asm "..."` directive).
    module_asm: core::cell::RefCell<String>,
    use_list_orders: core::cell::RefCell<Vec<UseListOrderRecord>>,
    attribute_groups: core::cell::RefCell<Vec<(u32, AttributeStorage)>>,
    use_list_order_bbs: core::cell::RefCell<Vec<UseListOrderBbRecord>>,
    /// Module-level metadata node arena. Mirrors `LLVMContextImpl`'s
    /// metadata store (scoped to the module for simplicity).
    metadata: core::cell::RefCell<MetadataStore>,
    /// Named metadata nodes (`!llvm.module.flags`, `!llvm.ident`, ...).
    /// Mirrors `Module::NamedMDList`. Insertion order is preserved.
    named_metadata: core::cell::RefCell<Vec<NamedMetadataNode<StoredBrand>>>,
    /// Uniquing cache for [`metadata_as_value`](Self::metadata_as_value):
    /// maps a metadata node to its wrapping value so repeated wraps of the
    /// same node return the identical `Value`. Mirrors LLVM's uniqued
    /// `MetadataAsValue::get`.
    metadata_as_value_cache:
        core::cell::RefCell<std::collections::HashMap<MetadataSlot, ValueSlot>>,
    /// Monotonic id source for [`crate::ssa_builder::SsaBuilder`] instances
    /// created against this module. Mirrors the module-scoped counter shape
    /// of [`ModuleId::fresh`], but per-module (an `SsaBuilderId` only needs
    /// to disambiguate builders within one module, not process-globally).
    next_ssa_builder_id: core::cell::Cell<u32>,
}

/// Linear module token carrying a brand `B` and verification state `S`.
///
/// The token **owns** its `ModuleCore` and borrows nothing, so it has no
/// lifetime parameter at all: a `Module` *is* the storage, not a borrow of
/// storage parked in some caller's frame. That is what lets the token be moved
/// — returned from a builder function, stored in a struct field, collected into
/// a `Vec`, sent across a thread boundary — instead of being pinned to the
/// stack frame that created it.
///
/// Handles are the borrowing half: `m.view(id)` hands back a `Value<'a, B>`
/// whose `'a` is a borrow *of this token*, so the borrow checker still knows a
/// live handle keeps the module alive. The region lives on the handle, where it
/// describes a real borrow, rather than on the module, where it described
/// nothing.
///
/// `Module` deliberately has **no** `Drop` impl: the typestate transitions
/// ([`verify`](Self::verify), [`unverify`](Self::unverify)) move the owned core
/// out of `self` into the new-state token, which `Drop` would forbid (E0509).
/// The brand registration still has to be released when the token dies — that
/// `Drop` lives on the brand-registration *field* instead, which is legal
/// precisely because a `Drop` field does not block moving fields out of a
/// non-`Drop` struct.
///
/// # `Send`
///
/// The brand is stored as `Invariant<B>` = `PhantomData<fn(B) -> B>`, which is
/// `Send + Sync` whatever `B` is — so a module is `Send` even under a brand type
/// that is deliberately `!Send`. `ModuleCore`'s interior mutability is `RefCell`
/// (not `Sync`), so a module is `Send` but **not** `Sync`: it moves between
/// threads, it is not shared between them.
pub struct Module<B: ModuleBrand, S = Unverified> {
    core: Box<ModuleCore>,
    /// Live claim on `B` in the process-global registry, for the tokens that
    /// hold one. `None` for registry-exempt tokens: [`Module::dynamic`]
    /// ([`DynBrand`]).
    ///
    /// Moved along by every typestate transition, so a claim is released
    /// exactly once — when the last token over this module is dropped.
    registration: Option<BrandGuard<B>>,
    _brand: Invariant<B>,
    _state: PhantomData<S>,
}

/// A **summary**, deliberately not the module's IR.
///
/// [`Display`](core::fmt::Display) already prints the whole `.ll` file; a `Debug`
/// that forwarded to it would splice thousands of lines into every `dbg!`,
/// every `assert_eq!` failure on a struct that happens to hold a module, and
/// every `{:?}` on a `Result<Module<B>, _>`. So this prints the identity and
/// the shape — name, id, counts, verification state — and nothing that grows
/// with the IR.
///
/// The `S: 'static` bound is what lets the typestate be *named*: both
/// typestate markers are `'static` uninhabited enums, so a [`TypeId`]
/// comparison recovers which one is in play without a new trait or an `as`
/// cast.
///
/// [`TypeId`]: core::any::TypeId
impl<B: ModuleBrand, S: 'static> core::fmt::Debug for Module<B, S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use core::any::TypeId;

        let state = if TypeId::of::<S>() == TypeId::of::<Verified>() {
            "Verified"
        } else if TypeId::of::<S>() == TypeId::of::<Unverified>() {
            "Unverified"
        } else {
            // Unreachable in practice — no constructor mints a module in any
            // other typestate — but a `Debug` impl must never panic, so name
            // the type rather than assert about it.
            core::any::type_name::<S>()
        };

        // `try_borrow`, not `borrow`: printing a module from inside a mutator
        // that is holding one of these lists open must degrade to a marker,
        // never panic. `Debug` is what a user reaches for *while* debugging.
        struct Count(Option<usize>);
        impl core::fmt::Debug for Count {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                match self.0 {
                    Some(n) => core::fmt::Debug::fmt(&n, f),
                    None => f.write_str("<borrowed>"),
                }
            }
        }
        fn count<T>(cell: &core::cell::RefCell<Vec<T>>) -> Count {
            Count(cell.try_borrow().ok().map(|list| list.len()))
        }

        f.debug_struct("Module")
            .field("name", &self.core.name())
            .field("id", &self.core.id())
            .field("functions", &count(&self.core.functions))
            .field("globals", &count(&self.core.globals))
            .field("state", &state)
            .finish()
    }
}

impl<'ctx> ModuleCore {
    /// Construct a fresh, empty module with a freshly-allocated
    /// [`ModuleId`].
    pub(super) fn new<N>(name: N) -> Self
    where
        N: Into<String>,
    {
        Self {
            id: ModuleId::fresh(),
            name: name.into(),
            source_filename: core::cell::RefCell::new(None),
            ctx: Context::new(),
            functions: core::cell::RefCell::new(Vec::new()),
            function_by_name: core::cell::RefCell::new(std::collections::HashMap::new()),
            globals: core::cell::RefCell::new(Vec::new()),
            global_by_name: core::cell::RefCell::new(std::collections::HashMap::new()),
            aliases: core::cell::RefCell::new(Vec::new()),
            alias_by_name: core::cell::RefCell::new(std::collections::HashMap::new()),
            ifuncs: core::cell::RefCell::new(Vec::new()),
            ifunc_by_name: core::cell::RefCell::new(std::collections::HashMap::new()),
            comdats: boxcar::Vec::new(),
            comdat_by_name: core::cell::RefCell::new(std::collections::HashMap::new()),
            data_layout: core::cell::RefCell::new(DataLayout::default()),
            target_triple: core::cell::RefCell::new(None),
            module_asm: core::cell::RefCell::new(String::new()),
            use_list_orders: core::cell::RefCell::new(Vec::new()),
            use_list_order_bbs: core::cell::RefCell::new(Vec::new()),
            attribute_groups: core::cell::RefCell::new(Vec::new()),
            metadata: core::cell::RefCell::new(MetadataStore::default()),
            named_metadata: core::cell::RefCell::new(Vec::new()),
            metadata_as_value_cache: core::cell::RefCell::new(std::collections::HashMap::new()),
            next_ssa_builder_id: core::cell::Cell::new(0),
        }
    }

    /// Module identifier (the human-readable name).
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }
    /// `source_filename = "..."` directive. Mirrors
    /// `Module::getSourceFileName`.
    pub fn source_filename(&self) -> Option<core::cell::Ref<'_, str>> {
        core::cell::Ref::filter_map(self.source_filename.borrow(), Option::as_deref).ok()
    }

    /// Set the `source_filename` directive. Mirrors
    /// `Module::setSourceFileName`.
    pub fn set_source_filename<Filename>(&self, filename: Filename)
    where
        Filename: Into<String>,
    {
        *self.source_filename.borrow_mut() = Some(filename.into());
    }

    /// Clear the `source_filename` directive.
    pub fn clear_source_filename(&self) {
        *self.source_filename.borrow_mut() = None;
    }

    /// This module's globally-unique id.
    #[inline]
    pub fn id(&self) -> ModuleId {
        self.id
    }

    /// Crate-internal access to the interning context.
    #[inline]
    pub(super) fn context(&self) -> &Context {
        &self.ctx
    }

    /// Allocate the next per-module [`crate::ssa_builder::SsaBuilderId`].
    /// Fetch-and-increment, like the other id counters in this file.
    #[inline]
    pub(super) fn next_ssa_builder_id(&self) -> u32 {
        let id = self.next_ssa_builder_id.get();
        self.next_ssa_builder_id.set(id + 1);
        id
    }

    /// Named-struct type ids in declaration order. The printer turns each
    /// into a [`Type`](crate::r#type::Type) via `Type::new(id, self)` to emit
    /// the `%Name = type {...}` identity block.
    #[inline]
    pub(super) fn iter_named_struct_ids(&self) -> Vec<TypeSlot> {
        self.ctx.iter_named_structs()
    }

    // ---- Primitive type constructors ----

    /// `void`.
    pub fn void_type<B: ModuleBrand + 'ctx>(&'ctx self) -> VoidType<'ctx, B> {
        VoidType::new(self.ctx.void(), self)
    }

    /// `label`.
    pub fn label_type<B: ModuleBrand + 'ctx>(&'ctx self) -> LabelType<'ctx, B> {
        LabelType::new(self.ctx.label(), self)
    }

    /// `token`.
    pub fn token_type<B: ModuleBrand + 'ctx>(&'ctx self) -> TokenType<'ctx, B> {
        TokenType::new(self.ctx.token(), self)
    }

    /// `half`.
    pub fn half_type<B: ModuleBrand + 'ctx>(&'ctx self) -> FloatType<'ctx, Half, B> {
        FloatType::new(self.ctx.half(), self)
    }

    /// `bfloat`.
    pub fn bfloat_type<B: ModuleBrand + 'ctx>(&'ctx self) -> FloatType<'ctx, Bfloat, B> {
        FloatType::new(self.ctx.bfloat(), self)
    }

    /// `float` (32-bit IEEE 754).
    pub fn f32_type<B: ModuleBrand + 'ctx>(&'ctx self) -> FloatType<'ctx, f32, B> {
        FloatType::new(self.ctx.float(), self)
    }

    /// `double` (64-bit IEEE 754).
    pub fn f64_type<B: ModuleBrand + 'ctx>(&'ctx self) -> FloatType<'ctx, f64, B> {
        FloatType::new(self.ctx.double(), self)
    }

    /// `fp128` (128-bit IEEE 754 binary128).
    pub fn fp128_type<B: ModuleBrand + 'ctx>(&'ctx self) -> FloatType<'ctx, Fp128, B> {
        FloatType::new(self.ctx.fp128(), self)
    }

    /// `x86_fp80` (80-bit X87 extended precision).
    pub fn x86_fp80_type<B: ModuleBrand + 'ctx>(&'ctx self) -> FloatType<'ctx, X86Fp80, B> {
        FloatType::new(self.ctx.x86_fp80(), self)
    }

    /// `ppc_fp128` (PowerPC double-double).
    pub fn ppc_fp128_type<B: ModuleBrand + 'ctx>(&'ctx self) -> FloatType<'ctx, PpcFp128, B> {
        FloatType::new(self.ctx.ppc_fp128(), self)
    }

    // ---- Integer types ----

    /// `i1`. Convenience for [`Self::custom_width_int_type`] with `bits = 1`.
    pub fn bool_type<B: ModuleBrand + 'ctx>(&'ctx self) -> IntType<'ctx, bool, B> {
        IntType::new(self.ctx.int_type(1), self)
    }
    pub fn i8_type<B: ModuleBrand + 'ctx>(&'ctx self) -> IntType<'ctx, i8, B> {
        IntType::new(self.ctx.int_type(8), self)
    }
    pub fn i16_type<B: ModuleBrand + 'ctx>(&'ctx self) -> IntType<'ctx, i16, B> {
        IntType::new(self.ctx.int_type(16), self)
    }
    pub fn i32_type<B: ModuleBrand + 'ctx>(&'ctx self) -> IntType<'ctx, i32, B> {
        IntType::new(self.ctx.int_type(32), self)
    }
    pub fn i64_type<B: ModuleBrand + 'ctx>(&'ctx self) -> IntType<'ctx, i64, B> {
        IntType::new(self.ctx.int_type(64), self)
    }
    pub fn i128_type<B: ModuleBrand + 'ctx>(&'ctx self) -> IntType<'ctx, i128, B> {
        IntType::new(self.ctx.int_type(128), self)
    }

    /// Const-generic integer type. Returns [`IntType<'ctx, Width<N>>`](
    /// crate::Width). Const-evaluated range check at monomorphisation:
    /// `N` outside `MIN_INT_BITS..=MAX_INT_BITS` is a compile error.
    /// Mirrors `Type::getIntNTy(C, N)`.
    pub fn int_type_n<const N: u32, B: ModuleBrand + 'ctx>(
        &'ctx self,
    ) -> IntType<'ctx, Width<N>, B> {
        const {
            assert!(
                N >= MIN_INT_BITS && N <= MAX_INT_BITS,
                "integer width N outside [MIN_INT_BITS, MAX_INT_BITS]",
            );
        }
        IntType::new(self.ctx.int_type(N), self)
    }

    // ---- Pointer / typed-pointer ----

    /// Opaque pointer in address space `addr_space` (`0` = default).
    pub fn ptr_type<B: ModuleBrand + 'ctx>(&'ctx self, addr_space: u32) -> PointerType<'ctx, B> {
        PointerType::new(self.ctx.ptr_type(addr_space), self)
    }

    // ---- Array / vector ----

    /// Fixed `<N x T>` or scalable `<vscale x N x T>` vector.
    pub fn vector_type<B, T>(
        &'ctx self,
        elem: T,
        n: u32,
        scalable: bool,
    ) -> VectorType<'ctx, ElemDyn, LenDyn, B>
    where
        B: ModuleBrand + 'ctx,
        T: Into<Type<'ctx, B>>,
    {
        let elem_id = elem.into().id();
        let id = if scalable {
            self.ctx.scalable_vector_type(elem_id, n)
        } else {
            self.ctx.fixed_vector_type(elem_id, n)
        };
        VectorType::new(id, self)
    }
    // NB: unlike `int_type_n` (which the width-marker projection in
    // `int_width.rs` reaches via `module.module().int_type_n()`), the
    // element-marker projection lives in `element.rs` and does not route
    // through a `ModuleCore` vector constructor, so there is no
    // `ModuleCore::vector_type_n` — the public `Module::vector_type_n`
    // (below) is the only const-generic vector entry point.

    // ---- Function creation ----

    /// Crate-internal CHECKED declaration path for
    /// [`FunctionBuilder::build`](crate::function::FunctionBuilder::build),
    /// the one constructor where a user-supplied signature and an
    /// independently chosen `R` meet. Mirrors `Function::Create`.
    /// Returns `Err(IrError::DuplicateFunctionName)` if a function
    /// of the same name already exists, or
    /// [`IrError::ReturnTypeMismatch`] if the signature's return
    /// type does not match the chosen [`ReturnMarker`](crate::marker::ReturnMarker).
    pub(crate) fn add_function_checked<B: ModuleBrand + 'ctx, R, Name>(
        &'ctx self,
        name: Name,
        signature: FunctionType<'ctx, B>,
        linkage: Linkage,
    ) -> IrResult<FunctionValue<'ctx, R, B>>
    where
        R: ReturnMarker,
        Name: AsRef<str>,
    {
        let name = name.as_ref();
        reject_reserved_intrinsic_name(name)?;
        if !name.is_empty() && self.global_name_exists(name) {
            return Err(IrError::DuplicateFunctionName {
                name: name.to_owned(),
            });
        }
        // Reject the static-marker / signature mismatch up front.
        let ret_data = self.ctx.type_data(signature.return_type().id());
        if !crate::function::signature_matches_marker::<R>(ret_data) {
            return Err(IrError::ReturnTypeMismatch {
                expected: crate::marker::marker_kind_label::<R>()
                    .unwrap_or_else(|| unreachable!("Dyn marker matches every signature")),
                got: signature.return_type().kind_label(),
            });
        }

        self.push_function(
            name,
            signature,
            linkage,
            crate::CallingConv::default(),
            None,
            None,
        )
    }

    fn push_function<B: ModuleBrand + 'ctx, R>(
        &'ctx self,
        name: &str,
        signature: FunctionType<'ctx, B>,
        linkage: Linkage,
        calling_conv: crate::CallingConv,
        intrinsic: Option<IntrinsicFunctionData>,
        attributes: Option<AttributeStorage>,
    ) -> IrResult<FunctionValue<'ctx, R, B>>
    where
        R: ReturnMarker,
    {
        let signature_id = signature.id;

        let fn_data = FunctionData::new(
            name.to_owned(),
            signature_id,
            linkage,
            calling_conv,
            intrinsic,
        );
        let fn_id = self.ctx.push_value(ValueData {
            ty: signature_id,
            name: core::cell::RefCell::new((!name.is_empty()).then(|| name.to_owned())),
            debug_loc: None,
            kind: ValueKindData::Function(Box::new(fn_data)),
            use_list: core::cell::RefCell::new(Vec::new()),
        });

        let param_types: Vec<TypeSlot> = signature.params().map(|t| t.id()).collect();
        let mut arg_ids = Vec::with_capacity(param_types.len());
        for (slot, &ty) in param_types.iter().enumerate() {
            let slot_u32 = u32::try_from(slot)
                .unwrap_or_else(|_| unreachable!("function parameter slot exceeds u32::MAX"));
            let id = self.ctx.push_value(ValueData {
                ty,
                name: core::cell::RefCell::new(None),
                debug_loc: None,
                kind: ValueKindData::Argument {
                    parent_fn: fn_id,
                    slot: slot_u32,
                },
                use_list: core::cell::RefCell::new(Vec::new()),
            });
            arg_ids.push(id);
        }

        let fn_value_data = self.ctx.value_data(fn_id);
        let fn_inner = match &fn_value_data.kind {
            ValueKindData::Function(f) => f,
            _ => unreachable!("function arena push returned the inserted function variant"),
        };
        *fn_inner.args.borrow_mut() = arg_ids.into_boxed_slice();
        if let Some(attributes) = attributes {
            *fn_inner.attributes.borrow_mut() = attributes;
        }

        self.functions.borrow_mut().push(fn_id);
        if !name.is_empty() {
            self.function_by_name
                .borrow_mut()
                .insert(name.to_owned(), fn_id);
        }
        Ok(FunctionValue::<'ctx, R, B>::from_parts_unchecked(
            fn_id,
            ModuleRef::<B>::new(self),
        ))
    }

    pub(crate) fn intrinsic_descriptor_from_signature<B: ModuleBrand + 'ctx>(
        &'ctx self,
        name: &str,
        fn_ty: FunctionType<'ctx, B>,
    ) -> IrResult<IntrinsicDescriptor<'ctx, B>> {
        let id = match resolve_intrinsic_name(name) {
            IntrinsicNameResolution::Known(id) => id,
            IntrinsicNameResolution::UnknownIntrinsic => {
                return Err(IrError::UnknownIntrinsic {
                    name: name.to_owned(),
                });
            }
            IntrinsicNameResolution::NonIntrinsic => {
                return Err(IrError::InvalidOperation {
                    message: "not an intrinsic name",
                });
            }
        };
        let module_ref = ModuleRef::<B>::new(self);
        let descriptor = descriptor_for_name(module_ref, id, name)?;
        let expected = descriptor.function_type_ref(module_ref)?;
        if expected != fn_ty || descriptor.mangled_name()? != name {
            return Err(IrError::IntrinsicSignatureMismatch {
                name: name.to_owned(),
            });
        }
        Ok(descriptor)
    }

    pub(crate) fn get_or_insert_intrinsic_declaration<B: ModuleBrand + 'ctx>(
        &'ctx self,
        descriptor: &IntrinsicDescriptor<'ctx, B>,
    ) -> IrResult<FunctionValue<'ctx, Dyn, B>> {
        let name = descriptor.mangled_name()?;
        let module_ref = ModuleRef::<B>::new(self);
        let signature = descriptor.function_type_ref(module_ref)?;
        if let Some(existing_id) = self.function_by_name.borrow().get(&name).copied() {
            let existing =
                FunctionValue::<'ctx, Dyn, B>::from_parts_unchecked(existing_id, module_ref);
            if existing.signature() != signature
                || existing.basic_blocks().len() != 0
                || existing.intrinsic_descriptor().as_ref() != Some(descriptor)
            {
                return Err(IrError::IntrinsicSignatureMismatch { name });
            }
            return Ok(existing);
        }
        let attributes = descriptor.declaration_attributes(signature)?;
        self.push_function::<B, Dyn>(
            &name,
            signature,
            Linkage::External,
            crate::CallingConv::default(),
            Some(descriptor.to_function_data()),
            Some(attributes),
        )
    }

    pub(crate) fn get_or_insert_intrinsic_declaration_by_name<B: ModuleBrand + 'ctx>(
        &'ctx self,
        name: &str,
    ) -> IrResult<FunctionValue<'ctx, Dyn, B>> {
        let id = IntrinsicId::lookup(name).ok_or_else(|| IrError::UnknownIntrinsic {
            name: name.to_owned(),
        })?;
        let module_ref = ModuleRef::<B>::new(self);
        let descriptor = descriptor_for_name(module_ref, id, name)?;
        self.get_or_insert_intrinsic_declaration(&descriptor)
    }

    /// Iterate the module's functions in declaration order, widened
    /// to [`Dyn`](Dyn). Mirrors `Module::functions`.
    pub fn iter_functions<B: ModuleBrand + 'ctx>(
        &'ctx self,
    ) -> impl ExactSizeIterator<Item = FunctionValue<'ctx, Dyn, B>>
    + DoubleEndedIterator
    + FusedIterator
    + 'ctx {
        let ids: Vec<ValueSlot> = self.functions.borrow().clone();
        ids.into_iter().map(move |id| {
            FunctionValue::<'ctx, Dyn, B>::from_parts_unchecked(id, ModuleRef::<B>::new(self))
        })
    }

    /// Start a [`FunctionBuilder`](FunctionBuilder)
    /// for incremental setup of linkage, calling convention,
    /// `unnamed_addr`, parameter names, and attributes before
    /// materialising the function.
    pub fn function_builder<B: ModuleBrand + 'ctx, R, Name>(
        &'ctx self,
        name: Name,
        signature: FunctionType<'ctx, B>,
    ) -> FunctionBuilder<'ctx, R, B>
    where
        R: ReturnMarker,
        Name: Into<String>,
    {
        FunctionBuilder::new(ModuleRef::<B>::new(self), name, signature)
    }

    // ---- Verification (Phase F) ----

    /// Iterate the module's globals in declaration order. Mirrors
    /// `Module::globals`.
    pub fn iter_globals<B: ModuleBrand + 'ctx>(
        &'ctx self,
    ) -> impl ExactSizeIterator<Item = GlobalVariable<'ctx, B>>
    + DoubleEndedIterator
    + FusedIterator
    + 'ctx {
        let ids: Vec<ValueSlot> = self.globals.borrow().clone();
        ids.into_iter().map(move |id| {
            let value_data = self.ctx.value_data(id);
            GlobalVariable::from_parts_unchecked(id, ModuleRef::<B>::new(self), value_data.ty)
        })
    }

    pub fn iter_aliases<B: ModuleBrand + 'ctx>(
        &'ctx self,
    ) -> impl ExactSizeIterator<Item = GlobalAlias<'ctx, B>> + DoubleEndedIterator + FusedIterator + 'ctx
    {
        let ids: Vec<ValueSlot> = self.aliases.borrow().clone();
        ids.into_iter().map(move |id| {
            let value_data = self.ctx.value_data(id);
            GlobalAlias::from_parts_unchecked(id, ModuleRef::<B>::new(self), value_data.ty)
        })
    }

    pub fn alias_empty(&self) -> bool {
        self.aliases.borrow().is_empty()
    }

    pub fn iter_ifuncs<B: ModuleBrand + 'ctx>(
        &'ctx self,
    ) -> impl ExactSizeIterator<Item = GlobalIfunc<'ctx, B>> + DoubleEndedIterator + FusedIterator + 'ctx
    {
        let ids: Vec<ValueSlot> = self.ifuncs.borrow().clone();
        ids.into_iter().map(move |id| {
            let value_data = self.ctx.value_data(id);
            GlobalIfunc::from_parts_unchecked(id, ModuleRef::<B>::new(self), value_data.ty)
        })
    }

    pub fn ifunc_empty(&self) -> bool {
        self.ifuncs.borrow().is_empty()
    }

    pub fn global_empty(&self) -> bool {
        self.globals.borrow().is_empty()
    }

    /// Crate-internal: install a built [`GlobalBuilder`] into the
    /// module. Performs the duplicate-name check and the comdat
    /// existence check, then pushes to the value arena.
    pub(super) fn install_global_variable<B: ModuleBrand + 'ctx>(
        &'ctx self,
        builder: GlobalBuilder<'ctx, B>,
    ) -> IrResult<GlobalVariable<'ctx, B>> {
        let (name, data, _initializer, address_space, value_type) = builder.into_data();
        if !name.is_empty() && self.global_name_exists(&name) {
            return Err(IrError::DuplicateGlobalName { name });
        }
        let pointer_ty = self.ctx.ptr_type(address_space);
        // Sanity: value_type must already be in the same context. Use
        // the cached id directly. (Construction APIs only hand out
        // typed ids belonging to this module.)
        let _ = value_type;
        let seeded_initializer = data.initializer.get();
        let value_id = self.ctx.push_value(ValueData {
            ty: pointer_ty,
            name: core::cell::RefCell::new((!name.is_empty()).then(|| name.clone())),
            debug_loc: None,
            kind: ValueKindData::GlobalVariable(data),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        // A builder-supplied initializer is a use like any other; the setter
        // path registers its edge, so the construction path must too.
        self.ctx.retarget_global_field_use(
            value_id,
            GlobalFieldKind::Initializer,
            None,
            seeded_initializer,
        );
        self.globals.borrow_mut().push(value_id);
        if !name.is_empty() {
            self.global_by_name.borrow_mut().insert(name, value_id);
        }
        Ok(GlobalVariable::from_parts_unchecked(
            value_id,
            ModuleRef::<B>::new(self),
            pointer_ty,
        ))
    }

    pub(super) fn install_global_alias<B: ModuleBrand + 'ctx>(
        &'ctx self,
        builder: GlobalAliasBuilder<'ctx, B>,
    ) -> IrResult<GlobalAlias<'ctx, B>> {
        let (name, data, address_space) = builder.into_data();
        if !name.is_empty() && self.global_name_exists(&name) {
            return Err(IrError::DuplicateGlobalName { name });
        }
        let pointer_ty = self.ctx.ptr_type(address_space);
        let seeded_aliasee = data.aliasee.get();
        let value_id = self.ctx.push_value(ValueData {
            ty: pointer_ty,
            name: core::cell::RefCell::new((!name.is_empty()).then(|| name.clone())),
            debug_loc: None,
            kind: ValueKindData::GlobalAlias(data),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.ctx.retarget_global_field_use(
            value_id,
            GlobalFieldKind::Aliasee,
            None,
            Some(seeded_aliasee),
        );
        self.aliases.borrow_mut().push(value_id);
        if !name.is_empty() {
            self.alias_by_name.borrow_mut().insert(name, value_id);
        }
        Ok(GlobalAlias::from_parts_unchecked(
            value_id,
            ModuleRef::<B>::new(self),
            pointer_ty,
        ))
    }

    pub(super) fn install_global_ifunc<B: ModuleBrand + 'ctx>(
        &'ctx self,
        builder: GlobalIfuncBuilder<'ctx, B>,
    ) -> IrResult<GlobalIfunc<'ctx, B>> {
        let (name, data, address_space) = builder.into_data();
        if !name.is_empty() && self.global_name_exists(&name) {
            return Err(IrError::DuplicateGlobalName { name });
        }
        let pointer_ty = self.ctx.ptr_type(address_space);
        let seeded_resolver = data.resolver.get();
        let value_id = self.ctx.push_value(ValueData {
            ty: pointer_ty,
            name: core::cell::RefCell::new((!name.is_empty()).then(|| name.clone())),
            debug_loc: None,
            kind: ValueKindData::GlobalIfunc(data),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.ctx.retarget_global_field_use(
            value_id,
            GlobalFieldKind::IfuncResolver,
            None,
            Some(seeded_resolver),
        );
        self.ifuncs.borrow_mut().push(value_id);
        if !name.is_empty() {
            self.ifunc_by_name.borrow_mut().insert(name, value_id);
        }
        Ok(GlobalIfunc::from_parts_unchecked(
            value_id,
            ModuleRef::<B>::new(self),
            pointer_ty,
        ))
    }

    fn global_name_exists(&self, name: &str) -> bool {
        self.function_by_name.borrow().contains_key(name)
            || self.global_by_name.borrow().contains_key(name)
            || self.alias_by_name.borrow().contains_key(name)
            || self.ifunc_by_name.borrow().contains_key(name)
    }

    // ---- DataLayout / target triple / module asm ----

    /// Borrow the parsed [`DataLayout`](crate::data_layout::DataLayout).
    /// Mirrors `Module::getDataLayout`. Returns the default (empty)
    /// layout when no directive has been set.
    pub fn data_layout(&self) -> core::cell::Ref<'_, DataLayout> {
        self.data_layout.borrow()
    }

    /// Replace the data layout. Mirrors
    /// `Module::setDataLayout(const DataLayout &)`; the string-directive
    /// path parses first ([`DataLayout::parse`]) so this setter itself
    /// cannot fail.
    pub fn set_data_layout(&self, layout: DataLayout) {
        *self.data_layout.borrow_mut() = layout;
    }

    /// `target triple = "..."` directive. Mirrors
    /// `Module::getTargetTriple` (post-Triple-class API: returns the
    /// stored string).
    pub fn target_triple(&self) -> Option<String> {
        self.target_triple.borrow().clone()
    }

    /// Set the `target triple` directive. Mirrors
    /// `Module::setTargetTriple`.
    pub fn set_target_triple<T>(&self, triple: T)
    where
        T: Into<String>,
    {
        *self.target_triple.borrow_mut() = Some(triple.into());
    }

    /// Clear the `target triple` directive.
    pub fn clear_target_triple(&self) {
        *self.target_triple.borrow_mut() = None;
    }

    /// Module-level inline assembly. Mirrors
    /// `Module::getModuleInlineAsm`.
    pub fn module_asm(&self) -> String {
        self.module_asm.borrow().clone()
    }

    /// Replace the module-level inline assembly. Mirrors
    /// `Module::setModuleInlineAsm`. Pass an empty string to clear.
    pub fn set_module_asm<Asm>(&self, asm: Asm)
    where
        Asm: Into<String>,
    {
        *self.module_asm.borrow_mut() = asm.into();
    }

    /// Append one line of module-level inline assembly. Mirrors
    /// `Module::appendModuleInlineAsm`.
    pub fn append_module_asm<Line>(&self, line: Line)
    where
        Line: AsRef<str>,
    {
        let mut buf = self.module_asm.borrow_mut();
        if !buf.is_empty() && !buf.ends_with('\n') {
            buf.push('\n');
        }
        buf.push_str(line.as_ref());
    }
    pub fn append_use_list_order(&self, record: UseListOrderRecord) -> IrResult<()> {
        validate_use_list_order_indexes(record.indexes())?;
        self.use_list_orders.borrow_mut().push(record);
        Ok(())
    }

    pub fn append_use_list_order_bb(&self, record: UseListOrderBbRecord) -> IrResult<()> {
        validate_use_list_order_indexes(record.indexes())?;
        self.use_list_order_bbs.borrow_mut().push(record);
        Ok(())
    }

    pub fn iter_use_list_orders(&self) -> impl ExactSizeIterator<Item = UseListOrderRecord> {
        self.use_list_orders.borrow().clone().into_iter()
    }

    pub fn iter_use_list_order_bbs(&self) -> impl ExactSizeIterator<Item = UseListOrderBbRecord> {
        self.use_list_order_bbs.borrow().clone().into_iter()
    }

    pub fn set_attribute_group(&self, id: u32, storage: AttributeStorage) {
        let mut groups = self.attribute_groups.borrow_mut();
        if let Some((_, existing)) = groups.iter_mut().find(|(slot, _)| *slot == id) {
            *existing = storage;
            return;
        }
        groups.push((id, storage));
        groups.sort_by_key(|(slot, _)| *slot);
    }

    pub fn attribute_groups(&self) -> Vec<(u32, AttributeStorage)> {
        self.attribute_groups.borrow().clone()
    }

    /// The attribute group numbered `id`, or `None` if no such group was
    /// registered. Scans from the back so a later registration wins, matching
    /// what a caller walking [`Self::attribute_groups`] in reverse would see —
    /// though [`Self::set_attribute_group`] replaces in place, so the ids are
    /// unique and the direction only matters as a tie-break that cannot fire.
    pub fn attribute_group(&self, id: u32) -> Option<AttributeStorage> {
        self.attribute_groups
            .borrow()
            .iter()
            .rev()
            .find_map(|(slot, storage)| (*slot == id).then(|| storage.clone()))
    }

    // ---- Metadata ----
    //
    // `ModuleCore` is brand-free, so these are generic in `B`: the brand rides
    // on the caller's ids and on the ids handed back, while the arena speaks
    // the crate-private storage form. Every one of them that *accepts* an id
    // routes it through `MetadataId::into_stored`, which compares the module
    // tag — the single choke point for the metadata currency, the same role
    // `ViewIn::resolve_in` plays for the value currency.

    /// Intern a metadata string node. Returns an existing id if an
    /// identical string was already interned. Mirrors `MDString::get`.
    pub fn metadata_string<B, S>(&self, s: S) -> MetadataId<B>
    where
        B: ModuleBrand,
        S: Into<String>,
    {
        let slot = self.metadata.borrow_mut().get_string(s);
        MetadataId::from_raw(self.id, slot)
    }

    /// Create a metadata tuple node. Mirrors `MDTuple::get` (distinct).
    ///
    /// Accepts anything that borrows as a slice of
    /// [`MetadataId`](crate::MetadataId) — both an owned `Vec` and a borrowed
    /// `&[..]` work.
    pub fn metadata_tuple<B, Ops>(&self, operands: Ops) -> IrResult<MetadataId<B>>
    where
        B: ModuleBrand,
        Ops: AsRef<[MetadataId<B>]>,
    {
        self.metadata_tuple_with_distinct(false, operands)
    }

    /// Create a tuple node with explicit distinctness.
    pub fn metadata_tuple_with_distinct<B, Ops>(
        &self,
        distinct: bool,
        operands: Ops,
    ) -> IrResult<MetadataId<B>>
    where
        B: ModuleBrand,
        Ops: AsRef<[MetadataId<B>]>,
    {
        let operands = operands
            .as_ref()
            .iter()
            .map(|id| id.into_stored(self.id))
            .collect::<IrResult<Vec<_>>>()?;
        let slot = self
            .metadata
            .borrow_mut()
            .get_tuple_with_distinct(distinct, operands);
        Ok(MetadataId::from_raw(self.id, slot))
    }

    /// Create a specialized debug metadata node.
    pub fn metadata_specialized<B>(
        &self,
        node: SpecializedMetadataNode<B>,
    ) -> IrResult<MetadataId<B>>
    where
        B: ModuleBrand,
    {
        let node = node.into_stored(self.id)?;
        let slot = self.metadata.borrow_mut().get_specialized(node);
        Ok(MetadataId::from_raw(self.id, slot))
    }

    /// Store an already-parsed metadata node and return its id.
    pub fn metadata_node<B>(&self, kind: MetadataKind<B>) -> IrResult<MetadataId<B>>
    where
        B: ModuleBrand,
    {
        let kind = kind.into_stored(self.id)?;
        let (slot, value_use) = {
            let mut store = self.metadata.borrow_mut();
            match kind {
                MetadataKind::String(s) => (store.get_string(s), Vec::new()),
                MetadataKind::Tuple { distinct, operands } => (
                    store.get_tuple_with_distinct(distinct, operands),
                    Vec::new(),
                ),
                MetadataKind::Specialized(node) => (store.get_specialized(node), Vec::new()),
                MetadataKind::Constant(value_id) => {
                    let slot = store.get_constant(value_id);
                    (slot, vec![value_id.slot()])
                }
                MetadataKind::Ref(id) => (id.slot(), Vec::new()),
                MetadataKind::Null => {
                    let slot = store.reserve();
                    store.set(slot, MetadataKind::Null);
                    (slot, Vec::new())
                }
                // Every operand of a `!DIArgList` is a real use of its value,
                // so all of them are registered — not just a first one.
                MetadataKind::ArgList { arguments } => {
                    let uses = arguments.iter().map(|id| id.slot()).collect();
                    let slot = store.get_arg_list(arguments);
                    (slot, uses)
                }
            }
        };
        for value_slot in value_use {
            self.register_metadata_value_use(slot, value_slot);
        }
        Ok(MetadataId::from_raw(self.id, slot))
    }

    /// Reserve a fresh metadata node id with placeholder content, to be
    /// filled via [`metadata_set`](Self::metadata_set). Used by the parser
    /// to resolve forward references without assuming textual `!N` slots
    /// equal arena indices.
    pub fn metadata_reserve<B>(&self) -> MetadataId<B>
    where
        B: ModuleBrand,
    {
        let slot = self.metadata.borrow_mut().reserve();
        MetadataId::from_raw(self.id, slot)
    }

    /// Overwrite a reserved metadata node with concrete content. Pairs
    /// with [`metadata_reserve`](Self::metadata_reserve).
    ///
    /// `Err(IrError::ForeignMetadataId)` when `id` was minted by another
    /// module, `Err(IrError::UnknownMetadataSlot)` when it names nothing here.
    pub fn metadata_set<B>(&self, id: MetadataId<B>, kind: MetadataKind<B>) -> IrResult<()>
    where
        B: ModuleBrand,
    {
        let slot = self.metadata_slot_of(id)?;
        let kind = kind.into_stored(self.id)?;
        match self.metadata.borrow().get(slot).cloned() {
            Some(MetadataKind::Constant(value_id)) => {
                self.deregister_metadata_value_use(slot, value_id.slot());
            }
            Some(MetadataKind::ArgList { arguments }) => {
                for value_id in arguments {
                    self.deregister_metadata_value_use(slot, value_id.slot());
                }
            }
            _ => {}
        }
        let value_use: Vec<_> = match &kind {
            MetadataKind::Constant(value_id) => vec![value_id.slot()],
            MetadataKind::ArgList { arguments } => arguments.iter().map(|id| id.slot()).collect(),
            MetadataKind::Null
            | MetadataKind::String(_)
            | MetadataKind::Tuple { .. }
            | MetadataKind::Ref(_)
            | MetadataKind::Specialized(_) => Vec::new(),
        };
        self.metadata.borrow_mut().set(slot, kind);
        for value_slot in value_use {
            self.register_metadata_value_use(slot, value_slot);
        }
        Ok(())
    }

    /// The metadata arena's boundary: compare `id`'s module tag against this
    /// module, then range-check the slot it names.
    ///
    /// Nothing else in the crate turns a caller's [`MetadataId`] into a
    /// [`MetadataSlot`] — `MetadataId::slot` exists only on the storage brand —
    /// so the tag check cannot be skipped one level up.
    fn metadata_slot_of<B>(&self, id: MetadataId<B>) -> IrResult<MetadataSlot>
    where
        B: ModuleBrand,
    {
        let slot = id.into_stored(self.id)?.slot();
        let store = self.metadata.borrow();
        if store.get(slot).is_none() {
            return Err(IrError::UnknownMetadataSlot {
                index: slot.index(),
                len: store.len(),
            });
        }
        Ok(slot)
    }

    pub(super) fn metadata_constant_value<B>(&self, value_id: ValueSlot) -> MetadataId<B>
    where
        B: ModuleBrand,
    {
        let slot = self
            .metadata
            .borrow_mut()
            .get_constant(ValueId::from_raw(self.id, value_id));
        self.register_metadata_value_use(slot, value_id);
        MetadataId::from_raw(self.id, slot)
    }

    pub(super) fn rewrite_metadata_value(
        &self,
        slot: MetadataSlot,
        from: ValueSlot,
        to: ValueSlot,
    ) {
        let mut store = self.metadata.borrow_mut();
        if let Some(MetadataKind::Constant(value_id)) = store.get_mut(slot)
            && value_id.slot() == from
        {
            let tag = value_id.tag();
            *value_id = ValueId::from_raw(tag, to);
        }
    }

    fn register_metadata_value_use(&self, metadata_slot: MetadataSlot, value_id: ValueSlot) {
        self.ctx
            .value_data(value_id)
            .use_list
            .borrow_mut()
            .push(ValueUse::Metadata(metadata_slot));
    }

    fn deregister_metadata_value_use(&self, metadata_slot: MetadataSlot, value_id: ValueSlot) {
        let mut uses = self.ctx.value_data(value_id).use_list.borrow_mut();
        if let Some(pos) = uses
            .iter()
            .position(|edge| *edge == ValueUse::Metadata(metadata_slot))
        {
            uses.remove(pos);
        }
    }

    /// Look up a metadata node by id. `None` for a foreign id or one whose
    /// slot names nothing here — never another module's node.
    pub fn metadata_get<B>(&self, id: MetadataId<B>) -> Option<MetadataKind<B>>
    where
        B: ModuleBrand,
    {
        let slot = id.into_stored(self.id).ok()?.slot();
        self.metadata
            .borrow()
            .get(slot)
            .map(MetadataKind::from_stored)
    }

    /// Number of numbered metadata nodes. `MDString`s are uniqued metadata
    /// operands, but LLVM does not assign them standalone `!N` slots.
    pub fn metadata_count(&self) -> usize {
        self.metadata
            .borrow()
            .nodes()
            .iter()
            .filter(|node| !matches!(node, MetadataKind::String(_)))
            .count()
    }

    /// Crate-internal: borrow the metadata store.
    pub(super) fn metadata_store(&self) -> core::cell::Ref<'_, MetadataStore> {
        self.metadata.borrow()
    }

    /// Get or create a named metadata node with the given name, minting its
    /// stored-brand id. Mirrors `Module::getOrInsertNamedMetadata`.
    pub fn get_or_insert_named_metadata(
        &self,
        name: NamedMetadataName,
    ) -> NamedMetadataId<StoredBrand> {
        let mut nmd = self.named_metadata.borrow_mut();
        for (i, node) in nmd.iter().enumerate() {
            if *node.name() == name {
                return NamedMetadataId::from_raw(self.id, NamedMetadataSlot(i));
            }
        }
        let slot = NamedMetadataSlot(nmd.len());
        nmd.push(NamedMetadataNode::new(name));
        NamedMetadataId::from_raw(self.id, slot)
    }

    /// Look up an existing named metadata node by name. Mirrors
    /// `Module::getNamedMetadata`.
    pub fn named_metadata(&self, name: &NamedMetadataName) -> Option<NamedMetadataId<StoredBrand>> {
        let nmd = self.named_metadata.borrow();
        let slot = nmd.iter().position(|node| node.name() == name)?;
        Some(NamedMetadataId::from_raw(self.id, NamedMetadataSlot(slot)))
    }

    /// Append an operand to a named metadata node. Mirrors
    /// `NamedMDNode::addOperand`.
    ///
    /// `Err(IrError::ForeignNamedMetadataId)` when `id` was minted by another
    /// module, `Err(IrError::ForeignMetadataId)` when `op` was. There is no
    /// unknown-slot case: the named-metadata list is append-only, so a native
    /// id's slot keeps naming the node it was minted for.
    pub fn named_metadata_add_operand<B>(
        &self,
        id: NamedMetadataId<B>,
        op: MetadataId<B>,
    ) -> IrResult<()>
    where
        B: ModuleBrand,
    {
        let slot = id.into_stored(self.id)?.slot();
        let op = op.into_stored(self.id)?;
        let mut nmd = self.named_metadata.borrow_mut();
        let node = nmd.get_mut(slot.0).unwrap_or_else(|| {
            unreachable!("a stored NamedMetadataId always names a node in the append-only list")
        });
        node.add_operand(op);
        Ok(())
    }

    /// Look up a named metadata node by id, cloning it out. `None` when `id`
    /// belongs to another module — never another module's node. A native id
    /// always resolves: the named-metadata list is append-only.
    pub fn named_metadata_get<B>(&self, id: NamedMetadataId<B>) -> Option<NamedMetadataNode<B>>
    where
        B: ModuleBrand,
    {
        let slot = id.into_stored(self.id).ok()?.slot();
        let nmd = self.named_metadata.borrow();
        let node = nmd.get(slot.0).unwrap_or_else(|| {
            unreachable!("a stored NamedMetadataId always names a node in the append-only list")
        });
        Some(NamedMetadataNode::from_stored(node))
    }

    /// Number of named metadata nodes.
    pub fn named_metadata_count(&self) -> usize {
        self.named_metadata.borrow().len()
    }

    /// Crate-internal: borrow named metadata list for printing.
    pub(super) fn named_metadata_list(
        &self,
    ) -> core::cell::Ref<'_, Vec<NamedMetadataNode<StoredBrand>>> {
        self.named_metadata.borrow()
    }

    // ---- Module flags ----
    //
    // Flags have no storage of their own: they are the three-operand tuples
    // of the `llvm.module.flags` named metadata node, exactly as upstream
    // stores them, so the printer and the parse/print round trip are
    // untouched. Malformed tuples are skipped silently on this read path —
    // upstream's `Module::getModuleFlagsMetadata` reads them unchecked with
    // the comment "The verifier will catch errors"; the checking lives in
    // `Verifier::visit_module_flags`.

    /// The operand ids of the `llvm.module.flags` node, or an empty list
    /// when the node is absent.
    fn module_flag_operands(&self) -> Vec<MetadataId<StoredBrand>> {
        let Some(id) = self.named_metadata(&NamedMetadataName::ModuleFlags) else {
            return Vec::new();
        };
        let nmd = self.named_metadata.borrow();
        let node = nmd.get(id.slot().0).unwrap_or_else(|| {
            unreachable!("a stored NamedMetadataId always names a node in the append-only list")
        });
        node.operands().to_vec()
    }

    /// The value operand of the flag whose key string is `key`. Mirrors
    /// `Module::getModuleFlag` (`lib/IR/Module.cpp`), which walks the flag
    /// tuples comparing only the `!"key"` operand.
    pub(super) fn module_flag_value(&self, key: &str) -> Option<MetadataId<StoredBrand>> {
        let operands = self.module_flag_operands();
        let store = self.metadata.borrow();
        for op in operands {
            let Some([_, key_id, value_id]) = module_flag_tuple(&store, op) else {
                continue;
            };
            let Some(MetadataKind::String(s)) =
                resolve_metadata_ref(&store, key_id.slot()).and_then(|slot| store.get(slot))
            else {
                continue;
            };
            if s.as_str() == key {
                return Some(value_id);
            }
        }
        None
    }

    /// Decode every well-formed flag tuple. Mirrors
    /// `Module::getModuleFlagsMetadata(SmallVectorImpl<ModuleFlagEntry>&)`
    /// (`lib/IR/Module.cpp`).
    pub(super) fn module_flags_stored(&self) -> Vec<ModuleFlagEntry<StoredBrand>> {
        let operands = self.module_flag_operands();
        let store = self.metadata.borrow();
        let mut entries = Vec::new();
        for op in operands {
            let Some([behavior_id, key_id, value_id]) = module_flag_tuple(&store, op) else {
                continue;
            };
            let Some(behavior) = resolve_metadata_ref(&store, behavior_id.slot())
                .and_then(|slot| metadata_constant_int(self, &store, slot))
                .and_then(|(_, value)| ModuleFlagBehavior::from_raw(value.limited_value(u64::MAX)))
            else {
                continue;
            };
            let Some(MetadataKind::String(key)) =
                resolve_metadata_ref(&store, key_id.slot()).and_then(|slot| store.get(slot))
            else {
                continue;
            };
            entries.push(ModuleFlagEntry {
                behavior,
                key: ModuleFlagKey::from_key(key),
                value: value_id,
            });
        }
        entries
    }

    /// The replace half of `Module::setModuleFlag` (`lib/IR/Module.cpp`):
    /// overwrite the first flag whose key string is `key` with
    /// `replacement`, preserving its position. `false` when no flag carries
    /// that key — the caller then appends, exactly as upstream falls through
    /// to `addModuleFlag`.
    pub(super) fn replace_module_flag(
        &self,
        key: &str,
        replacement: MetadataId<StoredBrand>,
    ) -> bool {
        let Some(id) = self.named_metadata(&NamedMetadataName::ModuleFlags) else {
            return false;
        };
        let replace_at = {
            let nmd = self.named_metadata.borrow();
            let node = nmd.get(id.slot().0).unwrap_or_else(|| {
                unreachable!("a stored NamedMetadataId always names a node in the append-only list")
            });
            let store = self.metadata.borrow();
            node.operands().iter().position(|op| {
                matches!(
                    module_flag_tuple(&store, *op).and_then(|[_, key_id, _]| {
                        resolve_metadata_ref(&store, key_id.slot())
                            .and_then(|slot| store.get(slot))
                    }),
                    Some(MetadataKind::String(s)) if s.as_str() == key
                )
            })
        };
        let Some(index) = replace_at else {
            return false;
        };
        // `NamedMDNode` deliberately exposes no positional operand mutator
        // (its public surface mirrors `NamedMDNode::addOperand`), so the
        // replacement rebuilds the node with the one operand swapped —
        // observable content is identical to upstream's `setOperand(i, ..)`.
        let mut nmd = self.named_metadata.borrow_mut();
        let node = nmd.get_mut(id.slot().0).unwrap_or_else(|| {
            unreachable!("a stored NamedMetadataId always names a node in the append-only list")
        });
        let mut rebuilt = NamedMetadataNode::new(node.name().clone());
        for (i, op) in node.operands().iter().enumerate() {
            rebuilt.add_operand(if i == index { replacement } else { *op });
        }
        *node = rebuilt;
        true
    }

    // ---- Comdats ----

    /// Get or create a [`ComdatRef`](ComdatRef) of
    /// the given name. Mirrors `Module::getOrInsertComdat`.
    ///
    /// On first lookup the selection kind defaults to
    /// [`SelectionKind::Any`](crate::comdat::SelectionKind::Any);
    /// callers can refine via
    /// [`ComdatRef::set_selection_kind`](ComdatRef::set_selection_kind).
    pub fn get_or_insert_comdat<B, Name>(&'ctx self, name: Name) -> ComdatRef<'ctx, B>
    where
        B: ModuleBrand,
        Name: AsRef<str>,
    {
        let name = name.as_ref();
        if let Some(&id) = self.comdat_by_name.borrow().get(name) {
            return ComdatRef {
                module: ModuleRef::new(self),
                id,
            };
        }
        let index = self
            .comdats
            .push(ComdatData::new(name.to_owned(), SelectionKind::Any));
        let id = ComdatId::from_index(index);
        self.comdat_by_name.borrow_mut().insert(name.to_owned(), id);
        ComdatRef {
            module: ModuleRef::new(self),
            id,
        }
    }

    /// Look up an existing comdat by name. Returns `None` when not
    /// present.
    pub fn comdat<B: ModuleBrand>(&'ctx self, name: &str) -> Option<ComdatRef<'ctx, B>> {
        let id = *self.comdat_by_name.borrow().get(name)?;
        Some(ComdatRef {
            module: ModuleRef::new(self),
            id,
        })
    }

    /// Crate-internal: borrow the underlying [`ComdatData`] by id.
    /// Mirrors `Module::comdat_at`.
    pub(super) fn comdat_at(&self, id: ComdatId) -> &ComdatData {
        self.comdats
            .get(id.arena_index())
            .unwrap_or_else(|| unreachable!("ComdatId is always valid for the owning module"))
    }

    /// Iterate comdat refs in insertion order. Mirrors
    /// `Module::getComdatSymbolTable` (insertion-order traversal).
    pub fn iter_comdats<B: ModuleBrand + 'ctx>(
        &'ctx self,
    ) -> impl ExactSizeIterator<Item = ComdatRef<'ctx, B>> + DoubleEndedIterator + FusedIterator + 'ctx
    {
        let count = self.comdats.count();
        (0..count).map(move |i| ComdatRef {
            module: ModuleRef::new(self),
            id: ComdatId::from_index(i),
        })
    }
}

impl Module<DynBrand, Unverified> {
    /// Construct a fresh module under the **named** brand `B`.
    ///
    /// At most one live module may hold a given brand type. A second call for a
    /// brand whose module is still alive fails with [`IrError::BrandInUse`];
    /// once that module is dropped the brand is free again.
    ///
    /// ```
    /// use llvmkit_ir::{IrError, Module};
    ///
    /// #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    /// struct LiftedBin;
    /// impl llvmkit_ir::ModuleBrand for LiftedBin {}
    ///
    /// let m = Module::branded::<LiftedBin, _>("lifted")?;
    /// assert!(matches!(
    ///     Module::branded::<LiftedBin, _>("again"),
    ///     Err(IrError::BrandInUse { .. })
    /// ));
    ///
    /// drop(m);
    /// let _reused = Module::branded::<LiftedBin, _>("again")?;
    /// # Ok::<(), IrError>(())
    /// ```
    ///
    /// # Leaking
    ///
    /// The claim is released by the module's `Drop`. [`core::mem::forget`]ting
    /// a module (or leaking it any other way) therefore keeps the brand claimed
    /// for the rest of the process: leaking is an implicit
    /// [`branded_once`](Self::branded_once). There is deliberately no API to
    /// release a claim without dropping the module: one that existed would let
    /// a fresh module take a brand whose predecessor is still alive, so
    /// `'static` handles of two generations would share a single type.
    ///
    /// # Errors
    ///
    /// [`IrError::BrandInUse`] if a live module already holds `B`;
    /// [`IrError::BrandRetired`] if `B` was retired by
    /// [`branded_once`](Self::branded_once).
    pub fn branded<B, N>(name: N) -> IrResult<Module<B, Unverified>>
    where
        B: ModuleBrand,
        N: Into<String>,
    {
        Self::registered::<B, N>(name, false)
    }

    /// Construct a fresh module under the named brand `B`, **retiring `B`
    /// permanently** when the module is dropped.
    ///
    /// Where [`branded`](Self::branded) frees the brand for reuse, this marks it
    /// dead: every later claim fails with [`IrError::BrandRetired`], forever.
    /// Use it when handles minted from the module may outlive it — a retired
    /// brand can never name a *successor* module, so a stale handle can never be
    /// replayed against fresh storage even if the runtime [`ModuleId`] check
    /// were bypassed.
    ///
    /// ```
    /// use llvmkit_ir::{IrError, Module};
    ///
    /// #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    /// struct BuiltOnce;
    /// impl llvmkit_ir::ModuleBrand for BuiltOnce {}
    ///
    /// drop(Module::branded_once::<BuiltOnce, _>("once")?);
    /// assert!(matches!(
    ///     Module::branded_once::<BuiltOnce, _>("twice"),
    ///     Err(IrError::BrandRetired { .. })
    /// ));
    /// # Ok::<(), IrError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`IrError::BrandInUse`] if a live module already holds `B`;
    /// [`IrError::BrandRetired`] if `B` has already been retired.
    pub fn branded_once<B, N>(name: N) -> IrResult<Module<B, Unverified>>
    where
        B: ModuleBrand,
        N: Into<String>,
    {
        Self::registered::<B, N>(name, true)
    }

    /// Shared body of [`branded`](Self::branded) and
    /// [`branded_once`](Self::branded_once).
    ///
    /// The step order is load-bearing:
    ///
    /// 1. **user conversions first.** `name.into()` is arbitrary user code. If
    ///    it ran while the registry lock was held and re-entered `branded`, the
    ///    non-reentrant `Mutex` would deadlock. Running it here means the lock
    ///    is not yet taken.
    /// 2. **build the storage.** Also outside the lock, so a long or
    ///    allocation-heavy construction never blocks another thread's claim.
    /// 3. **register last.** Nothing after the claim can fail or panic, so a
    ///    partially-constructed module can never strand a brand as `InUse`. (If
    ///    it could, the guard's `Drop` would still release it on unwind — but
    ///    the ordering means that never has to happen.)
    fn registered<B, N>(name: N, retire_on_drop: bool) -> IrResult<Module<B, Unverified>>
    where
        B: ModuleBrand,
        N: Into<String>,
    {
        let name: String = name.into();
        let core = Box::new(ModuleCore::new(name));
        let registration = BrandGuard::<B>::claim(retire_on_drop)?;
        Ok(Module {
            core,
            registration: Some(registration),
            _brand: PhantomData,
            _state: PhantomData,
        })
    }

    /// Construct a fresh module under [`DynBrand`], the registry-exempt brand.
    ///
    /// Infallible, and arbitrarily many may be live at once — which is the
    /// point: use it when the number of modules is decided at run time.
    /// Distinct `DynBrand` modules share one static type, so handles are
    /// separated only by the runtime [`ModuleId`] tag.
    ///
    /// ```
    /// use llvmkit_ir::Module;
    ///
    /// let modules: Vec<_> = (0..4).map(|i| Module::dynamic(format!("m{i}"))).collect();
    /// assert_eq!(modules.len(), 4);
    /// ```
    pub fn dynamic<N>(name: N) -> Module<DynBrand, Unverified>
    where
        N: Into<String>,
    {
        let name: String = name.into();
        Module {
            core: Box::new(ModuleCore::new(name)),
            registration: None,
            _brand: PhantomData,
            _state: PhantomData,
        }
    }
}

impl<'ctx, B: ModuleBrand + 'ctx, S> Module<B, S> {
    /// Borrow the owned storage. Lifetime-eliding on purpose: a `&'ctx self`
    /// caller gets `&'ctx ModuleCore` (the handle-minting path), a short-borrow
    /// caller gets a short one (`Display`, the state transitions).
    #[inline]
    fn core(&self) -> &ModuleCore {
        &self.core
    }

    /// Owning module's [`ModuleId`].
    #[inline]
    pub fn id(&self) -> ModuleId {
        self.core().id()
    }

    /// Module identifier.
    #[inline]
    pub fn name(&self) -> &str {
        self.core().name()
    }

    /// Read-only branded view.
    #[inline]
    pub fn as_view(&'ctx self) -> ModuleView<'ctx, B> {
        ModuleView::new(self.core())
    }

    /// Crate-internal borrow of the state-erased module storage.
    #[inline]
    pub(super) fn core_ref(&'ctx self) -> &'ctx ModuleCore {
        self.core()
    }

    /// Crate-internal state-erased module handle with this token's brand.
    #[inline]
    pub(super) fn module_ref(&'ctx self) -> ModuleRef<'ctx, B> {
        ModuleRef::new(self.core())
    }

    /// `source_filename = "..."` directive.
    #[inline]
    pub fn source_filename(&self) -> Option<core::cell::Ref<'_, str>> {
        self.core().source_filename()
    }

    /// Borrow the parsed data layout.
    #[inline]
    pub fn data_layout(&self) -> core::cell::Ref<'_, DataLayout> {
        self.core().data_layout()
    }

    /// Target triple directive.
    #[inline]
    pub fn target_triple(&self) -> Option<String> {
        self.core().target_triple()
    }

    /// Module-level inline assembly.
    #[inline]
    pub fn module_asm(&self) -> String {
        self.core().module_asm()
    }

    /// Every numbered attribute group in the module, in registration order.
    ///
    /// Most callers want a single group; [`attribute_group`](Self::attribute_group)
    /// is the point lookup and copies only that one entry.
    ///
    /// The table lives behind a `RefCell`, so the iterator walks a snapshot
    /// taken at call time rather than borrowing the module.
    pub fn attribute_groups(
        &self,
    ) -> impl ExactSizeIterator<Item = (u32, AttributeStorage)>
    + DoubleEndedIterator
    + core::iter::FusedIterator
    + use<B, S> {
        self.core().attribute_groups().into_iter()
    }

    /// The attribute group printed as `#id`, or `None` if the module has no
    /// group with that number. Mirrors the `attributes #N = { … }` table a
    /// function's `#N` references resolve against.
    pub fn attribute_group(&self, id: u32) -> Option<AttributeStorage> {
        self.core().attribute_group(id)
    }

    /// Iterate globals in declaration order with this module token's brand.
    pub fn globals(
        &'ctx self,
    ) -> impl ExactSizeIterator<Item = GlobalVariable<'ctx, B>>
    + DoubleEndedIterator
    + FusedIterator
    + 'ctx {
        self.core().iter_globals::<B>()
    }

    /// Total number of instructions across every block of every function
    /// in this module. Mirrors the C++ idiom
    /// `for (F : M) for (BB : F) count += BB.size()` — LLVM has no
    /// `Module::getInstructionCount()`, but its size-driven heuristics
    /// (`InlineCost`, `-instcount`) compute exactly this.
    ///
    /// The reason it is on the module rather than left to the caller: a
    /// transform driven to a fixpoint terminates on "the module stopped
    /// changing size", and spelling that by hand means threading a
    /// nested walk through code whose subject is the transform, not the
    /// arithmetic. Declarations contribute nothing (they have no
    /// blocks).
    ///
    /// ```
    /// use llvmkit_ir::{Dyn, IrBuilder, IntValue, Linkage, Module};
    ///
    /// # fn main() -> Result<(), llvmkit_ir::IrError> {
    /// let m = Module::dynamic("count");
    /// assert_eq!(m.instruction_count(), 0);
    ///
    /// let i32_ty = m.i32_type();
    /// let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    /// let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    /// let entry = m.view(f).append_basic_block(&m, "entry");
    ///
    /// let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    /// let n: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    /// let sum = b.int_add(n, 1_i32, "sum")?;
    /// b.ret(m.view(sum))?;
    /// assert_eq!(m.instruction_count(), 2);
    /// # Ok(()) }
    /// ```
    pub fn instruction_count(&'ctx self) -> usize {
        self.core()
            .iter_functions::<B>()
            .map(|f| {
                f.basic_blocks()
                    .map(|bb| bb.instructions().len())
                    .sum::<usize>()
            })
            .sum()
    }

    /// Look up a function by name with this module token's brand,
    /// widened to [`Dyn`], returning its storable [`FunctionId`].
    ///
    /// Symmetric with [`add_function_dyn`](Self::add_function_dyn): a lookup
    /// hands back the same currency a declaration does. Reach the borrowing
    /// [`FunctionValue`] with [`view`](Self::view) when you need one.
    ///
    /// The id borrows nothing, so this takes `&self` rather than `&'ctx self` —
    /// a lookup can be interleaved with other borrows of the module.
    pub fn function_dyn(&self, name: &str) -> Option<FunctionId<Dyn, B>> {
        let slot = self.core().function_by_name.borrow().get(name).copied()?;
        Some(FunctionId::from_raw(self.core().id, slot))
    }

    /// Look up a function by name and narrow to a specific return marker,
    /// returning its storable [`FunctionId`].
    ///
    /// Symmetric with the `add_*` family. The marker check is unchanged: a
    /// signature that does not match `R` is
    /// [`IrError::ReturnTypeMismatch`], not a silently-widened id.
    pub fn function<R>(&self, name: &str) -> IrResult<Option<FunctionId<R, B>>>
    where
        R: ReturnMarker,
    {
        let Some(id) = self.core().function_by_name.borrow().get(name).copied() else {
            return Ok(None);
        };
        let value_data = self.core().ctx.value_data(id);
        let signature_id = match &value_data.kind {
            ValueKindData::Function(f) => f.signature,
            _ => unreachable!("function_by_name table only stores function ids"),
        };
        let ret_id = self
            .core()
            .ctx
            .type_data(signature_id)
            .as_function()
            .unwrap_or_else(|| unreachable!("function value carries a function signature"))
            .0;
        let ret_data = self.core().ctx.type_data(ret_id);
        if !crate::function::signature_matches_marker::<R>(ret_data) {
            let got =
                crate::r#type::Type::new(ret_id, ModuleRef::<B>::new(self.core())).kind_label();
            return Err(IrError::ReturnTypeMismatch {
                expected: crate::marker::marker_kind_label::<R>()
                    .unwrap_or_else(|| unreachable!("Dyn marker matches every signature")),
                got,
            });
        }
        Ok(Some(FunctionId::<R, B>::from_raw(self.core().id, id)))
    }

    /// Verify the module's structural invariants without consuming it.
    pub fn verify_borrowed(&self) -> IrResult<()> {
        // Deliberately not `self.as_view()`: that is a `&'ctx self` method (it
        // mints a `'ctx`-anchored view), while this only needs a view for the
        // duration of the call. Building it from a short borrow keeps
        // `verify_borrowed` callable on a token the caller is about to move.
        Verifier::new(ModuleView::<B>::new(self.core())).run()
    }

    /// Resolve a storable value id (from [`Value::id`](crate::Value::id)
    /// and its per-kind siblings) back into its borrowing handle — the
    /// resolution boundary for the 0.0.4 id family.
    ///
    /// This is the module-tag choke point: the id's tag is compared against
    /// this module's [`ModuleId`] *before* the arena is touched, so an id
    /// minted in a different module can never mis-resolve against an in-range
    /// slot here. The kind of handle returned is chosen by the id type (e.g.
    /// an [`IntValueId<W>`](crate::IntValueId) yields an
    /// [`IntValue<W>`](crate::IntValue), a [`BlockId`](crate::BlockId) yields a
    /// copyable [`BasicBlockLabel`](crate::BasicBlockLabel)).
    ///
    /// Works on both [`Unverified`] and [`Verified`] modules.
    ///
    /// # Panics
    ///
    /// Panics if the id belongs to a different module (foreign tag) or its slot
    /// is absent — a deterministic contract violation, like indexing a slice
    /// out of bounds. Use [`try_view`](Self::try_view) for the fallible form.
    ///
    /// A slot whose value was *erased* is tombstoned in place (the arena keeps
    /// it for id-stability); there is no cheap liveness flag, so `view`
    /// validates the module tag and arena range only — full
    /// tombstone-liveness detection is deferred (see the crate's `value_id`
    /// notes).
    #[inline]
    pub fn view<I>(&'ctx self, id: I) -> I::View
    where
        I: ViewIn<'ctx, B>,
    {
        id.resolve_in(self.module_ref()).unwrap_or_else(|| {
            panic!(
                "Module::view: id does not resolve in this module \
                 (foreign module tag or absent/tombstoned slot)"
            )
        })
    }

    /// Fallible [`view`](Self::view): resolve a storable value id into its
    /// borrowing handle, returning [`None`] when the id belongs to a different
    /// module (foreign tag) or its slot is absent.
    ///
    /// Like [`view`](Self::view), this validates the module tag and arena range
    /// only; a tombstoned-but-in-range slot is not detected (no cheap liveness
    /// flag exists). Works on both [`Unverified`] and [`Verified`] modules.
    #[inline]
    pub fn try_view<I>(&'ctx self, id: I) -> Option<I::View>
    where
        I: ViewIn<'ctx, B>,
    {
        id.resolve_in(self.module_ref())
    }

    // ---- By-name lookups ----
    //
    // State-generic since cycle E. These return either a capability-free
    // `Copy + Send` id or, for comdats, a read-only handle — nothing that can
    // mutate — so restricting them to `Module<B, Unverified>` bought no safety
    // and left a verified module with no O(1) route to a symbol at all: the
    // only alternative was a linear scan of `as_view().globals()` comparing
    // names. `function` / `function_dyn` already lived here.

    /// Look up a global variable by name, returning its storable
    /// [`GlobalId`].
    ///
    /// Symmetric with [`add_global`](Self::add_global) and the rest of the
    /// `add_global_*` family: a lookup hands back the same currency a
    /// declaration does. Reach the borrowing [`GlobalVariable`] with
    /// [`view`](Self::view).
    ///
    /// The id borrows nothing, so this takes `&self`.
    pub fn global(&self, name: &str) -> Option<GlobalId<B>> {
        let slot = self.core().global_by_name.borrow().get(name).copied()?;
        Some(GlobalId::from_raw(self.core().id, slot))
    }

    /// Look up a global alias by name, returning its storable
    /// [`GlobalAliasId`]. Symmetric with
    /// [`alias_builder`](Self::alias_builder)'s `build()`.
    pub fn alias(&self, name: &str) -> Option<GlobalAliasId<B>> {
        let slot = self.core().alias_by_name.borrow().get(name).copied()?;
        Some(GlobalAliasId::from_raw(self.core().id, slot))
    }

    /// Look up an ifunc by name, returning its storable [`GlobalIfuncId`].
    /// Symmetric with [`ifunc_builder`](Self::ifunc_builder)'s `build()`.
    pub fn ifunc(&self, name: &str) -> Option<GlobalIfuncId<B>> {
        let slot = self.core().ifunc_by_name.borrow().get(name).copied()?;
        Some(GlobalIfuncId::from_raw(self.core().id, slot))
    }

    /// Look up a comdat by name.
    ///
    /// Deliberately **not** part of the `get_* -> Option<Id>` symmetry the rest
    /// of the lookups follow. A comdat is not a `Value`: it lives in its own
    /// table, and [`ComdatId`] is a bare `u32` index carrying neither a
    /// [`ModuleId`] tag nor a brand, so it is not a member of the 0.0.4 id
    /// family and [`view`](Self::view) cannot resolve it. Returning it here
    /// would hand back something strictly *weaker* than the handle — untagged,
    /// unbranded, and unresolvable — so the handle stays.
    pub fn comdat(&'ctx self, name: &str) -> Option<ComdatRef<'ctx, B>> {
        self.core().comdat::<B>(name)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> Module<B, Unverified> {
    /// Allocate the next per-module [`crate::ssa_builder::SsaBuilderId`].
    #[inline]
    pub(crate) fn next_ssa_builder_id(&'ctx self) -> u32 {
        self.core().next_ssa_builder_id()
    }

    pub fn function_builder<R, Name>(
        &'ctx self,
        name: Name,
        signature: FunctionType<'ctx, B>,
    ) -> FunctionBuilder<'ctx, R, B>
    where
        R: ReturnMarker,
        Name: Into<String>,
    {
        self.core().function_builder::<B, R, Name>(name, signature)
    }

    pub fn constant_expr<Operands, Indices, Mask>(
        &'ctx self,
        result_ty: Type<'ctx, B>,
        opcode: ConstantExprOpcode,
        operands: Operands,
        indices: Indices,
        mask: Mask,
        flags: ConstantExprFlags,
    ) -> IrResult<Constant<'ctx, B>>
    where
        Operands: IntoIterator<Item = Value<'ctx, B>>,
        Indices: IntoIterator<Item = u32>,
        Mask: IntoIterator<Item = i32>,
    {
        self.core()
            .constant_expr::<B, _, _, _>(result_ty, opcode, operands, indices, mask, flags)
    }

    pub fn constant_expr_with_options<Operands, Indices, Mask>(
        &'ctx self,
        result_ty: Type<'ctx, B>,
        opcode: ConstantExprOpcode,
        operands: Operands,
        indices: Indices,
        mask: Mask,
        options: ConstantExprOptions<'ctx, B>,
    ) -> IrResult<Constant<'ctx, B>>
    where
        Operands: IntoIterator<Item = Value<'ctx, B>>,
        Indices: IntoIterator<Item = u32>,
        Mask: IntoIterator<Item = i32>,
    {
        self.core().constant_expr_with_options::<B, _, _, _>(
            result_ty, opcode, operands, indices, mask, options,
        )
    }

    pub fn block_address<R, S>(
        &'ctx self,
        function: FunctionValue<'ctx, R, B>,
        block: &BasicBlock<'ctx, R, S, B>,
    ) -> IrResult<Constant<'ctx, B>>
    where
        R: crate::ReturnMarker,
        S: crate::BlockTerminationState,
    {
        self.core().block_address::<B, R, S>(function, block)
    }

    pub fn forward_ref_value_placeholder(
        &'ctx self,
        ty: Type<'ctx, B>,
    ) -> IrResult<ForwardRefValue<'ctx, B>> {
        self.core().forward_ref_value_placeholder::<B>(ty)
    }

    pub fn dso_local_equivalent(
        &'ctx self,
        function: FunctionValue<'ctx, Dyn, B>,
    ) -> Constant<'ctx, B> {
        self.core().dso_local_equivalent::<B>(function)
    }

    pub fn dso_local_equivalent_global(
        &'ctx self,
        global: Constant<'ctx, B>,
    ) -> IrResult<Constant<'ctx, B>> {
        self.core().dso_local_equivalent_global::<B>(global)
    }

    pub fn no_cfi(&'ctx self, function: FunctionValue<'ctx, Dyn, B>) -> Constant<'ctx, B> {
        self.core().no_cfi::<B>(function)
    }

    pub fn no_cfi_global(&'ctx self, global: Constant<'ctx, B>) -> IrResult<Constant<'ctx, B>> {
        self.core().no_cfi_global::<B>(global)
    }

    pub fn ptr_auth<Pointer, Key, Discriminator, AddrDiscriminator, DeactivationSymbol>(
        &'ctx self,
        pointer: Pointer,
        key: Key,
        discriminator: Discriminator,
        addr_discriminator: AddrDiscriminator,
        deactivation_symbol: DeactivationSymbol,
    ) -> IrResult<Constant<'ctx, B>>
    where
        Pointer: IsConstant<'ctx, B>,
        Key: IsConstant<'ctx, B>,
        Discriminator: IsConstant<'ctx, B>,
        AddrDiscriminator: IsConstant<'ctx, B>,
        DeactivationSymbol: IsConstant<'ctx, B>,
    {
        self.core().ptr_auth::<B, _, _, _, _, _>(
            pointer,
            key,
            discriminator,
            addr_discriminator,
            deactivation_symbol,
        )
    }

    pub fn token_none(&'ctx self) -> Constant<'ctx, B> {
        self.core().token_none::<B>()
    }

    pub fn target_ext_none(&'ctx self, ty: Type<'ctx, B>) -> IrResult<Constant<'ctx, B>> {
        self.core().target_ext_none::<B>(ty)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> Module<B, Unverified> {
    /// `void`.
    #[inline]
    pub fn void_type(&'ctx self) -> VoidType<'ctx, B> {
        VoidType::new(self.core().ctx.void(), self.module_ref())
    }

    /// `label`.
    #[inline]
    pub fn label_type(&'ctx self) -> LabelType<'ctx, B> {
        LabelType::new(self.core().ctx.label(), self.module_ref())
    }

    /// `metadata`.
    #[inline]
    pub fn metadata_type(&'ctx self) -> MetadataType<'ctx, B> {
        MetadataType::new(self.core().ctx.metadata(), self.module_ref())
    }

    /// `token`.
    #[inline]
    pub fn token_type(&'ctx self) -> TokenType<'ctx, B> {
        TokenType::new(self.core().ctx.token(), self.module_ref())
    }

    /// `half`.
    #[inline]
    pub fn half_type(&'ctx self) -> FloatType<'ctx, Half, B> {
        FloatType::new(self.core().ctx.half(), self.module_ref())
    }

    /// `bfloat`.
    #[inline]
    pub fn bfloat_type(&'ctx self) -> FloatType<'ctx, Bfloat, B> {
        FloatType::new(self.core().ctx.bfloat(), self.module_ref())
    }

    /// `float` (32-bit IEEE 754).
    #[inline]
    pub fn f32_type(&'ctx self) -> FloatType<'ctx, f32, B> {
        FloatType::new(self.core().ctx.float(), self.module_ref())
    }

    /// `double` (64-bit IEEE 754).
    #[inline]
    pub fn f64_type(&'ctx self) -> FloatType<'ctx, f64, B> {
        FloatType::new(self.core().ctx.double(), self.module_ref())
    }

    /// `fp128`.
    #[inline]
    pub fn fp128_type(&'ctx self) -> FloatType<'ctx, Fp128, B> {
        FloatType::new(self.core().ctx.fp128(), self.module_ref())
    }

    /// `x86_fp80`.
    #[inline]
    pub fn x86_fp80_type(&'ctx self) -> FloatType<'ctx, X86Fp80, B> {
        FloatType::new(self.core().ctx.x86_fp80(), self.module_ref())
    }

    /// `ppc_fp128`.
    #[inline]
    pub fn ppc_fp128_type(&'ctx self) -> FloatType<'ctx, PpcFp128, B> {
        FloatType::new(self.core().ctx.ppc_fp128(), self.module_ref())
    }

    /// `x86_amx`.
    #[inline]
    pub fn x86_amx_type(&'ctx self) -> Type<'ctx, B> {
        Type::new(self.core().ctx.x86_amx(), self.module_ref())
    }

    /// `exnref`.
    #[inline]
    pub fn wasm_exnref_type(&'ctx self) -> Type<'ctx, B> {
        Type::new(self.core().ctx.wasm_exnref(), self.module_ref())
    }

    /// `i1`.
    #[inline]
    pub fn bool_type(&'ctx self) -> IntType<'ctx, bool, B> {
        IntType::new(self.core().ctx.int_type(1), self.module_ref())
    }

    /// Alias for [`Self::bool_type`].
    #[inline]
    pub fn i1_type(&'ctx self) -> IntType<'ctx, bool, B> {
        self.bool_type()
    }

    #[inline]
    pub fn i8_type(&'ctx self) -> IntType<'ctx, i8, B> {
        IntType::new(self.core().ctx.int_type(8), self.module_ref())
    }

    #[inline]
    pub fn i16_type(&'ctx self) -> IntType<'ctx, i16, B> {
        IntType::new(self.core().ctx.int_type(16), self.module_ref())
    }

    #[inline]
    pub fn i32_type(&'ctx self) -> IntType<'ctx, i32, B> {
        IntType::new(self.core().ctx.int_type(32), self.module_ref())
    }

    #[inline]
    pub fn i64_type(&'ctx self) -> IntType<'ctx, i64, B> {
        IntType::new(self.core().ctx.int_type(64), self.module_ref())
    }

    #[inline]
    pub fn i128_type(&'ctx self) -> IntType<'ctx, i128, B> {
        IntType::new(self.core().ctx.int_type(128), self.module_ref())
    }

    pub fn custom_width_int_type(&'ctx self, bits: u32) -> IrResult<IntType<'ctx, IntDyn, B>> {
        if !(MIN_INT_BITS..=MAX_INT_BITS).contains(&bits) {
            return Err(IrError::InvalidIntegerWidth { bits });
        }
        Ok(IntType::new(
            self.core().ctx.int_type(bits),
            self.module_ref(),
        ))
    }

    pub fn int_type_n<const N: u32>(&'ctx self) -> IntType<'ctx, Width<N>, B> {
        const {
            assert!(
                N >= MIN_INT_BITS && N <= MAX_INT_BITS,
                "integer width N outside [MIN_INT_BITS, MAX_INT_BITS]",
            );
        }
        IntType::new(self.core().ctx.int_type(N), self.module_ref())
    }

    pub fn ptr_type(&'ctx self, addr_space: u32) -> PointerType<'ctx, B> {
        PointerType::new(self.core().ctx.ptr_type(addr_space), self.module_ref())
    }

    pub fn typed_pointer_type<T>(
        &'ctx self,
        pointee: T,
        addr_space: u32,
    ) -> TypedPointerType<'ctx, B>
    where
        T: Into<Type<'ctx, B>>,
    {
        let pointee_id = pointee.into().id();
        TypedPointerType::new(
            self.core().ctx.typed_pointer_type(pointee_id, addr_space),
            self.module_ref(),
        )
    }

    pub fn array_type<T>(&'ctx self, elem: T, n: u64) -> ArrayType<'ctx, ElemDyn, ArrLenDyn, B>
    where
        T: Into<Type<'ctx, B>>,
    {
        let elem_id = elem.into().id();
        ArrayType::new(self.core().ctx.array_type(elem_id, n), self.module_ref())
    }

    /// Const-generic typed array `[N x E]`. The element marker `E` projects
    /// the scalar element type and `N` pins the element count, yielding a
    /// statically typed [`ArrayType<'ctx, E, ArrLen<N>, B>`]. Unlike
    /// [`vector_type_n`](Self::vector_type_n), `N == 0` is **not** rejected:
    /// LLVM permits zero-length arrays `[0 x T]`. Mirrors `Type::getIntNTy`
    /// + `ArrayType::get`.
    pub fn array_type_n<E, const N: u64>(&'ctx self) -> ArrayType<'ctx, E, ArrLen<N>, B>
    where
        E: StaticVecElem<'ctx, B>,
    {
        let elem = E::element_ir_type(self.module_ref());
        let id = self.core().ctx.array_type(elem.id(), N);
        ArrayType::new(id, self.module_ref())
    }

    /// Fixed `<n x elem>` vector. Mirrors `FixedVectorType::get`.
    pub fn vector_type<T>(&'ctx self, elem: T, n: u32) -> VectorType<'ctx, ElemDyn, LenDyn, B>
    where
        T: Into<Type<'ctx, B>>,
    {
        self.as_view().vector_type(elem, n)
    }

    /// Scalable `<vscale x n x elem>` vector. Mirrors
    /// `ScalableVectorType::get`.
    pub fn scalable_vector_type<T>(
        &'ctx self,
        elem: T,
        n: u32,
    ) -> VectorType<'ctx, ElemDyn, LenDyn, B>
    where
        T: Into<Type<'ctx, B>>,
    {
        self.as_view().scalable_vector_type(elem, n)
    }

    /// Const-generic typed vector `<N x E>`. The element marker `E`
    /// projects the scalar element type and `N` pins the lane count,
    /// yielding a statically typed [`VectorType<'ctx, E, Len<N>, B>`].
    /// `const`-evaluated at monomorphisation: `N == 0` is a compile error.
    /// Mirrors `Type::getIntNTy` + `VectorType::get`.
    pub fn vector_type_n<E, const N: u32>(&'ctx self) -> VectorType<'ctx, E, Len<N>, B>
    where
        E: StaticVecElem<'ctx, B>,
    {
        const {
            assert!(N > 0, "vector length must be >= 1");
        }
        let elem = E::element_ir_type(self.module_ref());
        let id = self.core().ctx.fixed_vector_type(elem.id(), N);
        VectorType::new(id, self.module_ref())
    }

    /// Literal (unnamed) struct type `{ .. }`.
    pub fn struct_type<I, T>(&'ctx self, elements: I) -> StructType<'ctx, StructBodyDyn, B>
    where
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx, B>>,
    {
        self.as_view().struct_type(elements)
    }

    /// Packed literal struct type `<{ .. }>`.
    pub fn packed_struct_type<I, T>(&'ctx self, elements: I) -> StructType<'ctx, StructBodyDyn, B>
    where
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx, B>>,
    {
        self.as_view().packed_struct_type(elements)
    }

    /// Get or create the identified struct type `%name`, body unset.
    /// Delegates to [`ModuleView::get_or_insert_named_struct`].
    /// A fresh identified struct with no name — `%0 = type { i32 }`.
    /// Mirrors `StructType::create(Context)` called without a name.
    #[inline]
    pub fn anonymous_identified_struct(&'ctx self) -> StructType<'ctx, StructBodyDyn, B> {
        self.as_view().anonymous_identified_struct()
    }

    pub fn get_or_insert_named_struct(
        &'ctx self,
        name: &str,
    ) -> StructType<'ctx, StructBodyDyn, B> {
        self.as_view().get_or_insert_named_struct(name)
    }

    pub fn opaque_struct(&'ctx self, name: &str) -> IrResult<StructType<'ctx, Opaque, B>> {
        let (id, existed) = self.core().ctx.get_or_create_named_struct(name);
        if existed {
            let s = self
                .core()
                .ctx
                .type_data(id)
                .as_struct()
                .unwrap_or_else(|| unreachable!("named struct id stores struct data"));
            if s.body.borrow().is_some() {
                return Err(IrError::StructBodyAlreadySet {
                    name: name.to_owned(),
                });
            }
        }
        Ok(StructType::new(id, self.module_ref()))
    }

    /// Look up an existing identified struct type by name. Delegates to
    /// [`ModuleView::named_struct`].
    pub fn named_struct(&'ctx self, name: &str) -> Option<StructType<'ctx, StructBodyDyn, B>> {
        self.as_view().named_struct(name)
    }

    /// Idempotently intern schema `S`'s named struct type. Delegates to
    /// [`ModuleView::get_or_insert_struct_of`], which is where the schema
    /// traits reach it.
    pub fn get_or_insert_struct_of<S>(&'ctx self) -> IrResult<StructType<'ctx, BodySet, B>>
    where
        S: StructSchema,
    {
        self.as_view().get_or_insert_struct_of::<S>()
    }

    pub fn set_struct_body_dyn<I, T>(
        &'ctx self,
        st: StructType<'ctx, StructBodyDyn, B>,
        elements: I,
        packed: bool,
    ) -> IrResult<()>
    where
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx, B>>,
    {
        let elems: Box<[TypeSlot]> = elements.into_iter().map(|t| t.into().id()).collect();
        let body = StructBody {
            elements: elems,
            packed,
        };
        let s = self
            .core()
            .ctx
            .type_data(st.id)
            .as_struct()
            .unwrap_or_else(|| unreachable!("StructType wraps struct data"));
        if s.identity.is_literal() {
            return Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Struct,
                got: TypeKindLabel::Struct,
            });
        }
        self.core().ctx.set_named_struct_body(st.id, body)
    }

    pub fn set_struct_body<I, T>(
        &'ctx self,
        opaque: StructType<'ctx, Opaque, B>,
        elements: I,
        packed: bool,
    ) -> IrResult<StructType<'ctx, BodySet, B>>
    where
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx, B>>,
    {
        let elems: Box<[TypeSlot]> = elements.into_iter().map(|t| t.into().id()).collect();
        let body = StructBody {
            elements: elems,
            packed,
        };
        self.core().ctx.set_named_struct_body(opaque.id, body)?;
        Ok(opaque.retag::<BodySet>())
    }

    /// Function type `ret (params...)`.
    pub fn function_type<I, R, T>(
        &'ctx self,
        return_type: R,
        parameters: I,
    ) -> FunctionType<'ctx, B>
    where
        I: IntoIterator<Item = T>,
        R: Into<Type<'ctx, B>>,
        T: Into<Type<'ctx, B>>,
    {
        self.as_view().function_type(return_type, parameters)
    }

    /// Variadic function type `ret (params..., ...)`.
    pub fn variadic_function_type<I, R, T>(
        &'ctx self,
        return_type: R,
        parameters: I,
    ) -> FunctionType<'ctx, B>
    where
        I: IntoIterator<Item = T>,
        R: Into<Type<'ctx, B>>,
        T: Into<Type<'ctx, B>>,
    {
        self.as_view()
            .variadic_function_type(return_type, parameters)
    }

    /// A function type with no parameters. Avoids the empty-iterator
    /// inference cliff of [`function_type`](Self::function_type); see
    /// [`ModuleView::function_type_no_parameters`].
    pub fn function_type_no_parameters<R>(&'ctx self, return_type: R) -> FunctionType<'ctx, B>
    where
        R: Into<Type<'ctx, B>>,
    {
        self.as_view().function_type_no_parameters(return_type)
    }

    /// Variadic sibling of
    /// [`function_type_no_parameters`](Self::function_type_no_parameters).
    pub fn variadic_function_type_no_parameters<R>(
        &'ctx self,
        return_type: R,
    ) -> FunctionType<'ctx, B>
    where
        R: Into<Type<'ctx, B>>,
    {
        self.as_view()
            .variadic_function_type_no_parameters(return_type)
    }

    /// Fixed-arity typed function type: `Ret (Params...)`.
    pub fn typed_function_type<Ret, Params>(&'ctx self) -> IrResult<FunctionType<'ctx, B>>
    where
        Ret: FunctionReturn,
        Params: FunctionParamList,
    {
        let ret = Ret::ir_type(self.as_view())?;
        let params = Params::ir_types(self.as_view())?;
        Ok(self.function_type(ret, params))
    }

    /// Fixed-arity typed function type from a Rust function-pointer
    /// schema (`fn(...) -> Ret`).
    pub fn typed_function_type_of<Sig>(&'ctx self) -> IrResult<FunctionType<'ctx, B>>
    where
        Sig: FunctionSignature,
    {
        self.typed_function_type::<Sig::Ret, Sig::Params>()
    }

    /// Variadic typed function type: `Ret (Params..., ...)`. `Params`
    /// describes only the fixed-prefix parameters — the trailing `...`
    /// is not itself a schema-typed parameter.
    pub fn typed_varargs_function_type<Ret, Params>(&'ctx self) -> IrResult<FunctionType<'ctx, B>>
    where
        Ret: FunctionReturn,
        Params: FunctionParamList,
    {
        let ret = Ret::ir_type(self.as_view())?;
        let params = Params::ir_types(self.as_view())?;
        Ok(self.variadic_function_type(ret, params))
    }

    /// Variadic typed function type from a Rust function-pointer schema.
    pub fn typed_varargs_function_type_of<Sig>(&'ctx self) -> IrResult<FunctionType<'ctx, B>>
    where
        Sig: FunctionSignature,
    {
        self.typed_varargs_function_type::<Sig::Ret, Sig::Params>()
    }

    pub fn target_ext_type<Name, I, T, J>(
        &'ctx self,
        name: Name,
        type_params: I,
        int_params: J,
    ) -> TargetExtType<'ctx, B>
    where
        Name: Into<String>,
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx, B>>,
        J: IntoIterator<Item = u32>,
    {
        let name: String = name.into();
        let type_params: Box<[TypeSlot]> = type_params.into_iter().map(|t| t.into().id()).collect();
        let int_params: Box<[u32]> = int_params.into_iter().collect();
        TargetExtType::new(
            self.core()
                .ctx
                .target_ext_type(name, type_params, int_params),
            self.module_ref(),
        )
    }

    /// Declare a typed function `Ret @name(Params...)`, returning its storable
    /// [`TypedFunctionId`]. Resolve it back into the borrowing
    /// [`TypedFunctionValue`] facade with [`view`](Self::view).
    pub fn add_typed_function<Ret, Params, Name>(
        &'ctx self,
        name: Name,
        linkage: Linkage,
    ) -> IrResult<TypedFunctionId<Ret, Params, B>>
    where
        Ret: FunctionReturn,
        Params: FunctionParamList,
        Name: AsRef<str>,
    {
        let signature = self.typed_function_type::<Ret, Params>()?;
        let function = self.declare_function::<Ret::Marker>(name.as_ref(), signature, linkage)?;
        TypedFunctionValue::<Ret, Params, B>::try_from_function(function).map(|f| f.id())
    }

    /// Declare a typed function from a Rust function-pointer schema, returning
    /// its storable [`TypedFunctionId`].
    pub fn add_typed_function_of<Sig, Name>(
        &'ctx self,
        name: Name,
        linkage: Linkage,
    ) -> IrResult<TypedFunctionId<Sig::Ret, Sig::Params, B>>
    where
        Sig: FunctionSignature,
        Name: AsRef<str>,
    {
        let signature = self.typed_function_type_of::<Sig>()?;
        let function = self.declare_function::<<Sig::Ret as FunctionReturn>::Marker>(
            name.as_ref(),
            signature,
            linkage,
        )?;
        TypedFunctionValue::<Sig::Ret, Sig::Params, B>::try_from_function(function).map(|f| f.id())
    }

    /// Declare a variadic typed function `Ret @name(Params..., ...)`,
    /// returning its storable [`TypedVarArgsFunctionId`]. Resolve it back into
    /// the borrowing [`crate::function_signature::TypedVarArgsFunctionValue`]
    /// facade with [`view`](Self::view).
    pub fn add_typed_varargs_function<Ret, Params, Name>(
        &'ctx self,
        name: Name,
        linkage: Linkage,
    ) -> IrResult<TypedVarArgsFunctionId<Ret, Params, B>>
    where
        Ret: FunctionReturn,
        Params: FunctionParamList,
        Name: AsRef<str>,
    {
        let signature = self.typed_varargs_function_type::<Ret, Params>()?;
        let function = self.declare_function::<Ret::Marker>(name.as_ref(), signature, linkage)?;
        TypedVarArgsFunctionValue::<Ret, Params, B>::try_from_function(function).map(|f| f.id())
    }

    /// Declare a variadic typed function from a Rust function-pointer schema,
    /// returning its storable [`TypedVarArgsFunctionId`].
    pub fn add_typed_varargs_function_of<Sig, Name>(
        &'ctx self,
        name: Name,
        linkage: Linkage,
    ) -> IrResult<TypedVarArgsFunctionId<Sig::Ret, Sig::Params, B>>
    where
        Sig: FunctionSignature,
        Name: AsRef<str>,
    {
        let signature = self.typed_varargs_function_type_of::<Sig>()?;
        let function = self.declare_function::<<Sig::Ret as FunctionReturn>::Marker>(
            name.as_ref(),
            signature,
            linkage,
        )?;
        TypedVarArgsFunctionValue::<Sig::Ret, Sig::Params, B>::try_from_function(function)
            .map(|f| f.id())
    }

    /// Shared declaration tail for every public constructor: name
    /// validation (reserved intrinsic names, duplicate rejection)
    /// followed by the arena push. Return-marker/signature agreement is
    /// the CALLER's responsibility — the typed constructors derive the
    /// signature from their markers so a mismatch is unrepresentable,
    /// and [`add_function_dyn`](Self::add_function_dyn)'s `Dyn` matches
    /// every signature by definition. That caller contract stays closed
    /// because `FunctionReturn` cannot gain downstream impls: the
    /// `impl<S: StructSchema> FunctionReturn for S` blanket
    /// coherence-blocks direct external impls (and itself pins
    /// `Marker = Dyn`), so removing or narrowing that blanket would make
    /// the dropped marker check load-bearing again — re-add it here if
    /// that ever changes.
    fn declare_function<R>(
        &'ctx self,
        name: &str,
        signature: FunctionType<'ctx, B>,
        linkage: Linkage,
    ) -> IrResult<FunctionValue<'ctx, R, B>>
    where
        R: ReturnMarker,
    {
        reject_reserved_intrinsic_name(name)?;
        if !name.is_empty() && self.core().global_name_exists(name) {
            return Err(IrError::DuplicateFunctionName {
                name: name.to_owned(),
            });
        }
        self.core().push_function(
            name,
            signature,
            linkage,
            crate::CallingConv::default(),
            None,
            None,
        )
    }

    /// Add a function whose return marker is erased to [`Dyn`].
    ///
    /// The honest erased declaration path: it takes a runtime [`FunctionType`] and
    /// returns a `FunctionId<Dyn>`. Unlike [`add_typed_function`](Self::add_typed_function) it
    /// carries no static return marker and runs no return-marker check — `Dyn` matches
    /// every signature by definition. Use this for the parser and runtime-schema-driven
    /// tooling; for statically-typed authoring prefer
    /// [`add_typed_function`](Self::add_typed_function), whose turbofish *is* the schema
    /// (no separate `FunctionType`, and its parameters come back typed).
    ///
    /// Resolve the id back into a borrowing [`FunctionValue`] with
    /// [`view`](Self::view).
    pub fn add_function_dyn<Name>(
        &'ctx self,
        name: Name,
        signature: FunctionType<'ctx, B>,
        linkage: Linkage,
    ) -> IrResult<FunctionId<Dyn, B>>
    where
        Name: AsRef<str>,
    {
        // `R = Dyn` matches every signature, so no return-marker check is needed.
        self.declare_function::<Dyn>(name.as_ref(), signature, linkage)
            .map(|f| f.id())
    }

    pub fn intrinsic_descriptor_from_signature(
        &'ctx self,
        name: &str,
        fn_ty: FunctionType<'ctx, B>,
    ) -> IrResult<IntrinsicDescriptor<'ctx, B>> {
        self.core()
            .intrinsic_descriptor_from_signature::<B>(name, fn_ty)
    }

    /// Return the existing declaration for `descriptor`, or insert its canonical
    /// generated declaration.
    pub fn get_or_insert_intrinsic_declaration(
        &'ctx self,
        descriptor: &IntrinsicDescriptor<'ctx, B>,
    ) -> IrResult<FunctionId<Dyn, B>> {
        let function = self
            .core()
            .get_or_insert_intrinsic_declaration::<B>(descriptor)?;
        for (arg_index, name) in descriptor.argument_names() {
            let arg = function.param(arg_index)?;
            arg.set_name(self, name);
        }
        Ok(function.id())
    }

    pub fn get_or_insert_intrinsic_declaration_by_id<Overloads>(
        &'ctx self,
        id: IntrinsicId,
        overloads: Overloads,
    ) -> IrResult<FunctionId<Dyn, B>>
    where
        Overloads: Into<Box<[Type<'ctx, B>]>>,
    {
        let descriptor = IntrinsicDescriptor::new(id, overloads)?;
        self.get_or_insert_intrinsic_declaration(&descriptor)
    }

    pub fn get_or_insert_intrinsic_declaration_by_name<Name>(
        &'ctx self,
        name: Name,
    ) -> IrResult<FunctionId<Dyn, B>>
    where
        Name: AsRef<str>,
    {
        let name = name.as_ref();
        let id = IntrinsicId::lookup(name).ok_or_else(|| IrError::UnknownIntrinsic {
            name: name.to_owned(),
        })?;
        let descriptor = descriptor_for_name(self.module_ref(), id, name)?;
        self.get_or_insert_intrinsic_declaration(&descriptor)
    }

    /// Add a `global` whose type is derived from its `initializer`.
    ///
    /// The initializer is any [`IntoConstantValue`] — an existing constant
    /// handle or a Rust scalar literal (`add_global("marker", 0i32)`). The
    /// global's value type is the constant's type, so a creation-time type
    /// mismatch is unrepresentable.
    ///
    /// Returns the storable [`GlobalId`]; resolve it back into a borrowing
    /// [`GlobalVariable`] with [`view`](Self::view).
    pub fn add_global<N, C>(&'ctx self, name: N, initializer: C) -> IrResult<GlobalId<B>>
    where
        N: Into<String>,
        C: IntoConstantValue<'ctx, B>,
    {
        let constant = initializer.into_constant(self.module_ref());
        GlobalBuilder::<B>::new(self.module_ref(), name, constant.ty())
            .initializer(constant)
            .build()
    }

    /// Add a `constant` whose type is derived from its `initializer`.
    ///
    /// Like [`add_global`](Self::add_global) but marks the global as
    /// `constant` rather than mutable.
    pub fn add_global_constant<N, C>(&'ctx self, name: N, initializer: C) -> IrResult<GlobalId<B>>
    where
        N: Into<String>,
        C: IntoConstantValue<'ctx, B>,
    {
        let constant = initializer.into_constant(self.module_ref());
        GlobalBuilder::<B>::new(self.module_ref(), name, constant.ty())
            .constant()
            .initializer(constant)
            .build()
    }

    /// Add a global with no initializer, declared at `value_type`.
    ///
    /// For the declaration-only case where there is no initializer to
    /// derive the type from. Unlike [`add_external_global`](Self::add_external_global),
    /// this uses the module's default linkage. Accepts any
    /// `impl Into<Type>` so a typed handle needn't be widened via
    /// `.as_type()`.
    pub fn add_global_uninitialized<N, T>(
        &'ctx self,
        name: N,
        value_type: T,
    ) -> IrResult<GlobalId<B>>
    where
        N: Into<String>,
        T: Into<Type<'ctx, B>>,
    {
        GlobalBuilder::<B>::new(self.module_ref(), name, value_type.into()).build()
    }

    pub fn add_external_global<N, T>(&'ctx self, name: N, value_type: T) -> IrResult<GlobalId<B>>
    where
        N: Into<String>,
        T: Into<Type<'ctx, B>>,
    {
        GlobalBuilder::<B>::new(self.module_ref(), name, value_type.into())
            .linkage(Linkage::External)
            .build()
    }

    pub fn global_builder<N, T>(&'ctx self, name: N, value_type: T) -> GlobalBuilder<'ctx, B>
    where
        N: Into<String>,
        T: Into<Type<'ctx, B>>,
    {
        GlobalBuilder::new(self.module_ref(), name, value_type.into())
    }

    pub fn alias_builder<C, Name>(
        &'ctx self,
        name: Name,
        value_type: Type<'ctx, B>,
        aliasee: C,
    ) -> GlobalAliasBuilder<'ctx, B>
    where
        C: IsConstant<'ctx, B>,
        Name: Into<String>,
    {
        GlobalAliasBuilder::new(self.module_ref(), name, value_type, aliasee)
    }

    pub fn alias_empty(&'ctx self) -> bool {
        self.core().alias_empty()
    }

    pub fn ifunc_builder<C, Name>(
        &'ctx self,
        name: Name,
        value_type: Type<'ctx, B>,
        resolver: C,
    ) -> GlobalIfuncBuilder<'ctx, B>
    where
        C: IsConstant<'ctx, B>,
        Name: Into<String>,
    {
        GlobalIfuncBuilder::new(self.module_ref(), name, value_type, resolver)
    }

    pub fn ifunc_empty(&'ctx self) -> bool {
        self.core().ifunc_empty()
    }

    pub fn global_empty(&'ctx self) -> bool {
        self.core().global_empty()
    }

    pub fn set_source_filename<N>(&'ctx self, filename: N)
    where
        N: Into<String>,
    {
        self.core().set_source_filename(filename);
    }

    pub fn clear_source_filename(&'ctx self) {
        self.core().clear_source_filename();
    }

    /// Replace the data layout with an already-parsed [`DataLayout`].
    /// Infallible: parse failures belong to [`DataLayout::parse`], not
    /// to the setter.
    pub fn set_data_layout(&'ctx self, layout: DataLayout) {
        self.core().set_data_layout(layout);
    }

    pub fn set_target_triple<T>(&'ctx self, triple: T)
    where
        T: Into<String>,
    {
        self.core().set_target_triple(triple);
    }

    pub fn clear_target_triple(&'ctx self) {
        self.core().clear_target_triple();
    }

    pub fn set_module_asm<A>(&'ctx self, asm: A)
    where
        A: Into<String>,
    {
        self.core().set_module_asm(asm);
    }

    pub fn append_module_asm<A>(&'ctx self, line: A)
    where
        A: AsRef<str>,
    {
        self.core().append_module_asm(line);
    }

    pub fn get_or_insert_comdat<Name: AsRef<str>>(&'ctx self, name: Name) -> ComdatRef<'ctx, B> {
        self.core().get_or_insert_comdat::<B, _>(name)
    }

    pub fn inline_asm<Asm, Constraints>(
        &'ctx self,
        fn_ty: FunctionType<'ctx, B>,
        asm: Asm,
        constraints: Constraints,
        options: InlineAsmOptions,
    ) -> InlineAsm<'ctx, B>
    where
        Asm: Into<String>,
        Constraints: Into<String>,
    {
        let ptr_ty = self.ptr_type(0).as_type().id();
        let data = InlineAsmData {
            asm_string: asm.into(),
            constraint_string: constraints.into(),
            fn_ty: fn_ty.as_type().id(),
            has_side_effects: options.has_side_effects(),
            is_align_stack: options.is_align_stack(),
            can_unwind: options.can_unwind(),
            dialect: options.dialect(),
        };
        let id = self.core().ctx.push_value(ValueData {
            ty: ptr_ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::InlineAsm(data),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        InlineAsm::from_parts(id, self.module_ref(), ptr_ty)
    }

    // ---- Metadata ----
    //
    // The public face of the metadata currency. Every entry point that accepts
    // a [`MetadataId`] is fallible, because a caller can always hand over an id
    // minted by a *different* module: that is `IrError::ForeignMetadataId`,
    // never a silent mis-resolve against this module's arena. The two that
    // cannot fail — interning a string and reserving a fresh node — say so by
    // returning the id directly.

    /// Intern a metadata string node, returning its storable
    /// [`MetadataId`]. Mirrors `MDString::get`.
    pub fn metadata_string<S>(&'ctx self, s: S) -> MetadataId<B>
    where
        S: Into<String>,
    {
        self.core().metadata_string(s)
    }

    /// Create a metadata tuple node. Mirrors `MDTuple::get`.
    ///
    /// `Err(IrError::ForeignMetadataId)` if any operand was minted by another
    /// module.
    pub fn metadata_tuple<Ops>(&'ctx self, operands: Ops) -> IrResult<MetadataId<B>>
    where
        Ops: AsRef<[MetadataId<B>]>,
    {
        self.core().metadata_tuple(operands)
    }

    /// Create a metadata tuple node with explicit distinctness.
    ///
    /// `Err(IrError::ForeignMetadataId)` if any operand was minted by another
    /// module.
    pub fn metadata_tuple_with_distinct<Ops>(
        &'ctx self,
        distinct: bool,
        operands: Ops,
    ) -> IrResult<MetadataId<B>>
    where
        Ops: AsRef<[MetadataId<B>]>,
    {
        self.core().metadata_tuple_with_distinct(distinct, operands)
    }

    /// Wrap a constant as a typed metadata operand (`i64 1`, `ptr null`, ...).
    ///
    /// `Err(IrError::ForeignValueId)` when `c` belongs to another module. Under
    /// a shared brand ([`DynBrand`], or a re-issued named brand) the handle's
    /// type says nothing about *which* module minted it, so the node would
    /// otherwise be interned here pointing at a value slot that means something
    /// else in this arena.
    pub fn metadata_constant<C>(&'ctx self, c: C) -> IrResult<MetadataId<B>>
    where
        C: IsConstant<'ctx, B>,
    {
        let constant = c.as_constant();
        if constant.module.id() != self.core().id() {
            return Err(IrError::ForeignValueId);
        }
        Ok(self.core().metadata_constant_value(constant.id))
    }

    /// Create a specialized `DI*` metadata node.
    ///
    /// `Err(IrError::ForeignMetadataId)` if any field references a node minted
    /// by another module.
    pub fn metadata_specialized(
        &'ctx self,
        node: SpecializedMetadataNode<B>,
    ) -> IrResult<MetadataId<B>> {
        self.core().metadata_specialized(node)
    }

    /// Store an already-built metadata node and return its id.
    ///
    /// `Err(IrError::ForeignMetadataId)` if the node references a node — or a
    /// value — minted by another module.
    pub fn metadata_node(&'ctx self, kind: MetadataKind<B>) -> IrResult<MetadataId<B>> {
        self.core().metadata_node(kind)
    }

    /// Wrap a metadata node so it can appear where a [`Value`] is expected.
    /// Mirrors LLVM's uniqued `MetadataAsValue::get`.
    ///
    /// `Err(IrError::ForeignMetadataId)` when `md` was minted by another
    /// module, `Err(IrError::UnknownMetadataSlot)` when it names nothing here.
    pub fn metadata_as_value(&'ctx self, md: MetadataId<B>) -> IrResult<Value<'ctx, B>> {
        let slot = self.core().metadata_slot_of(md)?;
        let ty = self.core().ctx.metadata();
        if let Some(&id) = self.core().metadata_as_value_cache.borrow().get(&slot) {
            return Ok(Value::from_parts(id, self.module_ref(), ty));
        }
        let id = self.core().ctx.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::MetadataAsValue(slot),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.core()
            .metadata_as_value_cache
            .borrow_mut()
            .insert(slot, id);
        Ok(Value::from_parts(id, self.module_ref(), ty))
    }

    /// Reserve a fresh metadata node id with placeholder content, to be filled
    /// via [`metadata_set`](Self::metadata_set). Used by the parser to resolve
    /// forward references without assuming textual `!N` slots equal arena
    /// indices.
    pub fn metadata_reserve(&'ctx self) -> MetadataId<B> {
        self.core().metadata_reserve()
    }

    /// Overwrite a reserved metadata node, pairing with `metadata_reserve`.
    ///
    /// `Err(IrError::ForeignMetadataId)` when `id` was minted by another
    /// module; `Err(IrError::UnknownMetadataSlot)` when it names nothing here.
    /// It used to no-op silently, which the 2.0 contract forbids.
    pub fn metadata_set(&'ctx self, id: MetadataId<B>, kind: MetadataKind<B>) -> IrResult<()> {
        self.core().metadata_set(id, kind)
    }

    /// Look up a metadata node by id. `None` when `id` belongs to another
    /// module or names nothing here — never another module's node.
    pub fn metadata_get(&'ctx self, id: MetadataId<B>) -> Option<MetadataKind<B>> {
        self.core().metadata_get(id)
    }

    pub fn metadata_count(&'ctx self) -> usize {
        self.core().metadata_count()
    }

    /// Get or create a named metadata node with the given name, minting its
    /// storable [`NamedMetadataId`]. Mirrors `Module::getOrInsertNamedMetadata`.
    ///
    /// The name is anything that converts into a [`NamedMetadataName`]: the
    /// well-known variants directly, or any `&str` / `String` spelling (which
    /// classifies itself, falling back to
    /// [`NamedMetadataName::Custom`]).
    pub fn get_or_insert_named_metadata<Name>(&'ctx self, name: Name) -> NamedMetadataId<B>
    where
        Name: Into<NamedMetadataName>,
    {
        NamedMetadataId::from_stored(self.core().get_or_insert_named_metadata(name.into()))
    }

    /// Look up an existing named metadata node by name. `None` when this
    /// module holds no node with that name. Mirrors
    /// `Module::getNamedMetadata`.
    pub fn named_metadata(&'ctx self, name: &NamedMetadataName) -> Option<NamedMetadataId<B>> {
        self.core()
            .named_metadata(name)
            .map(NamedMetadataId::from_stored)
    }

    /// Append an operand to a named metadata node. Mirrors
    /// `NamedMDNode::addOperand`.
    ///
    /// `Err(IrError::ForeignNamedMetadataId)` when `id` was minted by another
    /// module, `Err(IrError::ForeignMetadataId)` when `operand` was. There is
    /// no unknown-slot case: the named-metadata list is append-only, so a
    /// native id's slot keeps naming the node it was minted for.
    pub fn named_metadata_add_operand(
        &'ctx self,
        id: NamedMetadataId<B>,
        operand: MetadataId<B>,
    ) -> IrResult<()> {
        self.core().named_metadata_add_operand(id, operand)
    }

    /// Look up a named metadata node by id, cloning it out. `None` when `id`
    /// belongs to another module — never another module's node. A native id
    /// always resolves: the named-metadata list is append-only.
    pub fn named_metadata_get(&'ctx self, id: NamedMetadataId<B>) -> Option<NamedMetadataNode<B>> {
        self.core().named_metadata_get(id)
    }

    pub fn named_metadata_count(&'ctx self) -> usize {
        self.core().named_metadata_count()
    }

    // ---- Module flags ----
    //
    // Backed entirely by the `llvm.module.flags` named metadata node — each
    // flag is the three-operand tuple `!{i32 behavior, !"key", value}`
    // upstream stores, so parsed IR, the printer, and the round-trip
    // contract are untouched. The typed vocabulary lives in
    // [`crate::module_flags`].

    /// Append a module flag. Mirrors `Module::addModuleFlag`
    /// (`lib/IR/Module.cpp`): builds the tuple
    /// `!{i32 behavior, !"key", value}` and appends it to the
    /// `llvm.module.flags` named metadata node, creating the node if absent.
    ///
    /// `Err(IrError::ForeignMetadataId)` when `value` was minted by another
    /// module (checked before anything is interned);
    /// `Err(IrError::UnknownMetadataSlot)` when it names nothing here.
    pub fn add_module_flag<Key>(
        &'ctx self,
        behavior: ModuleFlagBehavior,
        key: Key,
        value: MetadataId<B>,
    ) -> IrResult<()>
    where
        Key: Into<ModuleFlagKey>,
    {
        let tuple = self.module_flag_tuple_id(behavior, &key.into(), value)?;
        let flags = self.get_or_insert_named_metadata(NamedMetadataName::ModuleFlags);
        self.named_metadata_add_operand(flags, tuple)
    }

    /// Like [`add_module_flag`](Self::add_module_flag), but replaces the
    /// existing flag with the same key in place (preserving its position)
    /// instead of appending a duplicate. Mirrors `Module::setModuleFlag`
    /// (`lib/IR/Module.cpp`).
    ///
    /// `Err(IrError::ForeignMetadataId)` when `value` was minted by another
    /// module; `Err(IrError::UnknownMetadataSlot)` when it names nothing
    /// here.
    pub fn set_module_flag<Key>(
        &'ctx self,
        behavior: ModuleFlagBehavior,
        key: Key,
        value: MetadataId<B>,
    ) -> IrResult<()>
    where
        Key: Into<ModuleFlagKey>,
    {
        let key = key.into();
        let tuple = self.module_flag_tuple_id(behavior, &key, value)?;
        if self
            .core()
            .replace_module_flag(key.key(), tuple.into_stored(self.core().id())?)
        {
            return Ok(());
        }
        let flags = self.get_or_insert_named_metadata(NamedMetadataName::ModuleFlags);
        self.named_metadata_add_operand(flags, tuple)
    }

    /// The value operand of the flag named `key`, or `None` when no flag
    /// carries that key. Mirrors `Module::getModuleFlag`
    /// (`lib/IR/Module.cpp`). Resolve the returned id with
    /// [`metadata_get`](Self::metadata_get).
    pub fn module_flag(&'ctx self, key: &ModuleFlagKey) -> Option<MetadataId<B>> {
        self.core()
            .module_flag_value(key.key())
            .map(MetadataId::from_stored)
    }

    /// Every well-formed module flag, in `llvm.module.flags` operand order.
    /// Mirrors `Module::getModuleFlagsMetadata` (`lib/IR/Module.cpp`);
    /// like upstream's read path, malformed tuples are skipped silently —
    /// rejecting them is [`verify`](Self::verify)'s job.
    ///
    /// The flags live behind a `RefCell`, so the iterator walks a snapshot
    /// taken at call time rather than borrowing the module.
    pub fn module_flags(
        &'ctx self,
    ) -> impl ExactSizeIterator<Item = ModuleFlagEntry<B>>
    + DoubleEndedIterator
    + core::iter::FusedIterator
    + use<B> {
        self.core()
            .module_flags_stored()
            .into_iter()
            .map(ModuleFlagEntry::from_stored)
    }

    /// Shared tuple constructor for
    /// [`add_module_flag`](Self::add_module_flag) /
    /// [`set_module_flag`](Self::set_module_flag) — the `Ops[3]` array of
    /// `Module::addModuleFlag`. Tag-checks `value` *before* interning the
    /// behavior constant and key string, so a foreign id leaves no junk
    /// nodes behind.
    fn module_flag_tuple_id(
        &'ctx self,
        behavior: ModuleFlagBehavior,
        key: &ModuleFlagKey,
        value: MetadataId<B>,
    ) -> IrResult<MetadataId<B>> {
        self.core().metadata_slot_of(value)?;
        let behavior_md = self.metadata_constant(self.i32_type().const_int(behavior.raw()))?;
        let key_md = self.metadata_string(key.key());
        self.metadata_tuple([behavior_md, key_md, value])
    }

    pub fn append_use_list_order(&'ctx self, record: UseListOrderRecord) -> IrResult<()> {
        self.core().append_use_list_order(record)
    }

    pub fn append_use_list_order_bb(&'ctx self, record: UseListOrderBbRecord) -> IrResult<()> {
        self.core().append_use_list_order_bb(record)
    }

    pub fn set_attribute_group(&'ctx self, id: u32, storage: AttributeStorage) {
        self.core().set_attribute_group(id, storage);
    }

    /// Verify the module and consume it into the `Verified` state.
    ///
    /// The owned core is **moved** into the new-state token; the `Unverified`
    /// token is gone, which is the whole point of the linear typestate. (This
    /// is also why `Module` must never grow a `Drop` impl — moving a field out
    /// of a `Drop` type is E0509.)
    pub fn verify(self) -> IrResult<Module<B, Verified>> {
        // A *short* view, not `as_view()`: `self` is about to be moved from, so
        // it cannot lend a `'ctx`-long borrow of itself.
        Verifier::new(ModuleView::<B>::new(self.core())).run()?;
        Ok(Module {
            core: self.core,
            registration: self.registration,
            _brand: PhantomData,
            _state: PhantomData,
        })
    }
}

impl<B: ModuleBrand> Module<B, Verified> {
    /// Strip the verified state after mutation is required. Moves the owned
    /// core into the `Unverified` token.
    pub fn unverify(self) -> Module<B, Unverified> {
        Module {
            core: self.core,
            registration: self.registration,
            _brand: PhantomData,
            _state: PhantomData,
        }
    }
}

impl<B: ModuleBrand> Module<B, Unverified> {
    /// Re-stamp this token `Verified` **without re-running the verifier**.
    ///
    /// Crate-internal plumbing for the read-only `Dyn…` pass pipelines
    /// (`crate::pass_manager`): a boxed pass runs behind an erased trait whose
    /// entry point is typed to take a `&Module<Unverified>` mutation token, but a
    /// read-only container holds only `Inspect` passes — whose rung token is `()`,
    /// so the reference is projected away and never reaches a mutator. The
    /// container therefore [`unverify`](Module::unverify)s once, lends the
    /// resulting token to the erased signature, and re-stamps it here on the way
    /// out (no re-verification; D8). The container's `push` bound
    /// (`Inspect`-only) is what makes the no-mutation invariant structural
    /// rather than assumed.
    ///
    /// This replaces the old `scratch_unverified`, which handed out a *second*
    /// live token over the same storage — impossible now that a token owns its
    /// core, and undesirable regardless.
    pub(crate) fn assume_verified(self) -> Module<B, Verified> {
        Module {
            core: self.core,
            registration: self.registration,
            _brand: PhantomData,
            _state: PhantomData,
        }
    }
}

// `&'ctx TypeData` borrows are *not* mutated; they point into a
// `boxcar::Vec` that only ever appends. The `RefCell`s inside `Context`
// guard hashmap mutation, never the arena, so iteration / accessor
// borrows of payload data are safe even while construction proceeds.
//
// `Module<B, S>: !Sync` falls out of those `RefCell` fields: a module is not
// shared between threads. It *is* `Send` — it owns its storage and borrows
// nothing, and the brand rides as `Invariant<B>`, which is `Send` whatever `B`
// is — so a module can be moved to another thread mid-authoring and finished
// there. See `tests/module_send.rs`.

impl<B: ModuleBrand, S> core::fmt::Display for Module<B, S> {
    /// Print the module as textual `.ll`. Mirrors `Module::print` from
    /// `llvm/lib/IR/AsmWriter.cpp`.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::asm_writer::fmt_module(f, &self.core)
    }
}
