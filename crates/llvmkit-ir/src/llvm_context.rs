//! Per-Module interning state. Mirrors the type-storage layout of
//! `llvm/lib/IR/LLVMContextImpl.h` (`LLVMContextImpl`'s `IntegerTypes`
//! / `ArrayTypes` / `FunctionTypes` / `NamedStructTypes` / etc. fields):
//! one `HashMap` per type kind, keyed by the kind's structural fingerprint.
//!
//! Storage layout decisions:
//!
//! - One shared backing arena (`boxcar::Vec<TypeData>`), indexed by
//!   [`TypeSlot`]. Boxcar gives stable addresses under `&self`, so reads
//!   return plain `&TypeData` without `Ref<...>` wrapper noise.
//! - Per-kind intern maps (`int_types: HashMap<u32, TypeSlot>` etc.) instead
//!   of one big `HashMap<TypeKey, TypeSlot>` over a giant enum. Keys stay
//!   small and hash cheaply, and each constructor knows exactly which map
//!   to consult — the same way `LLVMContextImpl` operates.
//! - Singletons (`void`, `half`, ...) live in `Cell<Option<TypeSlot>>`
//!   slots, lazily filled on first request.
//!
//! `Context` is `pub(crate)` — the public surface is on
//! [`Module`](crate::Module). Promotion to a public `TypePool<'ctx>` is
//! a future-work item if cross-module type sharing ever becomes a need.

use core::cell::{Cell, RefCell};
use std::collections::HashMap;

use super::value::{GlobalFieldKind, ValueUse};
use crate::constant::{ConstantData, ConstantExprData};
use crate::r#type::{StructBody, TypeData, TypeSlot};
use crate::value::{ValueData, ValueKindData, ValueSlot};

pub(crate) struct Context {
    /// Backing arena. `&TypeData` borrows are stable for the lifetime of
    /// the owning module thanks to `boxcar::Vec`'s segmented storage.
    types: boxcar::Vec<TypeData>,

    // ---- Singleton primitives. Each lazily filled on first request.
    void: Cell<Option<TypeSlot>>,
    label: Cell<Option<TypeSlot>>,
    metadata: Cell<Option<TypeSlot>>,
    token: Cell<Option<TypeSlot>>,
    half: Cell<Option<TypeSlot>>,
    bfloat: Cell<Option<TypeSlot>>,
    float: Cell<Option<TypeSlot>>,
    double: Cell<Option<TypeSlot>>,
    fp128: Cell<Option<TypeSlot>>,
    x86_fp80: Cell<Option<TypeSlot>>,
    ppc_fp128: Cell<Option<TypeSlot>>,
    x86_amx: Cell<Option<TypeSlot>>,
    wasm_exnref: Cell<Option<TypeSlot>>,

    // ---- Parameterised — one map per kind. Keys are small structural
    // fingerprints (mirrors LLVMContextImpl).
    int_types: RefCell<HashMap<u32, TypeSlot>>,
    ptr_types: RefCell<HashMap<u32, TypeSlot>>,
    array_types: RefCell<HashMap<(TypeSlot, u64), TypeSlot>>,
    fixed_vector_types: RefCell<HashMap<(TypeSlot, u32), TypeSlot>>,
    scalable_vector_types: RefCell<HashMap<(TypeSlot, u32), TypeSlot>>,
    function_types: RefCell<HashMap<FunctionKey, TypeSlot>>,
    literal_struct_types: RefCell<HashMap<LiteralStructKey, TypeSlot>>,
    named_struct_types: RefCell<HashMap<String, TypeSlot>>,
    /// Insertion-ordered list of named-struct ids, parallel to
    /// `named_struct_types` (which is unordered). Lets the printer emit the
    /// `%Name = type {...}` identity block in declaration order.
    named_struct_order: RefCell<Vec<TypeSlot>>,
    typed_pointer_types: RefCell<HashMap<(TypeSlot, u32), TypeSlot>>,
    target_ext_types: RefCell<HashMap<TargetExtKey, TypeSlot>>,

