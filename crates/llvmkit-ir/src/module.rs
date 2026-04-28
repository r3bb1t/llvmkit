//! Top-level IR container. Mirrors `llvm/include/llvm/IR/Module.h` and
//! `llvm/lib/IR/Module.cpp`.
//!
//! Phase A surface: name + every type constructor required by
//! `IRBuilder` and the `.ll` parser. Functions, globals, named metadata,
//! and the data-layout subsystem land in Phase D.
//!
//! ## Identity model
//!
//! Each `Module` carries:
//!
//! - A globally-unique [`ModuleId`] (process-wide atomic counter)
//!   that backs handle equality and hashing across modules.
//! - A `'ctx` brand parameter that scopes every typed handle the module
//!   produces. Cross-module mixing is rejected by the borrow checker for
//!   the common short-lived borrow case (each `let m = Module::new()`
//!   gets a fresh elided lifetime); the [`ModuleRef`] helper inside each
//!   handle additionally compares by `ModuleId`, so even when lifetimes
//!   happen to unify, two distinct modules' handles do not.
//!
//! ## Borrow shape
//!
//! Type constructors take `&'ctx self` so the returned typed handles
//! borrow the module for at least `'ctx`. The module's interior is
//! mutated through `RefCell` from the same `&self`, so this does not
//! block subsequent type or value construction.

use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::num::NonZeroU32;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::derived_types::{
    ArrayType, FloatType, FunctionType, IntType, LabelType, MetadataType, PointerType, StructType,
    TargetExtType, TokenType, VectorType, VoidType,
};
use crate::error::{IrError, IrResult, TypeKindLabel};
use crate::float_kind::{BFloat, Fp128, Half, PpcFp128, X86Fp80};
use crate::int_width::IntDyn;
use crate::llvm_context::Context;
use crate::r#type::{MAX_INT_BITS, MIN_INT_BITS, StructBody, Type, TypeId};
use crate::typed_pointer_type::TypedPointerType;

// --------------------------------------------------------------------------
// ModuleId
// --------------------------------------------------------------------------

/// Globally-unique module identifier. Assigned at construction by an
/// atomic counter; never reused within a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(NonZeroU32);

impl ModuleId {
    /// Allocate the next unused id. The counter starts at 1 so the
    /// underlying `NonZeroU32` always has its niche populated.
    fn fresh() -> Self {
        // `Relaxed` is fine: the counter only needs uniqueness, not
        // happens-before ordering with any other memory operation.
        static NEXT: AtomicU32 = AtomicU32::new(1);
        let raw = NEXT.fetch_add(1, Ordering::Relaxed);
        let nz = NonZeroU32::new(raw).expect("ModuleId counter overflow (>u32::MAX modules)");
        Self(nz)
    }

    /// Raw integer value. Useful for diagnostics.
    #[inline]
    pub fn as_u32(self) -> u32 {
        self.0.get()
    }
}

// --------------------------------------------------------------------------
// ModuleRef helper
// --------------------------------------------------------------------------

/// `&Module<'ctx>` wrapped so its `Hash`/`PartialEq`/`Eq`/`Debug` go
/// through [`ModuleId`] instead of pointer-identity or deep field
/// comparison.
///
/// This is the single hand-rolled `Hash`/`Eq` impl in the IR crate;
/// every public type and value handle holds a `ModuleRef<'ctx>` and
/// derives the rest of its trait surface.
#[derive(Clone, Copy)]
pub struct ModuleRef<'ctx>(&'ctx Module<'ctx>);

impl<'ctx> ModuleRef<'ctx> {
    #[inline]
    pub(crate) fn new(m: &'ctx Module<'ctx>) -> Self {
        Self(m)
    }

    /// Borrow the underlying [`Module`].
    #[inline]
    pub fn module(self) -> &'ctx Module<'ctx> {
        self.0
    }

    /// Owning module's [`ModuleId`].
    #[inline]
    pub fn id(self) -> ModuleId {
        self.0.id
    }

    /// Crate-internal: resolve a [`TypeId`] to its payload via the
    /// owning module's context.
    #[inline]
    pub(crate) fn type_data(self, id: crate::r#type::TypeId) -> &'ctx crate::r#type::TypeData {
        self.0.context().type_data(id)
    }

    /// Crate-internal: resolve a [`ValueId`](crate::value::ValueId) to its
    /// payload via the owning module's context.
    #[inline]
    pub(crate) fn value_data(self, id: crate::value::ValueId) -> &'ctx crate::value::ValueData {
        self.0.context().value_data(id)
    }
}

