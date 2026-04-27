//! Per-Module interning state. Mirrors the type-storage layout of
//! `llvm/lib/IR/LLVMContextImpl.h` (`LLVMContextImpl`'s `IntegerTypes`
//! / `ArrayTypes` / `FunctionTypes` / `NamedStructTypes` / etc. fields):
//! one `HashMap` per type kind, keyed by the kind's structural fingerprint.
//!
//! Storage layout decisions:
//!
//! - One shared backing arena (`boxcar::Vec<TypeData>`), indexed by
//!   [`TypeId`]. Boxcar gives stable addresses under `&self`, so reads
//!   return plain `&TypeData` without `Ref<...>` wrapper noise.
//! - Per-kind intern maps (`int_types: HashMap<u32, TypeId>` etc.) instead
//!   of one big `HashMap<TypeKey, TypeId>` over a giant enum. Keys stay
//!   small and hash cheaply, and each constructor knows exactly which map
//!   to consult — the same way `LLVMContextImpl` operates.
//! - Singletons (`void`, `half`, ...) live in `Cell<Option<TypeId>>`
//!   slots, lazily filled on first request.
//!
//! `Context` is `pub(crate)` — the public surface is on
//! [`Module`](crate::Module). Promotion to a public `TypePool<'ctx>` is
//! a future-work item if cross-module type sharing ever becomes a need.

use core::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::constant::ConstantData;
use crate::r#type::{StructBody, TypeData, TypeId};
use crate::value::{ValueData, ValueId, ValueKindData};

pub(crate) struct Context {
    /// Backing arena. `&TypeData` borrows are stable for the lifetime of
    /// the owning module thanks to `boxcar::Vec`'s segmented storage.
    types: boxcar::Vec<TypeData>,

    // ---- Singleton primitives. Each lazily filled on first request.
    void: Cell<Option<TypeId>>,
    label: Cell<Option<TypeId>>,
    metadata: Cell<Option<TypeId>>,
    token: Cell<Option<TypeId>>,
    half: Cell<Option<TypeId>>,
    bfloat: Cell<Option<TypeId>>,
    float: Cell<Option<TypeId>>,
    double: Cell<Option<TypeId>>,
    fp128: Cell<Option<TypeId>>,
    x86_fp80: Cell<Option<TypeId>>,
    ppc_fp128: Cell<Option<TypeId>>,
    x86_amx: Cell<Option<TypeId>>,

    // ---- Parameterised — one map per kind. Keys are small structural
    // fingerprints (mirrors LLVMContextImpl).
    int_types: RefCell<HashMap<u32, TypeId>>,
    ptr_types: RefCell<HashMap<u32, TypeId>>,
    array_types: RefCell<HashMap<(TypeId, u64), TypeId>>,
    fixed_vector_types: RefCell<HashMap<(TypeId, u32), TypeId>>,
    scalable_vector_types: RefCell<HashMap<(TypeId, u32), TypeId>>,
    function_types: RefCell<HashMap<FunctionKey, TypeId>>,
    literal_struct_types: RefCell<HashMap<LiteralStructKey, TypeId>>,
    named_struct_types: RefCell<HashMap<String, TypeId>>,
    typed_pointer_types: RefCell<HashMap<(TypeId, u32), TypeId>>,
    target_ext_types: RefCell<HashMap<TargetExtKey, TypeId>>,

    // ---- Value arena. Like the type arena, `boxcar::Vec` gives
    // stable `&ValueData` borrows under `&self`.
    values: boxcar::Vec<ValueData>,

    // ---- Per-kind constant interning. Mirrors
    // `LLVMContextImpl::IntConstants` / `FPConstants` / etc.
    int_constants: RefCell<IntConstantMap>,
    float_constants: RefCell<FloatConstantMap>,
    null_constants: RefCell<HashMap<TypeId, ValueId>>,
    undef_constants: RefCell<HashMap<TypeId, ValueId>>,
    poison_constants: RefCell<HashMap<TypeId, ValueId>>,
    aggregate_constants: RefCell<AggregateConstantMap>,
}