    // ---- Value arena. Like the type arena, `boxcar::Vec` gives
    // stable `&ValueData` borrows under `&self`.
    values: boxcar::Vec<ValueData>,

    // ---- Per-kind constant interning. Mirrors
    // `LLVMContextImpl::IntConstants` / `FPConstants` / etc.
    int_constants: RefCell<IntConstantMap>,
    float_constants: RefCell<FloatConstantMap>,
    global_value_ref_constants: RefCell<HashMap<(TypeSlot, ValueSlot), ValueSlot>>,
    gep_offset_constants: RefCell<GepOffsetConstantMap>,
    symbol_delta_constants: RefCell<SymbolDeltaConstantMap>,
    symbol_delta_plus_constants: RefCell<SymbolDeltaPlusConstantMap>,
    null_constants: RefCell<HashMap<TypeSlot, ValueSlot>>,
    undef_constants: RefCell<HashMap<TypeSlot, ValueSlot>>,
    poison_constants: RefCell<HashMap<TypeSlot, ValueSlot>>,
    aggregate_constants: RefCell<AggregateConstantMap>,
    expr_constants: RefCell<HashMap<ConstantExprData, ValueSlot>>,
    block_address_constants: RefCell<HashMap<(TypeSlot, ValueSlot, ValueSlot), ValueSlot>>,
    dso_local_equivalent_constants: RefCell<HashMap<(TypeSlot, ValueSlot), ValueSlot>>,
    no_cfi_constants: RefCell<HashMap<(TypeSlot, ValueSlot), ValueSlot>>,
    token_none_constant: Cell<Option<ValueSlot>>,
    target_ext_none_constants: RefCell<HashMap<TypeSlot, ValueSlot>>,
    ptrauth_constants: RefCell<PtrauthConstantMap>,
}

/// Intern key for [`ConstantData::Int`](crate::constant::ConstantData::Int):
/// the integer's type plus its little-endian magnitude words.
type IntConstantMap = HashMap<(TypeSlot, Box<[u64]>), ValueSlot>;
/// Intern key for [`ConstantData::Float`](crate::constant::ConstantData::Float):
/// the float's type plus its IEEE bit pattern (held as a `u128` so
/// every IEEE width up to `fp128` fits without a discriminant).
type FloatConstantMap = HashMap<(TypeSlot, u128), ValueSlot>;
/// Intern key for [`ConstantData::GepOffset`](crate::constant::ConstantData::GepOffset):
/// the pointer type, the host global's value-id, and the byte offset.
type GepOffsetConstantMap = HashMap<(TypeSlot, ValueSlot, i64), ValueSlot>;
/// Intern key for [`ConstantData::SymbolDelta`](crate::constant::ConstantData::SymbolDelta):
/// the integer type and the two symbol value-ids, in order — the delta is not
/// commutative.
type SymbolDeltaConstantMap = HashMap<(TypeSlot, ValueSlot, ValueSlot), ValueSlot>;
/// Intern key for
/// [`ConstantData::SymbolDeltaPlus`](crate::constant::ConstantData::SymbolDeltaPlus):
/// the [`SymbolDeltaConstantMap`] key plus the baked-in addend.
type SymbolDeltaPlusConstantMap = HashMap<(TypeSlot, ValueSlot, ValueSlot, i64), ValueSlot>;
type PtrauthConstantMap = HashMap<
    (
        TypeSlot,
        ValueSlot,
        ValueSlot,
        ValueSlot,
        ValueSlot,
        ValueSlot,
    ),
    ValueSlot,
>;
/// Intern key for `ConstantArray` / `ConstantStruct` / `ConstantVector`
/// payloads: the aggregate's type plus its element value-ids.
type AggregateConstantMap = HashMap<(TypeSlot, Box<[ValueSlot]>), ValueSlot>;