impl PartialEq for ModuleRef<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}
impl Eq for ModuleRef<'_> {}
impl Hash for ModuleRef<'_> {
    #[inline]
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.0.id.hash(h);
    }
}
impl core::fmt::Debug for ModuleRef<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("ModuleRef").field(&self.0.id).finish()
    }
}

// --------------------------------------------------------------------------
// Module
// --------------------------------------------------------------------------

/// Top-level IR container.
pub struct Module<'ctx> {
    id: ModuleId,
    name: String,
    ctx: Context,
    /// Functions defined in this module, in declaration order.
    /// Stored as a `RefCell<Vec<ValueId>>` so `add_function` can mutate
    /// while the same `&'ctx self` borrow is held by call sites.
    functions: core::cell::RefCell<Vec<crate::value::ValueId>>,
    /// Module-level name -> function value-id table.
    function_by_name: core::cell::RefCell<std::collections::HashMap<String, crate::value::ValueId>>,
    /// Globals defined in this module, in declaration order.
    /// Mirrors `Module::GlobalList`. Stored under the same shape as
    /// `functions` so the AsmWriter can iterate in source order.
    globals: core::cell::RefCell<Vec<crate::value::ValueId>>,
    /// Module-level name -> global value-id table.
    global_by_name: core::cell::RefCell<std::collections::HashMap<String, crate::value::ValueId>>,
    /// Module-level COMDAT entries. Mirrors `Module::ComdatSymTab`.
    /// Stored in a `boxcar::Vec` for stable `&ComdatData` references
    /// under `&self`, so [`ComdatRef`](crate::comdat::ComdatRef) can
    /// hand out borrows without runtime cell juggling.
    comdats: boxcar::Vec<crate::comdat::ComdatData>,
    /// Name -> comdat-id table. Mirrors
    /// `Module::ComdatSymTab` lookup.
    comdat_by_name: core::cell::RefCell<std::collections::HashMap<String, crate::comdat::ComdatId>>,
    /// Brand carrier. Without it, `Module<'ctx>` would have no use of
    /// `'ctx` in its fields (since `Context` is lifetime-free) and the
    /// parameter would be unconstrained.
    _brand: PhantomData<&'ctx ()>,
}