/// Intern key for [`ConstantData::Int`](crate::constant::ConstantData::Int):
/// the integer's type plus its little-endian magnitude words.
type IntConstantMap = HashMap<(TypeId, Box<[u64]>), ValueId>;
/// Intern key for [`ConstantData::Float`](crate::constant::ConstantData::Float):
/// the float's type plus its IEEE bit pattern (held as a `u128` so
/// every IEEE width up to `fp128` fits without a discriminant).
type FloatConstantMap = HashMap<(TypeId, u128), ValueId>;
/// Intern key for `ConstantArray` / `ConstantStruct` / `ConstantVector`
/// payloads: the aggregate's type plus its element value-ids.
type AggregateConstantMap = HashMap<(TypeId, Box<[ValueId]>), ValueId>;

/// Hashable structural key for a function type. Children are already
/// interned, so by-value [`TypeId`] equality is exactly LLVM's
/// pointer-equality-after-interning.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct FunctionKey {
    pub ret: TypeId,
    pub params: Box<[TypeId]>,
    pub is_var_arg: bool,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct LiteralStructKey {
    pub elements: Box<[TypeId]>,
    pub packed: bool,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct TargetExtKey {
    pub name: String,
    pub type_params: Box<[TypeId]>,
    pub int_params: Box<[u32]>,
}

impl Context {
    pub(crate) fn new() -> Self {
        Self {
            types: boxcar::Vec::new(),
            void: Cell::new(None),
            label: Cell::new(None),
            metadata: Cell::new(None),
            token: Cell::new(None),
            half: Cell::new(None),
            bfloat: Cell::new(None),
            float: Cell::new(None),
            double: Cell::new(None),
            fp128: Cell::new(None),
            x86_fp80: Cell::new(None),
            ppc_fp128: Cell::new(None),
            x86_amx: Cell::new(None),
            int_types: RefCell::new(HashMap::new()),
            ptr_types: RefCell::new(HashMap::new()),
            array_types: RefCell::new(HashMap::new()),
            fixed_vector_types: RefCell::new(HashMap::new()),
            scalable_vector_types: RefCell::new(HashMap::new()),
            function_types: RefCell::new(HashMap::new()),
            literal_struct_types: RefCell::new(HashMap::new()),
            named_struct_types: RefCell::new(HashMap::new()),
            typed_pointer_types: RefCell::new(HashMap::new()),
            target_ext_types: RefCell::new(HashMap::new()),
            values: boxcar::Vec::new(),
            int_constants: RefCell::new(HashMap::new()),
            float_constants: RefCell::new(HashMap::new()),
            null_constants: RefCell::new(HashMap::new()),
            undef_constants: RefCell::new(HashMap::new()),
            poison_constants: RefCell::new(HashMap::new()),
            aggregate_constants: RefCell::new(HashMap::new()),
        }
    }

    /// Resolve a type id to its payload. Address is stable for the
    /// lifetime of the owning module.
    pub(crate) fn type_data(&self, id: TypeId) -> &TypeData {
        self.types
            .get(id.arena_index())
            .expect("invalid TypeId: out of arena range (cross-module mixing?)")
    }

    fn push(&self, data: TypeData) -> TypeId {
        let idx = self.types.push(data);
        // `idx + 1` keeps zero out of `NonZeroU32` so `Option<TypeId>` has
        // a niche and is still 4 bytes.
        TypeId::from_index(idx)
    }

    // ---- Singleton accessors ----

    pub(crate) fn void(&self) -> TypeId {
        self.singleton(&self.void, TypeData::Void)
    }
    pub(crate) fn label(&self) -> TypeId {
        self.singleton(&self.label, TypeData::Label)
    }
    pub(crate) fn metadata(&self) -> TypeId {
        self.singleton(&self.metadata, TypeData::Metadata)
    }
    pub(crate) fn token(&self) -> TypeId {
        self.singleton(&self.token, TypeData::Token)
    }
    pub(crate) fn half(&self) -> TypeId {
        self.singleton(&self.half, TypeData::Half)
    }
    pub(crate) fn bfloat(&self) -> TypeId {
        self.singleton(&self.bfloat, TypeData::BFloat)
    }
    pub(crate) fn float(&self) -> TypeId {
        self.singleton(&self.float, TypeData::Float)
    }
    pub(crate) fn double(&self) -> TypeId {
        self.singleton(&self.double, TypeData::Double)
    }
    pub(crate) fn fp128(&self) -> TypeId {
        self.singleton(&self.fp128, TypeData::Fp128)
    }
    pub(crate) fn x86_fp80(&self) -> TypeId {
        self.singleton(&self.x86_fp80, TypeData::X86Fp80)
    }
    pub(crate) fn ppc_fp128(&self) -> TypeId {
        self.singleton(&self.ppc_fp128, TypeData::PpcFp128)
    }
    pub(crate) fn x86_amx(&self) -> TypeId {
        self.singleton(&self.x86_amx, TypeData::X86Amx)
    }

    fn singleton(&self, slot: &Cell<Option<TypeId>>, data: TypeData) -> TypeId {
        if let Some(id) = slot.get() {
            return id;
        }
        let id = self.push(data);
        slot.set(Some(id));
        id
    }

    // ---- Parameterised constructors ----

    pub(crate) fn int_type(&self, bits: u32) -> TypeId {
        if let Some(&id) = self.int_types.borrow().get(&bits) {
            return id;
        }
        let id = self.push(TypeData::Integer { bits });
        self.int_types.borrow_mut().insert(bits, id);
        id
    }

    pub(crate) fn ptr_type(&self, addr_space: u32) -> TypeId {
        if let Some(&id) = self.ptr_types.borrow().get(&addr_space) {
            return id;
        }
        let id = self.push(TypeData::Pointer { addr_space });
        self.ptr_types.borrow_mut().insert(addr_space, id);
        id
    }

    pub(crate) fn array_type(&self, elem: TypeId, n: u64) -> TypeId {
        let key = (elem, n);
        if let Some(&id) = self.array_types.borrow().get(&key) {
            return id;
        }
        let id = self.push(TypeData::Array { elem, n });
        self.array_types.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn fixed_vector_type(&self, elem: TypeId, n: u32) -> TypeId {
        let key = (elem, n);
        if let Some(&id) = self.fixed_vector_types.borrow().get(&key) {
            return id;
        }
        let id = self.push(TypeData::FixedVector { elem, n });
        self.fixed_vector_types.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn scalable_vector_type(&self, elem: TypeId, min: u32) -> TypeId {
        let key = (elem, min);
        if let Some(&id) = self.scalable_vector_types.borrow().get(&key) {
            return id;
        }
        let id = self.push(TypeData::ScalableVector { elem, min });
        self.scalable_vector_types.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn function_type(
        &self,
        ret: TypeId,
        params: Box<[TypeId]>,
        is_var_arg: bool,
    ) -> TypeId {
        let key = FunctionKey {
            ret,
            params: params.clone(),
            is_var_arg,
        };
        if let Some(&id) = self.function_types.borrow().get(&key) {
            return id;
        }
        let id = self.push(TypeData::Function {
            ret,
            params,
            is_var_arg,
        });
        self.function_types.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn literal_struct_type(&self, elements: Box<[TypeId]>, packed: bool) -> TypeId {
        let key = LiteralStructKey {
            elements: elements.clone(),
            packed,
        };
        if let Some(&id) = self.literal_struct_types.borrow().get(&key) {
            return id;
        }
        let id = self.push(TypeData::Struct(crate::r#type::StructTypeData {
            name: None,
            body: RefCell::new(Some(StructBody { elements, packed })),
        }));
        self.literal_struct_types.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn typed_pointer_type(&self, pointee: TypeId, addr_space: u32) -> TypeId {
        let key = (pointee, addr_space);
        if let Some(&id) = self.typed_pointer_types.borrow().get(&key) {
            return id;
        }
        let id = self.push(TypeData::TypedPointer {
            pointee,
            addr_space,
        });
        self.typed_pointer_types.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn target_ext_type(
        &self,
        name: String,
        type_params: Box<[TypeId]>,
        int_params: Box<[u32]>,
    ) -> TypeId {
        let key = TargetExtKey {
            name: name.clone(),
            type_params: type_params.clone(),
            int_params: int_params.clone(),
        };
        if let Some(&id) = self.target_ext_types.borrow().get(&key) {
            return id;
        }
        let id = self.push(TypeData::TargetExt(crate::r#type::TargetExtTypeData {
            name,
            type_params,
            int_params,
        }));
        self.target_ext_types.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn get_or_create_named_struct(&self, name: &str) -> (TypeId, bool) {
        if let Some(&id) = self.named_struct_types.borrow().get(name) {
            return (id, true);
        }
        let id = self.push(TypeData::Struct(crate::r#type::StructTypeData {
            name: Some(name.to_owned()),
            body: RefCell::new(None),
        }));
        self.named_struct_types
            .borrow_mut()
            .insert(name.to_owned(), id);
        (id, false)
    }

    pub(crate) fn get_named_struct(&self, name: &str) -> Option<TypeId> {
        self.named_struct_types.borrow().get(name).copied()
    }

    pub(crate) fn set_named_struct_body(
        &self,
        id: TypeId,
        body: StructBody,
    ) -> crate::IrResult<()> {
        let s = self
            .type_data(id)
            .as_struct()
            .expect("set_named_struct_body invariant: id refers to a Struct");
        let mut slot = s.body.borrow_mut();
        if slot.is_some() {
            return Err(crate::IrError::StructBodyAlreadySet {
                name: s.name.clone().expect("named struct"),
            });
        }
        *slot = Some(body);
        Ok(())
    }

    // ---- Value arena ----

    /// Resolve a value-id to its payload. Address is stable for the
    /// lifetime of the owning module.
    pub(crate) fn value_data(&self, id: ValueId) -> &ValueData {
        match self.values.get(id.arena_index()) {
            Some(d) => d,
            None => unreachable!("invalid ValueId: out of arena range (cross-module mixing?)"),
        }
    }

    /// Push a fresh value to the arena and return its id.
    pub(crate) fn push_value(&self, data: ValueData) -> ValueId {
        let idx = self.values.push(data);
        ValueId::from_index(idx)
    }

    /// Update the parent block of the instruction stored at `inst_id`.
    /// No-op if the value at that id is not an instruction. Crate-internal:
    /// only the lifecycle primitives in [`crate::instruction`] reach for this.
    pub(crate) fn set_instruction_parent(&self, inst_id: ValueId, new_parent: ValueId) {
        let data = self.value_data(inst_id);
        if let crate::value::ValueKindData::Instruction(idata) = &data.kind {
            idata.parent.set(new_parent);
        }
    }

    // ---- Constant interning ----
    //
    // Each kind has its own intern map. Keys are the structural
    // fingerprint matching `LLVMContextImpl`'s constant uniquing.

    pub(crate) fn intern_constant_int(&self, ty: TypeId, words: Box<[u64]>) -> ValueId {
        let key = (ty, words.clone());
        if let Some(&id) = self.int_constants.borrow().get(&key) {
            return id;
        }
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::Int(words)),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.int_constants.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn intern_constant_float(&self, ty: TypeId, bits: u128) -> ValueId {
        let key = (ty, bits);
        if let Some(&id) = self.float_constants.borrow().get(&key) {
            return id;
        }
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::Float(bits)),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.float_constants.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn intern_constant_null(&self, ty: TypeId) -> ValueId {
        if let Some(&id) = self.null_constants.borrow().get(&ty) {
            return id;
        }
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::PointerNull),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.null_constants.borrow_mut().insert(ty, id);
        id
    }

    pub(crate) fn intern_constant_undef(&self, ty: TypeId) -> ValueId {
        if let Some(&id) = self.undef_constants.borrow().get(&ty) {
            return id;
        }
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::Undef),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.undef_constants.borrow_mut().insert(ty, id);
        id
    }

    pub(crate) fn intern_constant_poison(&self, ty: TypeId) -> ValueId {
        if let Some(&id) = self.poison_constants.borrow().get(&ty) {
            return id;
        }
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::Poison),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.poison_constants.borrow_mut().insert(ty, id);
        id
    }

    pub(crate) fn intern_constant_aggregate(
        &self,
        ty: TypeId,
        elements: Box<[ValueId]>,
    ) -> ValueId {
        let key = (ty, elements.clone());
        if let Some(&id) = self.aggregate_constants.borrow().get(&key) {
            return id;
        }
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::Aggregate(elements)),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.aggregate_constants.borrow_mut().insert(key, id);
        id
    }
}