/// Hashable structural key for a function type. Children are already
/// interned, so by-value [`TypeSlot`] equality is exactly LLVM's
/// pointer-equality-after-interning.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct FunctionKey {
    pub ret: TypeSlot,
    pub params: Box<[TypeSlot]>,
    pub is_var_arg: bool,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct LiteralStructKey {
    pub elements: Box<[TypeSlot]>,
    pub packed: bool,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct TargetExtKey {
    pub name: String,
    pub type_params: Box<[TypeSlot]>,
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
            wasm_exnref: Cell::new(None),
            int_types: RefCell::new(HashMap::new()),
            ptr_types: RefCell::new(HashMap::new()),
            array_types: RefCell::new(HashMap::new()),
            fixed_vector_types: RefCell::new(HashMap::new()),
            scalable_vector_types: RefCell::new(HashMap::new()),
            function_types: RefCell::new(HashMap::new()),
            literal_struct_types: RefCell::new(HashMap::new()),
            named_struct_types: RefCell::new(HashMap::new()),
            named_struct_order: RefCell::new(Vec::new()),
            typed_pointer_types: RefCell::new(HashMap::new()),
            target_ext_types: RefCell::new(HashMap::new()),
            values: boxcar::Vec::new(),
            int_constants: RefCell::new(HashMap::new()),
            float_constants: RefCell::new(HashMap::new()),
            global_value_ref_constants: RefCell::new(HashMap::new()),
            gep_offset_constants: RefCell::new(HashMap::new()),
            symbol_delta_constants: RefCell::new(HashMap::new()),
            symbol_delta_plus_constants: RefCell::new(HashMap::new()),
            null_constants: RefCell::new(HashMap::new()),
            undef_constants: RefCell::new(HashMap::new()),
            poison_constants: RefCell::new(HashMap::new()),
            aggregate_constants: RefCell::new(HashMap::new()),
            expr_constants: RefCell::new(HashMap::new()),
            block_address_constants: RefCell::new(HashMap::new()),
            dso_local_equivalent_constants: RefCell::new(HashMap::new()),
            no_cfi_constants: RefCell::new(HashMap::new()),
            token_none_constant: Cell::new(None),
            target_ext_none_constants: RefCell::new(HashMap::new()),
            ptrauth_constants: RefCell::new(HashMap::new()),
        }
    }

    /// Resolve a type id to its payload. Address is stable for the
    /// lifetime of the owning module.
    pub(crate) fn type_data(&self, id: TypeSlot) -> &TypeData {
        self.types
            .get(id.arena_index())
            .expect("invalid TypeSlot: out of arena range (cross-module mixing?)")
    }

    fn push(&self, data: TypeData) -> TypeSlot {
        let idx = self.types.push(data);
        // `idx + 1` keeps zero out of `NonZeroU32` so `Option<TypeSlot>` has
        // a niche and is still 4 bytes.
        TypeSlot::from_index(idx)
    }

    // ---- Singleton accessors ----

    pub(crate) fn void(&self) -> TypeSlot {
        self.singleton(&self.void, TypeData::Void)
    }
    pub(crate) fn label(&self) -> TypeSlot {
        self.singleton(&self.label, TypeData::Label)
    }
    pub(crate) fn metadata(&self) -> TypeSlot {
        self.singleton(&self.metadata, TypeData::Metadata)
    }
    pub(crate) fn token(&self) -> TypeSlot {
        self.singleton(&self.token, TypeData::Token)
    }
    pub(crate) fn half(&self) -> TypeSlot {
        self.singleton(&self.half, TypeData::Half)
    }
    pub(crate) fn bfloat(&self) -> TypeSlot {
        self.singleton(&self.bfloat, TypeData::Bfloat)
    }
    pub(crate) fn float(&self) -> TypeSlot {
        self.singleton(&self.float, TypeData::Float)
    }
    pub(crate) fn double(&self) -> TypeSlot {
        self.singleton(&self.double, TypeData::Double)
    }
    pub(crate) fn fp128(&self) -> TypeSlot {
        self.singleton(&self.fp128, TypeData::Fp128)
    }
    pub(crate) fn x86_fp80(&self) -> TypeSlot {
        self.singleton(&self.x86_fp80, TypeData::X86Fp80)
    }
    pub(crate) fn ppc_fp128(&self) -> TypeSlot {
        self.singleton(&self.ppc_fp128, TypeData::PpcFp128)
    }
    pub(crate) fn x86_amx(&self) -> TypeSlot {
        self.singleton(&self.x86_amx, TypeData::X86Amx)
    }
    pub(crate) fn wasm_exnref(&self) -> TypeSlot {
        self.singleton(&self.wasm_exnref, TypeData::WasmExnRef)
    }

    fn singleton(&self, slot: &Cell<Option<TypeSlot>>, data: TypeData) -> TypeSlot {
        if let Some(id) = slot.get() {
            return id;
        }
        let id = self.push(data);
        slot.set(Some(id));
        id
    }

    // ---- Parameterised constructors ----

    pub(crate) fn int_type(&self, bits: u32) -> TypeSlot {
        if let Some(&id) = self.int_types.borrow().get(&bits) {
            return id;
        }
        let id = self.push(TypeData::Integer { bits });
        self.int_types.borrow_mut().insert(bits, id);
        id
    }

    pub(crate) fn ptr_type(&self, addr_space: u32) -> TypeSlot {
        if let Some(&id) = self.ptr_types.borrow().get(&addr_space) {
            return id;
        }
        let id = self.push(TypeData::Pointer { addr_space });
        self.ptr_types.borrow_mut().insert(addr_space, id);
        id
    }

    pub(crate) fn array_type(&self, elem: TypeSlot, n: u64) -> TypeSlot {
        let key = (elem, n);
        if let Some(&id) = self.array_types.borrow().get(&key) {
            return id;
        }
        let id = self.push(TypeData::Array { elem, n });
        self.array_types.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn fixed_vector_type(&self, elem: TypeSlot, n: u32) -> TypeSlot {
        let key = (elem, n);
        if let Some(&id) = self.fixed_vector_types.borrow().get(&key) {
            return id;
        }
        let id = self.push(TypeData::FixedVector { elem, n });
        self.fixed_vector_types.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn scalable_vector_type(&self, elem: TypeSlot, min: u32) -> TypeSlot {
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
        ret: TypeSlot,
        params: Box<[TypeSlot]>,
        is_var_arg: bool,
    ) -> TypeSlot {
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

    pub(crate) fn literal_struct_type(&self, elements: Box<[TypeSlot]>, packed: bool) -> TypeSlot {
        let key = LiteralStructKey {
            elements: elements.clone(),
            packed,
        };
        if let Some(&id) = self.literal_struct_types.borrow().get(&key) {
            return id;
        }
        let id = self.push(TypeData::Struct(crate::r#type::StructTypeData {
            identity: crate::r#type::StructIdentity::Literal,
            body: RefCell::new(Some(StructBody { elements, packed })),
        }));
        self.literal_struct_types.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn typed_pointer_type(&self, pointee: TypeSlot, addr_space: u32) -> TypeSlot {
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
        type_params: Box<[TypeSlot]>,
        int_params: Box<[u32]>,
    ) -> TypeSlot {
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

    pub(crate) fn get_or_create_named_struct(&self, name: &str) -> (TypeSlot, bool) {
        if let Some(&id) = self.named_struct_types.borrow().get(name) {
            return (id, true);
        }
        let id = self.push(TypeData::Struct(crate::r#type::StructTypeData {
            identity: crate::r#type::StructIdentity::Identified {
                name: Some(name.to_owned()),
            },
            body: RefCell::new(None),
        }));
        self.named_struct_types
            .borrow_mut()
            .insert(name.to_owned(), id);
        self.named_struct_order.borrow_mut().push(id);
        (id, false)
    }

    /// An identified struct with no name — what `%0 = type { i32 }` is.
    /// Mirrors `StructType::create(Context)` with no name argument.
    ///
    /// Never interned: two anonymous identified structs with the same body are
    /// still two types, which is the whole point of the identified regime.
    pub(crate) fn create_anonymous_identified_struct(&self) -> TypeSlot {
        let id = self.push(TypeData::Struct(crate::r#type::StructTypeData {
            identity: crate::r#type::StructIdentity::Identified { name: None },
            body: RefCell::new(None),
        }));
        self.named_struct_order.borrow_mut().push(id);
        id
    }

    /// This anonymous identified struct's position among the module's
    /// anonymous identified structs — the number `AsmWriter` prints it under.
    /// Mirrors `TypePrinting::NumberedTypes`, which counts only the unnamed
    /// ones.
    pub(crate) fn anonymous_identified_struct_number(&self, id: TypeSlot) -> Option<u32> {
        let mut number: u32 = 0;
        for &candidate in self.named_struct_order.borrow().iter() {
            let TypeData::Struct(data) = self.type_data(candidate) else {
                continue;
            };
            if !matches!(
                data.identity,
                crate::r#type::StructIdentity::Identified { name: None }
            ) {
                continue;
            }
            if candidate == id {
                return Some(number);
            }
            number = number.saturating_add(1);
        }
        None
    }

    pub(crate) fn get_named_struct(&self, name: &str) -> Option<TypeSlot> {
        self.named_struct_types.borrow().get(name).copied()
    }

    /// Named-struct ids in insertion (declaration) order. Cloned out of the
    /// `RefCell` to avoid holding a borrow across the caller's work, matching
    /// the `ModuleCore::iter_functions`/`iter_globals` snapshot pattern
    /// (the crate-internal iterators, not the public `functions()`/
    /// `globals()` surface).
    pub(crate) fn iter_named_structs(&self) -> Vec<TypeSlot> {
        self.named_struct_order.borrow().clone()
    }

    pub(crate) fn set_named_struct_body(
        &self,
        id: TypeSlot,
        body: StructBody,
    ) -> crate::IrResult<()> {
        let s = self
            .type_data(id)
            .as_struct()
            .expect("set_named_struct_body invariant: id refers to a Struct");
        let mut slot = s.body.borrow_mut();
        if slot.is_some() {
            return Err(crate::IrError::StructBodyAlreadySet {
                name: s.identity.name().unwrap_or_default().to_owned(),
            });
        }
        if self.body_would_be_recursive(id, &body) {
            return Err(crate::IrError::RecursiveStructBody {
                name: s.identity.name().unwrap_or_default().to_owned(),
            });
        }
        *slot = Some(body);
        Ok(())
    }

    fn body_would_be_recursive(&self, target: TypeSlot, body: &StructBody) -> bool {
        body.elements
            .iter()
            .any(|&elem| self.type_reaches_type(elem, target, &mut Vec::new()))
    }

    fn type_reaches_type(
        &self,
        ty: TypeSlot,
        target: TypeSlot,
        visited: &mut Vec<TypeSlot>,
    ) -> bool {
        if ty == target {
            return true;
        }
        if visited.contains(&ty) {
            return false;
        }
        visited.push(ty);

        match self.type_data(ty) {
            TypeData::Function { ret, params, .. } => {
                self.type_reaches_type(*ret, target, visited)
                    || params
                        .iter()
                        .any(|&param| self.type_reaches_type(param, target, visited))
            }
            TypeData::Array { elem, .. }
            | TypeData::FixedVector { elem, .. }
            | TypeData::ScalableVector { elem, .. } => {
                self.type_reaches_type(*elem, target, visited)
            }
            TypeData::Struct(s) => {
                let body = s.body.borrow();
                body.as_ref().is_some_and(|body| {
                    body.elements
                        .iter()
                        .any(|&elem| self.type_reaches_type(elem, target, visited))
                })
            }
            TypeData::TypedPointer { pointee, .. } => {
                self.type_reaches_type(*pointee, target, visited)
            }
            TypeData::TargetExt(data) => data
                .type_params
                .iter()
                .any(|&param| self.type_reaches_type(param, target, visited)),
            TypeData::Void
            | TypeData::Half
            | TypeData::Bfloat
            | TypeData::Float
            | TypeData::Double
            | TypeData::X86Fp80
            | TypeData::Fp128
            | TypeData::PpcFp128
            | TypeData::X86Amx
            | TypeData::WasmExnRef
            | TypeData::Label
            | TypeData::Metadata
            | TypeData::Token
            | TypeData::Integer { .. }
            | TypeData::Pointer { .. } => false,
        }
    }

    // ---- Value arena ----

    /// Resolve a value-id to its payload. Address is stable for the
    /// lifetime of the owning module.
    pub(crate) fn value_data(&self, id: ValueSlot) -> &ValueData {
        match self.values.get(id.arena_index()) {
            Some(d) => d,
            None => unreachable!("invalid ValueSlot: out of arena range (cross-module mixing?)"),
        }
    }

    /// Render a basic-block value's printed name for a diagnostic: its
    /// textual name if present, else its numeric arena index. Used by the
    /// phi edge-add paths' [`IrError::AmbiguousPhiIncoming`](crate::IrError::AmbiguousPhiIncoming)
    /// message, which does not carry a borrowed block handle.
    pub(crate) fn block_diag_name(&self, id: ValueSlot) -> String {
        self.value_data(id)
            .name
            .borrow()
            .clone()
            .unwrap_or_else(|| id.arena_index().to_string())
    }

    /// Push a fresh value to the arena and return its id.
    pub(crate) fn push_value(&self, data: ValueData) -> ValueSlot {
        let idx = self.values.push(data);
        ValueSlot::from_index(idx)
    }

    fn register_constant_operand_uses(&self, user: ValueSlot) {
        let ValueKindData::Constant(data) = &self.value_data(user).kind else {
            return;
        };
        data.for_each_operand(|operand| {
            self.value_data(operand)
                .use_list
                .borrow_mut()
                .push(ValueUse::Constant(user));
        });
    }

    /// Move a global object's single-slot field edge from `old` to `new`.
    ///
    /// The counterpart of `Use::set` for the fields llvmkit stores as bare
    /// `Cell`s rather than as operands: without this, an initializer or
    /// aliasee would hold a value that neither `num_uses` counts nor RAUW can
    /// find. Callers set the cell; this keeps the reverse edge honest.
    pub(crate) fn retarget_global_field_use(
        &self,
        owner: ValueSlot,
        field: GlobalFieldKind,
        old: Option<ValueSlot>,
        new: Option<ValueSlot>,
    ) {
        if old == new {
            return;
        }
        let edge = ValueUse::GlobalField { owner, field };
        if let Some(old) = old {
            let data = self.value_data(old);
            let mut uses = data.use_list.borrow_mut();
            if let Some(position) = uses.iter().position(|use_| *use_ == edge) {
                uses.remove(position);
            }
        }
        if let Some(new) = new {
            self.value_data(new).use_list.borrow_mut().push(edge);
        }
    }

    /// Point a global object's single-slot field at `replacement` when it
    /// currently holds `from`. The [`ValueUse::GlobalField`] counterpart of
    /// `instruction::rewrite_operand_cells`: this cell is what `Use::set`
    /// would write to upstream.
    pub(crate) fn rewrite_global_field_cell(
        &self,
        owner: ValueSlot,
        field: GlobalFieldKind,
        from: ValueSlot,
        replacement: ValueSlot,
    ) {
        let swap = |cell: &Cell<Option<ValueSlot>>| {
            if cell.get() == Some(from) {
                cell.set(Some(replacement));
            }
        };
        let swap_required = |cell: &Cell<ValueSlot>| {
            if cell.get() == from {
                cell.set(replacement);
            }
        };
        match (&self.value_data(owner).kind, field) {
            (ValueKindData::GlobalVariable(g), GlobalFieldKind::Initializer) => {
                swap(&g.initializer);
            }
            (ValueKindData::GlobalAlias(a), GlobalFieldKind::Aliasee) => swap_required(&a.aliasee),
            (ValueKindData::GlobalIfunc(i), GlobalFieldKind::IfuncResolver) => {
                swap_required(&i.resolver);
            }
            (ValueKindData::Function(f), GlobalFieldKind::PersonalityFn) => {
                swap(&f.personality_fn);
            }
            (ValueKindData::Function(f), GlobalFieldKind::PrefixData) => swap(&f.prefix_data),
            (ValueKindData::Function(f), GlobalFieldKind::PrologueData) => swap(&f.prologue_data),
            // A `GlobalField` edge is only ever registered by
            // `retarget_global_field_use`, called from the one setter that
            // owns that exact cell, so a mismatched owner/field pair cannot
            // be constructed.
            (_, _) => unreachable!("GlobalField edge names a field its owner does not have"),
        }
    }

    /// Update the parent block of the instruction stored at `inst_id`.
    /// No-op if the value at that id is not an instruction. Crate-internal:
    /// only the lifecycle primitives in [`crate::instruction`] reach for this.
    pub(crate) fn set_instruction_parent(&self, inst_id: ValueSlot, new_parent: ValueSlot) {
        let data = self.value_data(inst_id);
        if let ValueKindData::Instruction(idata) = &data.kind {
            idata.parent.set(new_parent);
        }
    }

    // ---- Constant interning ----
    //
    // Each kind has its own intern map. Keys are the structural
    // fingerprint matching `LLVMContextImpl`'s constant uniquing.

    pub(crate) fn intern_constant_int(&self, ty: TypeSlot, words: Box<[u64]>) -> ValueSlot {
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

    pub(crate) fn intern_constant_float(&self, ty: TypeSlot, bits: u128) -> ValueSlot {
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

    pub(crate) fn intern_constant_null(&self, ty: TypeSlot) -> ValueSlot {
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

    pub(crate) fn intern_constant_undef(&self, ty: TypeSlot) -> ValueSlot {
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

    pub(crate) fn intern_constant_poison(&self, ty: TypeSlot) -> ValueSlot {
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

    pub(crate) fn intern_constant_global_value_ref(
        &self,
        ty: TypeSlot,
        value: ValueSlot,
    ) -> ValueSlot {
        let key = (ty, value);
        if let Some(&id) = self.global_value_ref_constants.borrow().get(&key) {
            return id;
        }
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::GlobalValueRef { value }),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.global_value_ref_constants.borrow_mut().insert(key, id);
        id
    }

    /// A forward reference awaiting its referent.
    ///
    /// **The one constant kind that is deliberately not uniqued.** Placeholders
    /// carry no payload, so every one of them is structurally identical — yet
    /// each stands for a *different* unresolved reference that the parser
    /// replaces individually as the named value is reached. Interning them
    /// would collapse every pending forward reference in a module into one
    /// node, and resolving the first would silently resolve them all. The name
    /// says `push`, not `intern`, for that reason.
    pub(crate) fn push_constant_forward_ref_placeholder(&self, ty: TypeSlot) -> ValueSlot {
        self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::ForwardRefPlaceholder),
            use_list: core::cell::RefCell::new(Vec::new()),
        })
    }

    /// Materialise a `getelementptr inbounds (i8, ptr @<base>, i64 <off>)`
    /// constant of pointer type `ty`.
    pub(crate) fn intern_constant_gep_offset(
        &self,
        ty: TypeSlot,
        base_id: ValueSlot,
        off: i64,
    ) -> ValueSlot {
        let key = (ty, base_id, off);
        if let Some(&id) = self.gep_offset_constants.borrow().get(&key) {
            return id;
        }
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::GepOffset { base_id, off }),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.gep_offset_constants.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn intern_constant_symbol_delta(
        &self,
        ty: TypeSlot,
        hi_id: ValueSlot,
        lo_id: ValueSlot,
    ) -> ValueSlot {
        let key = (ty, hi_id, lo_id);
        if let Some(&id) = self.symbol_delta_constants.borrow().get(&key) {
            return id;
        }
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::SymbolDelta { hi_id, lo_id }),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.symbol_delta_constants.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn intern_constant_symbol_delta_plus(
        &self,
        ty: TypeSlot,
        hi_id: ValueSlot,
        lo_id: ValueSlot,
        addend: i64,
    ) -> ValueSlot {
        let key = (ty, hi_id, lo_id, addend);
        if let Some(&id) = self.symbol_delta_plus_constants.borrow().get(&key) {
            return id;
        }
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::SymbolDeltaPlus {
                hi_id,
                lo_id,
                addend,
            }),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.symbol_delta_plus_constants
            .borrow_mut()
            .insert(key, id);
        id
    }

    pub(crate) fn intern_constant_aggregate(
        &self,
        ty: TypeSlot,
        elements: Box<[ValueSlot]>,
    ) -> ValueSlot {
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
        self.register_constant_operand_uses(id);
        self.aggregate_constants.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn intern_constant_expr(&self, data: ConstantExprData) -> ValueSlot {
        if let Some(&id) = self.expr_constants.borrow().get(&data) {
            return id;
        }
        let ty = data.result_ty;
        let key = data.clone();
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::Expr(data)),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.register_constant_operand_uses(id);
        self.expr_constants.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn intern_constant_block_address(
        &self,
        ty: TypeSlot,
        function: ValueSlot,
        block: ValueSlot,
    ) -> ValueSlot {
        let key = (ty, function, block);
        if let Some(&id) = self.block_address_constants.borrow().get(&key) {
            return id;
        }
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::BlockAddress { function, block }),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.block_address_constants.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn intern_constant_dso_local_equivalent(
        &self,
        ty: TypeSlot,
        function: ValueSlot,
    ) -> ValueSlot {
        let key = (ty, function);
        if let Some(&id) = self.dso_local_equivalent_constants.borrow().get(&key) {
            return id;
        }
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::DsoLocalEquivalent { function }),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.dso_local_equivalent_constants
            .borrow_mut()
            .insert(key, id);
        id
    }

    pub(crate) fn intern_constant_no_cfi(&self, ty: TypeSlot, function: ValueSlot) -> ValueSlot {
        let key = (ty, function);
        if let Some(&id) = self.no_cfi_constants.borrow().get(&key) {
            return id;
        }
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::NoCfi { function }),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.no_cfi_constants.borrow_mut().insert(key, id);
        id
    }

    pub(crate) fn intern_constant_token_none(&self, ty: TypeSlot) -> ValueSlot {
        if let Some(id) = self.token_none_constant.get() {
            return id;
        }
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::TokenNone),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.token_none_constant.set(Some(id));
        id
    }

    pub(crate) fn intern_constant_target_ext_none(&self, ty: TypeSlot) -> ValueSlot {
        if let Some(&id) = self.target_ext_none_constants.borrow().get(&ty) {
            return id;
        }
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::TargetExtNone),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.target_ext_none_constants.borrow_mut().insert(ty, id);
        id
    }

    pub(crate) fn intern_constant_ptrauth(
        &self,
        ty: TypeSlot,
        pointer: ValueSlot,
        key: ValueSlot,
        discriminator: ValueSlot,
        addr_discriminator: ValueSlot,
        deactivation_symbol: ValueSlot,
    ) -> ValueSlot {
        let map_key = (
            ty,
            pointer,
            key,
            discriminator,
            addr_discriminator,
            deactivation_symbol,
        );
        if let Some(&id) = self.ptrauth_constants.borrow().get(&map_key) {
            return id;
        }
        let id = self.push_value(ValueData {
            ty,
            name: core::cell::RefCell::new(None),
            debug_loc: None,
            kind: ValueKindData::Constant(ConstantData::PtrAuth {
                pointer,
                key,
                discriminator,
                addr_discriminator,
                deactivation_symbol,
            }),
            use_list: core::cell::RefCell::new(Vec::new()),
        });
        self.register_constant_operand_uses(id);
        self.ptrauth_constants.borrow_mut().insert(map_key, id);
        id
    }
}