impl<'ctx> Module<'ctx> {
    /// Construct a fresh, empty module with a freshly-allocated
    /// [`ModuleId`].
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: ModuleId::fresh(),
            name: name.into(),
            ctx: Context::new(),
            functions: core::cell::RefCell::new(Vec::new()),
            function_by_name: core::cell::RefCell::new(std::collections::HashMap::new()),
            globals: core::cell::RefCell::new(Vec::new()),
            global_by_name: core::cell::RefCell::new(std::collections::HashMap::new()),
            comdats: boxcar::Vec::new(),
            comdat_by_name: core::cell::RefCell::new(std::collections::HashMap::new()),
            _brand: PhantomData,
        }
    }

    /// Module identifier (the human-readable name).
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// This module's globally-unique id.
    #[inline]
    pub fn id(&self) -> ModuleId {
        self.id
    }

    /// Crate-internal access to the interning context.
    #[inline]
    pub(crate) fn context(&self) -> &Context {
        &self.ctx
    }

    // ---- Primitive type constructors ----

    /// `void`.
    pub fn void_type(&'ctx self) -> VoidType<'ctx> {
        VoidType::new(self.ctx.void(), self)
    }

    /// `label`.
    pub fn label_type(&'ctx self) -> LabelType<'ctx> {
        LabelType::new(self.ctx.label(), self)
    }

    /// `metadata`.
    pub fn metadata_type(&'ctx self) -> MetadataType<'ctx> {
        MetadataType::new(self.ctx.metadata(), self)
    }

    /// `token`.
    pub fn token_type(&'ctx self) -> TokenType<'ctx> {
        TokenType::new(self.ctx.token(), self)
    }

    /// `half`.
    pub fn half_type(&'ctx self) -> FloatType<'ctx, Half> {
        FloatType::new(self.ctx.half(), self)
    }

    /// `bfloat`.
    pub fn bfloat_type(&'ctx self) -> FloatType<'ctx, BFloat> {
        FloatType::new(self.ctx.bfloat(), self)
    }

    /// `float` (32-bit IEEE 754).
    pub fn f32_type(&'ctx self) -> FloatType<'ctx, f32> {
        FloatType::new(self.ctx.float(), self)
    }

    /// `double` (64-bit IEEE 754).
    pub fn f64_type(&'ctx self) -> FloatType<'ctx, f64> {
        FloatType::new(self.ctx.double(), self)
    }

    /// `fp128` (128-bit IEEE 754 binary128).
    pub fn fp128_type(&'ctx self) -> FloatType<'ctx, Fp128> {
        FloatType::new(self.ctx.fp128(), self)
    }

    /// `x86_fp80` (80-bit X87 extended precision).
    pub fn x86_fp80_type(&'ctx self) -> FloatType<'ctx, X86Fp80> {
        FloatType::new(self.ctx.x86_fp80(), self)
    }

    /// `ppc_fp128` (PowerPC double-double).
    pub fn ppc_fp128_type(&'ctx self) -> FloatType<'ctx, PpcFp128> {
        FloatType::new(self.ctx.ppc_fp128(), self)
    }

    /// `x86_amx` (X86 AMX matrix register).
    pub fn x86_amx_type(&'ctx self) -> Type<'ctx> {
        Type::new(self.ctx.x86_amx(), self)
    }

    // ---- Integer types ----

    /// `i1`. Convenience for [`Self::custom_width_int_type`] with `bits = 1`.
    pub fn bool_type(&'ctx self) -> IntType<'ctx, bool> {
        IntType::new(self.ctx.int_type(1), self)
    }
    /// Alias for [`Self::bool_type`] mirroring inkwell's spelling.
    #[inline]
    pub fn i1_type(&'ctx self) -> IntType<'ctx, bool> {
        self.bool_type()
    }
    pub fn i8_type(&'ctx self) -> IntType<'ctx, i8> {
        IntType::new(self.ctx.int_type(8), self)
    }
    pub fn i16_type(&'ctx self) -> IntType<'ctx, i16> {
        IntType::new(self.ctx.int_type(16), self)
    }
    pub fn i32_type(&'ctx self) -> IntType<'ctx, i32> {
        IntType::new(self.ctx.int_type(32), self)
    }
    pub fn i64_type(&'ctx self) -> IntType<'ctx, i64> {
        IntType::new(self.ctx.int_type(64), self)
    }
    pub fn i128_type(&'ctx self) -> IntType<'ctx, i128> {
        IntType::new(self.ctx.int_type(128), self)
    }

    /// Arbitrary-width integer (`iN`). Returns `Err` if `bits` is
    /// outside `[`[`MIN_INT_BITS`]`, `[`MAX_INT_BITS`]`]`.
    pub fn custom_width_int_type(&'ctx self, bits: u32) -> IrResult<IntType<'ctx, IntDyn>> {
        if !(MIN_INT_BITS..=MAX_INT_BITS).contains(&bits) {
            return Err(IrError::InvalidIntegerWidth { bits });
        }
        Ok(IntType::new(self.ctx.int_type(bits), self))
    }

    /// Const-generic integer type. Returns [`IntType<'ctx, Width<N>>`](
    /// crate::Width). Const-evaluated range check at monomorphisation:
    /// `N` outside `MIN_INT_BITS..=MAX_INT_BITS` is a compile error.
    /// Mirrors `Type::getIntNTy(C, N)`.
    pub fn int_type_n<const N: u32>(&'ctx self) -> IntType<'ctx, crate::int_width::Width<N>> {
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
    pub fn ptr_type(&'ctx self, addr_space: u32) -> PointerType<'ctx> {
        PointerType::new(self.ctx.ptr_type(addr_space), self)
    }

    /// Typed pointer (legacy GPU-target form).
    pub fn typed_pointer_type(
        &'ctx self,
        pointee: impl Into<Type<'ctx>>,
        addr_space: u32,
    ) -> TypedPointerType<'ctx> {
        let pointee_id = pointee.into().id();
        TypedPointerType::new(self.ctx.typed_pointer_type(pointee_id, addr_space), self)
    }

    // ---- Array / vector ----

    /// `[N x T]`.
    pub fn array_type(&'ctx self, elem: impl Into<Type<'ctx>>, n: u64) -> ArrayType<'ctx> {
        let elem_id = elem.into().id();
        ArrayType::new(self.ctx.array_type(elem_id, n), self)
    }

    /// Fixed `<N x T>` or scalable `<vscale x N x T>` vector.
    pub fn vector_type(
        &'ctx self,
        elem: impl Into<Type<'ctx>>,
        n: u32,
        scalable: bool,
    ) -> VectorType<'ctx> {
        let elem_id = elem.into().id();
        let id = if scalable {
            self.ctx.scalable_vector_type(elem_id, n)
        } else {
            self.ctx.fixed_vector_type(elem_id, n)
        };
        VectorType::new(id, self)
    }

    // ---- Struct ----

    /// Literal struct.
    pub fn struct_type<I, T>(&'ctx self, elements: I, packed: bool) -> StructType<'ctx>
    where
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx>>,
    {
        let elems: Box<[TypeId]> = elements.into_iter().map(|t| t.into().id()).collect();
        StructType::new(self.ctx.literal_struct_type(elems, packed), self)
    }

    /// Identified (named) struct. If `name` already exists, returns the
    /// existing handle (which may be opaque if its body has not yet
    /// been set).
    pub fn named_struct(&'ctx self, name: &str) -> StructType<'ctx> {
        let (id, _existed) = self.ctx.get_or_create_named_struct(name);
        StructType::new(id, self)
    }

    /// Identified opaque struct in its [`Opaque`](crate::Opaque)
    /// typestate. Doctrine D1: returns a handle whose `B` parameter
    /// is `Opaque`, gating [`Self::set_struct_body_typed`] (which
    /// consumes `Opaque` -> `BodySet`) at compile time.
    ///
    /// Errors with [`IrError::StructBodyAlreadySet`] if `name` was
    /// previously declared and already has a body. The runtime check
    /// is required because `'ctx`-shared contexts may have created
    /// the struct earlier.
    pub fn opaque_struct(
        &'ctx self,
        name: &str,
    ) -> IrResult<StructType<'ctx, crate::struct_body_state::Opaque>> {
        let (id, existed) = self.ctx.get_or_create_named_struct(name);
        if existed {
            let s = self
                .ctx
                .type_data(id)
                .as_struct()
                .expect("named struct invariant");
            if s.body.borrow().is_some() {
                return Err(IrError::StructBodyAlreadySet {
                    name: name.to_owned(),
                });
            }
        }
        Ok(StructType::new(id, self))
    }

    /// Look up an existing identified struct by name without creating
    /// one on miss.
    pub fn get_named_struct(&'ctx self, name: &str) -> Option<StructType<'ctx>> {
        self.ctx
            .get_named_struct(name)
            .map(|id| StructType::new(id, self))
    }

    /// Set the body of an identified struct. Errors if the struct is
    /// literal or if the body has already been set.
    pub fn set_struct_body<I, T>(
        &self,
        st: StructType<'ctx>,
        elements: I,
        packed: bool,
    ) -> IrResult<()>
    where
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx>>,
    {
        let elems: Box<[TypeId]> = elements.into_iter().map(|t| t.into().id()).collect();
        let body = StructBody {
            elements: elems,
            packed,
        };
        // Reject a struct that's actually literal — `set_struct_body` is
        // only meaningful for identified structs.
        let s = self
            .ctx
            .type_data(st.id)
            .as_struct()
            .expect("StructType invariant: wraps Struct");
        if s.name.is_none() {
            return Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Struct,
                got: TypeKindLabel::Struct,
            });
        }
        self.ctx.set_named_struct_body(st.id, body)
    }

    /// Typed-state `set_struct_body`: consumes a `StructType<Opaque>`
    /// and produces a `StructType<BodySet>` (Doctrine D1). Calling
    /// this twice is a compile error because there is no second
    /// `StructType<Opaque>` for the same id.
    pub fn set_struct_body_typed<I, T>(
        &self,
        opaque: StructType<'ctx, crate::struct_body_state::Opaque>,
        elements: I,
        packed: bool,
    ) -> IrResult<StructType<'ctx, crate::struct_body_state::BodySet>>
    where
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx>>,
    {
        let elems: Box<[TypeId]> = elements.into_iter().map(|t| t.into().id()).collect();
        let body = StructBody {
            elements: elems,
            packed,
        };
        self.ctx.set_named_struct_body(opaque.id, body)?;
        Ok(opaque.retag::<crate::struct_body_state::BodySet>())
    }

    // ---- Function ----

    /// Function signature `<ret>(params...)` (or `(...)` for varargs).
    pub fn fn_type<I, T>(
        &'ctx self,
        ret: impl Into<Type<'ctx>>,
        params: I,
        is_var_arg: bool,
    ) -> FunctionType<'ctx>
    where
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx>>,
    {
        let ret_id = ret.into().id();
        let params: Box<[TypeId]> = params.into_iter().map(|t| t.into().id()).collect();
        FunctionType::new(self.ctx.function_type(ret_id, params, is_var_arg), self)
    }

    // ---- Target extension ----

    /// `target("name", T1, T2, ..., I1, I2, ...)` opaque target type.
    pub fn target_ext_type<I, T, J>(
        &'ctx self,
        name: impl Into<String>,
        type_params: I,
        int_params: J,
    ) -> TargetExtType<'ctx>
    where
        I: IntoIterator<Item = T>,
        T: Into<Type<'ctx>>,
        J: IntoIterator<Item = u32>,
    {
        let name: String = name.into();
        let type_params: Box<[TypeId]> = type_params.into_iter().map(|t| t.into().id()).collect();
        let int_params: Box<[u32]> = int_params.into_iter().collect();
        TargetExtType::new(
            self.ctx.target_ext_type(name, type_params, int_params),
            self,
        )
    }

    // ---- Function creation ----

    /// Add a function to this module. Mirrors `Function::Create`.
    /// Returns `Err(IrError::DuplicateFunctionName)` if a function
    /// of the same name already exists, or
    /// [`IrError::ReturnTypeMismatch`] if the signature's return
    /// type does not match the chosen [`ReturnMarker`](crate::marker::ReturnMarker).
    pub fn add_function<R>(
        &'ctx self,
        name: impl AsRef<str>,
        signature: FunctionType<'ctx>,
        linkage: crate::global_value::Linkage,
    ) -> IrResult<crate::function::FunctionValue<'ctx, R>>
    where
        R: crate::marker::ReturnMarker,
    {
        let name = name.as_ref();
        if self.function_by_name.borrow().contains_key(name) {
            return Err(IrError::DuplicateFunctionName {
                name: name.to_owned(),
            });
        }
        // Reject the static-marker / signature mismatch up front.
        let ret_data = self.ctx.type_data(signature.return_type().id());
        if !crate::function::signature_matches_marker::<R>(ret_data) {
            return Err(IrError::ReturnTypeMismatch {
                expected: signature.return_type().kind_label(),
                got: signature.return_type().kind_label(),
            });
        }

        let signature_id = signature.id;

        // Push the function value first so each argument's
        // `parent_fn` can already point at the real id. Initial
        // `args` is empty; we patch it via `RefCell` once every
        // parameter is in the arena.
        let fn_data = crate::function::FunctionData::new(
            name.to_owned(),
            signature_id,
            linkage,
            crate::CallingConv::default(),
        );
        let fn_id = self.ctx.push_value(crate::value::ValueData {
            ty: signature_id,
            name: core::cell::RefCell::new(Some(name.to_owned())),
            debug_loc: None,
            kind: crate::value::ValueKindData::Function(fn_data),
            use_list: core::cell::RefCell::new(Vec::new()),
        });

        // Push each parameter as its own value-arena entry.
        let param_types: Vec<TypeId> = signature.params().map(|t| t.id()).collect();
        let mut arg_ids = Vec::with_capacity(param_types.len());
        for (slot, &ty) in param_types.iter().enumerate() {
            let slot_u32 = u32::try_from(slot)
                .unwrap_or_else(|_| unreachable!("function parameter slot exceeds u32::MAX"));
            let id = self.ctx.push_value(crate::value::ValueData {
                ty,
                name: core::cell::RefCell::new(None),
                debug_loc: None,
                kind: crate::value::ValueKindData::Argument {
                    parent_fn: fn_id,
                    slot: slot_u32,
                },
                use_list: core::cell::RefCell::new(Vec::new()),
            });
            arg_ids.push(id);
        }

        // Patch the function's args list.
        let fn_value_data = self.ctx.value_data(fn_id);
        let fn_inner = match &fn_value_data.kind {
            crate::value::ValueKindData::Function(f) => f,
            _ => unreachable!("just pushed Function variant"),
        };
        *fn_inner.args.borrow_mut() = arg_ids.into_boxed_slice();

        self.functions.borrow_mut().push(fn_id);
        self.function_by_name
            .borrow_mut()
            .insert(name.to_owned(), fn_id);
        Ok(crate::function::FunctionValue::<'ctx, R>::from_parts_unchecked(fn_id, self))
    }

    /// Look up a function by name, widened to the runtime-checked
    /// [`Dyn`](crate::marker::Dyn) form. Mirrors `Module::getFunction`. Use
    /// [`Self::function_by_name_typed`] when the caller knows the
    /// expected return shape and wants a typed handle.
    pub fn function_by_name(
        &'ctx self,
        name: &str,
    ) -> Option<crate::function::FunctionValue<'ctx, crate::marker::Dyn>> {
        self.function_by_name.borrow().get(name).copied().map(|id| {
            crate::function::FunctionValue::<'ctx, crate::marker::Dyn>::from_parts_unchecked(
                id, self,
            )
        })
    }

    /// Look up a function by name and narrow to a specific
    /// [`ReturnMarker`](crate::marker::ReturnMarker). Errors with
    /// [`IrError::ReturnTypeMismatch`] if the signature does not
    /// match `R`, or returns `Ok(None)` for an unknown name.
    pub fn function_by_name_typed<R>(
        &'ctx self,
        name: &str,
    ) -> IrResult<Option<crate::function::FunctionValue<'ctx, R>>>
    where
        R: crate::marker::ReturnMarker,
    {
        let Some(id) = self.function_by_name.borrow().get(name).copied() else {
            return Ok(None);
        };
        let value_data = self.ctx.value_data(id);
        let signature_id = match &value_data.kind {
            crate::value::ValueKindData::Function(f) => f.signature,
            _ => unreachable!("function_by_name table only stores function ids"),
        };
        let ret_id = self
            .ctx
            .type_data(signature_id)
            .as_function()
            .unwrap_or_else(|| unreachable!("function value carries a function signature"))
            .0;
        let ret_data = self.ctx.type_data(ret_id);
        if !crate::function::signature_matches_marker::<R>(ret_data) {
            let label = crate::r#type::Type::new(ret_id, self).kind_label();
            return Err(IrError::ReturnTypeMismatch {
                expected: label,
                got: label,
            });
        }
        Ok(Some(
            crate::function::FunctionValue::<'ctx, R>::from_parts_unchecked(id, self),
        ))
    }

    /// Iterate the module's functions in declaration order, widened
    /// to [`Dyn`](crate::marker::Dyn). Mirrors `Module::functions`.
    pub fn iter_functions(
        &'ctx self,
    ) -> impl ExactSizeIterator<Item = crate::function::FunctionValue<'ctx, crate::marker::Dyn>> + 'ctx
    {
        let ids: Vec<crate::value::ValueId> = self.functions.borrow().clone();
        ids.into_iter().map(move |id| {
            crate::function::FunctionValue::<'ctx, crate::marker::Dyn>::from_parts_unchecked(
                id, self,
            )
        })
    }

    /// Start a [`FunctionBuilder`](crate::function::FunctionBuilder)
    /// for incremental setup of linkage, calling convention,
    /// `unnamed_addr`, parameter names, and attributes before
    /// materialising the function.
    pub fn function_builder<R>(
        &'ctx self,
        name: impl Into<String>,
        signature: FunctionType<'ctx>,
    ) -> crate::function::FunctionBuilder<'ctx, R>
    where
        R: crate::marker::ReturnMarker,
    {
        crate::function::FunctionBuilder::new(self, name, signature)
    }

    // ---- Verification (Phase F) ----

    /// Verify the module's structural invariants without consuming
    /// it. Mirrors `verifyModule` (`Verifier.h`) for the diagnostic-
    /// only path.
    ///
    /// Returns the first invariant violation as an
    /// [`IrError::VerifierFailure`]. Use [`Self::verify`] when you
    /// want a branded [`VerifiedModule<'ctx>`] for the (future) pass
    /// manager.
    pub fn verify_borrowed(&'ctx self) -> IrResult<()> {
        crate::verifier::Verifier::new(self).run()
    }

    /// Verify the module and -- on success -- consume it into a
    /// brand-checked [`VerifiedModule<'ctx>`]. Future pass APIs
    /// require `&VerifiedModule` to guarantee the IR they operate
    /// on is well-formed.
    ///
    /// On failure, the underlying `Module` is destroyed; recover the
    /// IR by re-parsing or re-building.
    pub fn verify(self) -> IrResult<VerifiedModule<'ctx>> {
        // Borrow-checker dance: `verify_borrowed` requires `&'ctx self`,
        // which we satisfy by referencing the about-to-be-moved value
        // through a temporary borrow before the move.
        {
            crate::verifier::Verifier::new(&self).run()?;
        }
        Ok(VerifiedModule {
            inner: self,
            _brand: core::marker::PhantomData,
        })
    }

    // ---- Globals ----

    /// Add a `global` definition with an initializer. Mirrors the
    /// in-module `GlobalVariable::GlobalVariable(Module&, Type*,
    /// bool, LinkageTypes, Constant*, ...)` ctor.
    ///
    /// Returns `Err(IrError::DuplicateFunctionName)` if `name` is
    /// already bound at module scope (the same table covers
    /// functions and globals; mirrors LLVM's
    /// `Module::getValueSymbolTable`). Returns
    /// [`IrError::TypeMismatch`] when the initializer's type does
    /// not match `value_type`.
    pub fn add_global(
        &'ctx self,
        name: impl AsRef<str>,
        value_type: Type<'ctx>,
        initializer: impl crate::constant::IsConstant<'ctx>,
    ) -> IrResult<crate::global_variable::GlobalVariable<'ctx>> {
        crate::global_variable::GlobalBuilder::new(self, name.as_ref().to_owned(), value_type)
            .initializer(initializer)
            .build()
    }

    /// Add a `constant` (immutable) global with an initializer.
    /// Mirrors the same ctor with `isConstant=true`.
    pub fn add_global_constant(
        &'ctx self,
        name: impl AsRef<str>,
        value_type: Type<'ctx>,
        initializer: impl crate::constant::IsConstant<'ctx>,
    ) -> IrResult<crate::global_variable::GlobalVariable<'ctx>> {
        crate::global_variable::GlobalBuilder::new(self, name.as_ref().to_owned(), value_type)
            .constant(true)
            .initializer(initializer)
            .build()
    }

    /// Add an external global declaration (no initializer).
    /// Mirrors `Module::getOrInsertGlobal` in its declaration-form.
    pub fn add_external_global(
        &'ctx self,
        name: impl AsRef<str>,
        value_type: Type<'ctx>,
    ) -> IrResult<crate::global_variable::GlobalVariable<'ctx>> {
        crate::global_variable::GlobalBuilder::new(self, name.as_ref().to_owned(), value_type)
            .linkage(crate::global_value::Linkage::External)
            .build()
    }

    /// Begin a chainable [`GlobalBuilder`](crate::global_variable::GlobalBuilder)
    /// for full control over linkage, visibility, address space,
    /// alignment, comdat, etc.
    pub fn global_builder(
        &'ctx self,
        name: impl Into<String>,
        value_type: Type<'ctx>,
    ) -> crate::global_variable::GlobalBuilder<'ctx> {
        crate::global_variable::GlobalBuilder::new(self, name, value_type)
    }

    /// Look up a global by name. Mirrors `Module::getNamedGlobal`.
    pub fn get_global(
        &'ctx self,
        name: &str,
    ) -> Option<crate::global_variable::GlobalVariable<'ctx>> {
        let id = self.global_by_name.borrow().get(name).copied()?;
        let value_data = self.ctx.value_data(id);
        Some(crate::global_variable::GlobalVariable::from_parts_unchecked(id, self, value_data.ty))
    }

    /// Iterate the module's globals in declaration order. Mirrors
    /// `Module::globals`.
    pub fn iter_globals(
        &'ctx self,
    ) -> impl ExactSizeIterator<Item = crate::global_variable::GlobalVariable<'ctx>> + 'ctx {
        let ids: Vec<crate::value::ValueId> = self.globals.borrow().clone();
        ids.into_iter().map(move |id| {
            let value_data = self.ctx.value_data(id);
            crate::global_variable::GlobalVariable::from_parts_unchecked(id, self, value_data.ty)
        })
    }

    /// `true` if the module contains no globals. Mirrors
    /// `Module::global_empty`.
    #[inline]
    pub fn global_empty(&self) -> bool {
        self.globals.borrow().is_empty()
    }

    /// Crate-internal: install a built [`GlobalBuilder`] into the
    /// module. Performs the duplicate-name check and the comdat
    /// existence check, then pushes to the value arena.
    pub(crate) fn install_global_variable(
        &'ctx self,
        builder: crate::global_variable::GlobalBuilder<'ctx>,
    ) -> IrResult<crate::global_variable::GlobalVariable<'ctx>> {
        let (name, data, _initializer, address_space, value_type) = builder.into_data();
        if self.function_by_name.borrow().contains_key(&name)
            || self.global_by_name.borrow().contains_key(&name)
        {
            return Err(IrError::DuplicateFunctionName { name });
        }
        let pointer_ty = self.ctx.ptr_type(address_space);
        // Sanity: value_type must already be in the same context. Use
        // the cached id directly. (Construction APIs only hand out
        // typed ids belonging to this module.)
        let _ = value_type;
        let value_id = self.ctx.push_value(crate::value::ValueData {
            ty: pointer_ty,
            name: core::cell::RefCell::new(Some(name.clone())),
            debug_loc: None,
            kind: crate::value::ValueKindData::GlobalVariable(data),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.globals.borrow_mut().push(value_id);
        self.global_by_name.borrow_mut().insert(name, value_id);
        Ok(
            crate::global_variable::GlobalVariable::from_parts_unchecked(
                value_id, self, pointer_ty,
            ),
        )
    }

    // ---- Comdats ----

    /// Get or create a [`ComdatRef`](crate::comdat::ComdatRef) of
    /// the given name. Mirrors `Module::getOrInsertComdat`.
    ///
    /// On first lookup the selection kind defaults to
    /// [`SelectionKind::Any`](crate::comdat::SelectionKind::Any);
    /// callers can refine via
    /// [`ComdatRef::set_selection_kind`](crate::comdat::ComdatRef::set_selection_kind).
    pub fn get_or_insert_comdat(
        &'ctx self,
        name: impl AsRef<str>,
    ) -> crate::comdat::ComdatRef<'ctx> {
        let name = name.as_ref();
        if let Some(&id) = self.comdat_by_name.borrow().get(name) {
            return crate::comdat::ComdatRef {
                module: ModuleRef::new(self),
                id,
            };
        }
        let index = self.comdats.push(crate::comdat::ComdatData::new(
            name.to_owned(),
            crate::comdat::SelectionKind::Any,
        ));
        let id = crate::comdat::ComdatId::from_index(index);
        self.comdat_by_name.borrow_mut().insert(name.to_owned(), id);
        crate::comdat::ComdatRef {
            module: ModuleRef::new(self),
            id,
        }
    }

    /// Look up an existing comdat by name. Returns `None` when not
    /// present.
    pub fn get_comdat(&'ctx self, name: &str) -> Option<crate::comdat::ComdatRef<'ctx>> {
        let id = *self.comdat_by_name.borrow().get(name)?;
        Some(crate::comdat::ComdatRef {
            module: ModuleRef::new(self),
            id,
        })
    }

    /// Crate-internal: borrow the underlying [`ComdatData`] by id.
    /// Mirrors `Module::comdat_at`.
    pub(crate) fn comdat_at(&self, id: crate::comdat::ComdatId) -> &crate::comdat::ComdatData {
        self.comdats
            .get(id.arena_index())
            .unwrap_or_else(|| unreachable!("ComdatId is always valid for the owning module"))
    }

    /// Iterate comdat refs in insertion order. Mirrors
    /// `Module::getComdatSymbolTable` (insertion-order traversal).
    pub fn iter_comdats(
        &'ctx self,
    ) -> impl ExactSizeIterator<Item = crate::comdat::ComdatRef<'ctx>> + 'ctx {
        let count = self.comdats.count();
        (0..count).map(move |i| crate::comdat::ComdatRef {
            module: ModuleRef::new(self),
            id: crate::comdat::ComdatId::from_index(i),
        })
    }
}

// --------------------------------------------------------------------------
// VerifiedModule
// --------------------------------------------------------------------------

/// Brand-wrapper for a [`Module`] that has passed [`Module::verify`].
///
/// Constructed only by `Module::verify`, the wrapper signals to (future)
/// pass-manager APIs that the contained IR satisfies every verifier rule
/// at the moment of construction. Mutating the underlying module via
/// [`Self::unverify`] strips the brand; re-running [`Module::verify`] on
/// the resulting `Module` is required before passes that demand
/// well-formed IR can run again.
///
/// The wrapper carries a `'ctx` lifetime parameter so handles minted
/// against the contained module continue to compose with the rest of
/// the IR API.
pub struct VerifiedModule<'ctx> {
    pub(crate) inner: Module<'ctx>,
    /// Brand-only ZST; the only construction path is
    /// [`Module::verify`].
    pub(crate) _brand: core::marker::PhantomData<*const ()>,
}

impl<'ctx> VerifiedModule<'ctx> {
    /// Borrow the wrapped module for read-only inspection.
    #[inline]
    pub fn as_module(&'ctx self) -> &'ctx Module<'ctx> {
        &self.inner
    }

    /// Strip the brand and recover the inner [`Module`]. Any
    /// subsequent mutation invalidates the verification status; the
    /// caller can re-run [`Module::verify`] to recover a fresh
    /// [`VerifiedModule`].
    pub fn unverify(self) -> Module<'ctx> {
        self.inner
    }
}

// `&'ctx TypeData` borrows are *not* mutated; they point into a
// `boxcar::Vec` that only ever appends. The `RefCell`s inside `Context`
// guard hashmap mutation, never the arena, so iteration / accessor
// borrows of payload data are safe even while construction proceeds.
//
// `Module<'ctx>: !Sync` falls out of the `RefCell` fields. `Send` is
// blocked by `&'ctx` references in handles transitively, which is fine
// for a "one context per thread" model.

impl<'ctx> core::fmt::Display for Module<'ctx> {
    /// Print the module as textual `.ll`. Mirrors `Module::print` from
    /// `llvm/lib/IR/AsmWriter.cpp`.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        crate::asm_writer::fmt_module(f, self)
    }
}

impl<'ctx> core::fmt::Display for VerifiedModule<'ctx> {
    /// Forward to the wrapped [`Module`]'s [`fmt::Display`](core::fmt::Display) impl. Mirrors
    /// `Module::print`.
    #[inline]
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        <Module<'ctx> as core::fmt::Display>::fmt(&self.inner, f)
    }
}
