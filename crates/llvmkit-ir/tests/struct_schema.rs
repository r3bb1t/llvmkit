use llvmkit_ir::{
    Align, Capability, Dyn, Fp128, Half, IntValue, IntoErasedValue, IntoIrField, IntoLayoutValue,
    IrBuilder, IrError, Linkage, Module, ModuleBrand, ModuleRef, Mutable, Ptr, SchemaValue,
    StructFields, StructSchema, StructSchemaValue, StructValue, Value, Width, X86Fp80, module_new,
};
use llvmkit_macros::Branded;

/// A hand-written schema value, generic over its schema. It can hold only the
/// llvmkit-owned carrier, which llvmkit mints after checking a value against
/// `S` — so a `Hand<S>` is a value of `S`'s struct type by construction.
#[derive(Branded)]
struct Hand<'ctx, S: StructSchema, B: ModuleBrand, C: Capability = Mutable> {
    value: SchemaValue<'ctx, S, B, C>,
}

impl<'ctx, S: StructSchema, B: ModuleBrand + 'ctx, C: Capability> Hand<'ctx, S, B, C> {
    fn as_struct_value(self) -> StructValue<'ctx, B, C> {
        self.value.as_struct_value()
    }
}

impl<'ctx, S: StructSchema, B: ModuleBrand + 'ctx, C: Capability> StructSchemaValue<'ctx, S, B, C>
    for Hand<'ctx, S, B, C>
{
    fn from_schema_value(value: SchemaValue<'ctx, S, B, C>) -> Self {
        Self { value }
    }

    fn schema_value(self) -> SchemaValue<'ctx, S, B, C> {
        self.value
    }
}

impl<'ctx, S: StructSchema, B: ModuleBrand + 'ctx, C: Capability> IntoLayoutValue<'ctx, S, B>
    for Hand<'ctx, S, B, C>
{
    fn into_layout_value(self, module: ModuleRef<'ctx, B>) -> Result<Value<'ctx, B>, IrError> {
        self.value.as_struct_value().into_erased_value(module)
    }
}

/// An aggregate lift that hands back whatever value it holds, whatever its
/// type: `IntoLayoutValue` is open — the derive implements it in the user's
/// crate — so nothing stops one being written.
struct Forged<'ctx, B: ModuleBrand>(Value<'ctx, B>);

impl<'ctx, B: ModuleBrand + 'ctx> IntoLayoutValue<'ctx, Point, B> for Forged<'ctx, B> {
    fn into_layout_value(self, module: ModuleRef<'ctx, B>) -> Result<Value<'ctx, B>, IrError> {
        self.0.into_erased_value(module)
    }
}

struct Point;

impl StructSchema for Point {
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = Hand<'ctx, Self, B, C>;
    type Fields = (i32, i32);

    const NAME: &'static str = "Point";
}

impl<'ctx, B: ModuleBrand + 'ctx> Hand<'ctx, Point, B> {
    fn x<'m, F, R>(
        self,
        b: &llvmkit_ir::IrBuilder<'m, 'ctx, B, F, llvmkit_ir::Positioned, R>,
    ) -> Result<IntValue<'ctx, i32, B>, IrError>
    where
        F: llvmkit_ir::IrBuilderFolder<'ctx, B>,
        R: llvmkit_ir::ReturnMarker,
    {
        b.extract_field::<Point, i32, _, _>(self, 0, "x")
    }

    fn with_x<'m, F, R, V>(
        self,
        b: &llvmkit_ir::IrBuilder<'m, 'ctx, B, F, llvmkit_ir::Positioned, R>,
        value: V,
    ) -> Result<Self, IrError>
    where
        F: llvmkit_ir::IrBuilderFolder<'ctx, B>,
        R: llvmkit_ir::ReturnMarker,
        V: IntoIrField<'ctx, i32, B>,
    {
        b.insert_field::<Point, i32, _, _, _>(self, value, 0, "with_x")
    }
}

/// Named `"Point"` like [`Point`], with another body.
struct BadPoint;

impl StructSchema for BadPoint {
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = Hand<'ctx, Self, B, C>;
    type Fields = (i64,);

    const NAME: &'static str = "Point";
}

/// Contains itself by value: no Rust struct can, but a hand-written schema
/// can name itself as a field.
struct RecursiveNode;

impl StructSchema for RecursiveNode {
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = Hand<'ctx, Self, B, C>;
    type Fields = (RecursiveNode,);

    const NAME: &'static str = "RecursiveNode";
}

/// `CycleA { b: CycleB }` and `CycleB { a: CycleA }`: each contains the other
/// by value.
struct CycleA;
struct CycleB;

impl StructSchema for CycleA {
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = Hand<'ctx, Self, B, C>;
    type Fields = (CycleB,);

    const NAME: &'static str = "CycleA";
}

impl StructSchema for CycleB {
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = Hand<'ctx, Self, B, C>;
    type Fields = (CycleA,);

    const NAME: &'static str = "CycleB";
}

struct Rect;

impl StructSchema for Rect {
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = Hand<'ctx, Self, B, C>;
    type Fields = (Point, Point);

    const NAME: &'static str = "Rect";
}

/// One hand-written schema: its Rust type, LLVM name and field tuple.
macro_rules! schema {
    ($($ty:ident = $name:literal $fields:ty;)+) => {$(
        #[derive(Clone, Copy, Debug)]
        struct $ty;

        impl StructSchema for $ty {
            type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = Hand<'ctx, Self, B, C>;
            type Fields = $fields;

            const NAME: &'static str = $name;
        }
    )+};
}

schema! {
    // A cycle entered from above: `YLow` names both frames in progress.
    YTop = "YTop" (YMid,);
    YMid = "YMid" (YLow,);
    YLow = "YLow" (YTop, YMid);
    // A field repeated inside a cycle: the second `RepLeaf` meets a body that
    // holds the still-opaque `%RepTop`.
    RepTop = "RepTop" (RepLeaf, RepLeaf);
    RepLeaf = "RepLeaf" (RepTop,);
    // Two schemas of one name.
    TwinOne = "Twin" (i32,);
    TwinSame = "Twin" (i32,);
    TwinWide = "Twin" (i64,);
    Twins = "Twins" (TwinOne, TwinSame);
    TwinClash = "TwinClash" (TwinOne, TwinWide);
    DeepOne = "DeepNest" (TwinOne,);
    DeepTwo = "DeepNest" (TwinWide,);
    DeepClash = "DeepClash" (DeepOne, DeepTwo);
    // The one schema twice, sharing a body.
    Diamond = "Diamond" (Point, Point);
    // `%Rect` over an opaque `%Point`, with `Point` minted before and after it.
    PointThenRect = "PointThenRect" (Point, Rect);
    RectThenPoint = "RectThenPoint" (Rect, Point);
    // A refusal after a sibling that would have been fresh.
    Fresh = "Fresh" (i32,);
    Existing = "Existing" (i32,);
    FreshThenExisting = "FreshThenExisting" (Fresh, Existing);
    FreshThenCycle = "FreshThenCycle" (Fresh, CycleA);
    // A check that passes one field and fails the next.
    HoldInner = "HoldInner" (i32,);
    HoldOuter = "HoldOuter" (HoldInner, i64);
    // Every layout kind, a name `AsmWriter` must quote among them.
    EveryLayout = "EveryLayout" (
        bool,
        u8,
        i128,
        Width<7>,
        f32,
        f64,
        Half,
        X86Fp80,
        Fp128,
        Ptr,
        Ptr<3>,
        [i16; 3],
        [f64; 2],
        Point,
        Quoted,
    );
}

struct PackedPair;

impl StructSchema for PackedPair {
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = Hand<'ctx, Self, B, C>;
    type Fields = (i8, i32);

    const NAME: &'static str = "PackedPair";
    const PACKED: bool = true;
}

/// What minting `S` into a fresh module answers, as a verdict that compares.
fn mint_answer<S: StructSchema>() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    m.get_or_insert_struct_of::<S>().map(|_| ())
}

/// `StructBodyMismatch` naming `name`.
fn mismatch(name: &str) -> Result<(), IrError> {
    Err(IrError::StructBodyMismatch {
        name: name.to_owned(),
    })
}

/// `RecursiveStructBody` naming `name`.
fn recursive(name: &str) -> Result<(), IrError> {
    Err(IrError::RecursiveStructBody {
        name: name.to_owned(),
    })
}

impl<'ctx, B: ModuleBrand + 'ctx> Hand<'ctx, Rect, B> {
    fn min<'m, F, R>(
        self,
        b: &llvmkit_ir::IrBuilder<'m, 'ctx, B, F, llvmkit_ir::Positioned, R>,
    ) -> Result<Hand<'ctx, Point, B>, IrError>
    where
        F: llvmkit_ir::IrBuilderFolder<'ctx, B>,
        R: llvmkit_ir::ReturnMarker,
    {
        b.extract_field::<Rect, Point, _, _>(self, 0, "min")
    }

    fn max<'m, F, R>(
        self,
        b: &llvmkit_ir::IrBuilder<'m, 'ctx, B, F, llvmkit_ir::Positioned, R>,
    ) -> Result<Hand<'ctx, Point, B>, IrError>
    where
        F: llvmkit_ir::IrBuilderFolder<'ctx, B>,
        R: llvmkit_ir::ReturnMarker,
    {
        b.extract_field::<Rect, Point, _, _>(self, 1, "max")
    }
}

/// llvmkit-specific schema facade over LLVM named structs; closest upstream
/// coverage is `unittests/IR/TypeBuilderTest.cpp::TEST(TypeBuilder, NamedStruct)`
/// for idempotent identified-struct lookup.
#[test]
fn struct_schema_reuses_matching_named_body() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let first = m.get_or_insert_struct_of::<Point>()?;
    let second = m.get_or_insert_struct_of::<Point>()?;
    assert_eq!(first.as_type(), second.as_type());
    let text = format!("{m}");
    assert_eq!(text.matches("%Point = type { i32, i32 }").count(), 1);
    Ok(())
}

/// llvmkit-specific schema facade over LLVM named structs; closest upstream
/// coverage is `lib/IR/LLVMContextImpl.cpp::getOrCreateNamedStruct` reuse rules.
#[test]
fn struct_schema_rejects_mismatched_existing_named_body() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let _ = m.get_or_insert_struct_of::<Point>()?;
    assert_eq!(
        m.get_or_insert_struct_of::<BadPoint>(),
        Err(IrError::StructBodyMismatch {
            name: String::from("Point"),
        })
    );
    Ok(())
}

/// Mirrors `StructType::setBodyOrError` / `StructType::checkBody`
/// (`lib/IR/Type.cpp`): identified struct bodies may not recursively contain
/// the struct being defined. The schema names itself as its only field; the
/// minter hands the nested occurrence the still-opaque `%RecursiveNode`
/// rather than recursing, and the body setter refuses the cycle.
#[test]
fn struct_schema_rejects_recursive_named_body() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    assert_eq!(
        m.get_or_insert_struct_of::<RecursiveNode>(),
        Err(IrError::RecursiveStructBody {
            name: String::from("RecursiveNode"),
        })
    );
    Ok(())
}

/// Review Focus 3 of the type-constructor validity plan; mirrors
/// `StructType::setBodyOrError` / `StructType::checkBody` (`lib/IR/Type.cpp`),
/// whose walk refuses a body that reaches the struct being defined through
/// any depth of by-value elements. Two hand-written schemas that contain each
/// other: minting `CycleA` mints `CycleB`, whose `CycleA` field is the
/// still-opaque `%CycleA`; `%CycleB`'s body is set, and `%CycleA`'s is refused
/// — with no unbounded recursion on the way.
#[test]
fn mutually_recursive_schemas_are_refused() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    assert_eq!(
        m.get_or_insert_struct_of::<CycleA>(),
        Err(IrError::RecursiveStructBody {
            name: String::from("CycleA"),
        })
    );
    Ok(())
}

/// llvmkit-specific, no upstream counterpart: a refused cycle is refused before
/// anything is created, so neither `%CycleA` nor `%CycleB` exists afterwards and
/// a second mint answers exactly what the first did. The mint used to set
/// `%CycleB`'s body on the way to the refusal, and the retry then met that
/// leftover and said `StructBodyMismatch`.
#[test]
fn a_second_mint_of_a_refused_cycle_is_refused() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let before = format!("{m}");
    for _ in 0..2 {
        assert_eq!(
            m.get_or_insert_struct_of::<CycleA>().map(|_| ()),
            recursive("CycleA")
        );
        assert!(m.named_struct("CycleA").is_none());
        assert!(m.named_struct("CycleB").is_none());
        assert_eq!(format!("{m}"), before);
    }
    Ok(())
}

/// llvmkit-specific regression (D8): a refusal at any depth leaves the module as
/// it was. `%Fresh` is minted before `%Existing` is refused, so it used to stay
/// behind, in a module that prints every identified struct.
#[test]
fn a_refused_mint_leaves_nothing_at_any_depth() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let existing = m.get_or_insert_named_struct("Existing");
    m.set_struct_body_dyn(existing, [m.i64_type()], false)?;
    let before = format!("{m}");
    assert_eq!(
        m.get_or_insert_struct_of::<FreshThenExisting>().map(|_| ()),
        mismatch("Existing")
    );
    assert_eq!(
        m.get_or_insert_struct_of::<FreshThenCycle>().map(|_| ()),
        recursive("CycleA")
    );
    assert!(m.named_struct("Fresh").is_none());
    assert_eq!(format!("{m}"), before);
    Ok(())
}

/// llvmkit-specific regression (D8): the builder's routes into a mint, which
/// enter below `get_or_insert_struct_of`, refuse as cleanly. `typed_alloca` is
/// `IrBuilder::schema_ir_type`'s route and `field_gep` `schema_struct_type`'s.
#[test]
fn a_refused_mint_through_the_builder_leaves_nothing() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let existing = m.get_or_insert_named_struct("Existing");
    m.set_struct_body_dyn(existing, [m.i64_type()], false)?;
    let f = m.add_typed_function::<(), (), _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<()>(&m).position_at_end(entry);
    let slot = b.typed_alloca::<i32, _>("slot")?;
    let before = format!("{m}");

    assert_eq!(
        b.typed_alloca::<FreshThenExisting, _>("x").map(|_| ()),
        mismatch("Existing")
    );
    assert_eq!(
        b.typed_alloca::<CycleA, _>("y").map(|_| ()),
        recursive("CycleA")
    );
    assert_eq!(
        b.field_gep::<FreshThenExisting, _, 0, _>(slot.as_pointer_value().with_pointee(), "z")
            .map(|_| ()),
        mismatch("Existing")
    );
    assert!(m.named_struct("Fresh").is_none());
    assert_eq!(format!("{m}"), before);
    b.ret_void();
    Ok(())
}

/// llvmkit-specific regression (D8): a signature is one mint. `Ret` and each
/// parameter used to mint in turn, so a refused later one left the earlier ones.
/// Every route that builds a signature from a schema is here: a typed
/// function, a variadic one, a block's parameters and an indirect call.
#[test]
fn a_refused_signature_mints_no_part_of_it() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let existing = m.get_or_insert_named_struct("Existing");
    m.set_struct_body_dyn(existing, [m.i64_type()], false)?;
    let f = m.add_typed_function::<(), (), _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<()>(&m).position_at_end(entry);
    let slot = m.view(b.alloca_in::<0, _, _>(m.i32_type(), "slot")?);
    let before = format!("{m}");

    let refused = mismatch("Existing");
    assert_eq!(
        m.add_typed_function::<(), (Fresh, Existing), _>("g", Linkage::External)
            .map(|_| ()),
        refused
    );
    assert_eq!(
        m.add_typed_function::<Fresh, (Existing,), _>("g", Linkage::External)
            .map(|_| ()),
        refused
    );
    assert_eq!(
        m.add_typed_varargs_function::<(), (Fresh, Existing), _>("g", Linkage::External)
            .map(|_| ()),
        refused
    );
    assert_eq!(
        m.typed_function_type::<Fresh, (Existing,)>().map(|_| ()),
        refused
    );
    assert_eq!(
        b.append_block_typed::<(Fresh, Existing), _, _>(m.view(f).as_function(), "h")
            .map(|_| ()),
        refused
    );
    let argument = m.i32_type().const_int(1_i32).as_erased();
    assert_eq!(
        b.indirect_call::<fn(Existing) -> Fresh, _, _, _>(slot, (argument,), "c")
            .map(|_| ()),
        refused
    );
    assert!(m.named_struct("Fresh").is_none());
    assert_eq!(format!("{m}"), before);
    b.ret_void();
    Ok(())
}

/// llvmkit-specific regression (D8): `declare_function` read the name after the
/// signature was minted, so a refused name left `%Fresh` behind. Each of its
/// refusals is here, on each of the four typed routes: a name another global
/// holds, a known intrinsic's name, and a `llvm.` name that is neither.
#[test]
fn a_refused_name_mints_no_part_of_a_signature() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    m.add_typed_function::<(), (), _>("taken", Linkage::External)?;
    let before = format!("{m}");

    for (name, refused) in [
        (
            "taken",
            IrError::DuplicateFunctionName {
                name: "taken".to_owned(),
            },
        ),
        (
            "llvm.abs.i32",
            IrError::ReservedIntrinsicName {
                name: "llvm.abs.i32".to_owned(),
            },
        ),
        (
            "llvm.not.an.intrinsic",
            IrError::UnknownIntrinsic {
                name: "llvm.not.an.intrinsic".to_owned(),
            },
        ),
    ] {
        let refused = Err(refused);
        assert_eq!(
            m.add_typed_function::<Fresh, (), _>(name, Linkage::External)
                .map(|_| ()),
            refused
        );
        assert_eq!(
            m.add_typed_function_of::<fn() -> Fresh, _>(name, Linkage::External)
                .map(|_| ()),
            refused
        );
        assert_eq!(
            m.add_typed_varargs_function::<Fresh, (), _>(name, Linkage::External)
                .map(|_| ()),
            refused
        );
        assert_eq!(
            m.add_typed_varargs_function_of::<fn() -> Fresh, _>(name, Linkage::External)
                .map(|_| ()),
            refused
        );
    }
    assert!(m.named_struct("Fresh").is_none());
    assert_eq!(format!("{m}"), before);
    Ok(())
}

/// llvmkit-specific regression (D8): what an entry can still refuse after it
/// mints a schema's type. Each mint here was followed by the lift of an operand
/// another module owns, which `Module::dynamic` lets through the brand, so the
/// refusal left `%Fresh` behind. The lift comes first now. `%Point` is the
/// positive control: the same call with this module's operand mints it.
#[test]
fn a_refused_operand_mints_no_part_of_a_schema() -> Result<(), IrError> {
    let other = Module::dynamic("other");
    let g = other.add_typed_function::<(), (), _>("g", Linkage::External)?;
    let other_entry = other.view(g).append_basic_block(&other, "entry");
    let other_builder = IrBuilder::new_for::<()>(&other).position_at_end(other_entry);
    let foreign = other_builder.typed_alloca::<i32, _>("p")?;
    let foreign_int = other.i32_type().const_int(1_i32);

    let m = Module::dynamic("schema");
    let f = m.add_typed_function::<(), (), _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<()>(&m).position_at_end(entry);
    let slot = b.typed_alloca::<i32, _>("slot")?;
    let before = format!("{m}");

    let foreign_pointer = || foreign.as_pointer_value().with_pointee::<Fresh>();
    let local_pointer = || slot.as_pointer_value().with_pointee::<Fresh>();
    let refused = |result: Result<(), IrError>| {
        assert_eq!(result, Err(IrError::ForeignValueId));
    };
    refused(b.typed_load(foreign_pointer(), "a").map(|_| ()));
    refused(
        b.typed_load_with_align(foreign_pointer(), Align::ONE, "b")
            .map(|_| ()),
    );
    refused(
        b.load_from(foreign_pointer())
            .typed::<Fresh>("c")
            .map(|_| ()),
    );
    refused(
        b.field_gep::<Fresh, _, 0, _>(foreign_pointer(), "d")
            .map(|_| ()),
    );
    refused(
        b.element_gep::<Fresh, _, i32, _, _>(foreign_pointer(), 1_i32, "e")
            .map(|_| ()),
    );
    refused(
        b.element_gep::<Fresh, _, i32, _, _>(local_pointer(), foreign_int, "f")
            .map(|_| ()),
    );
    refused(
        b.inbounds_element_gep::<Fresh, _, i32, _, _>(foreign_pointer(), 1_i32, "g")
            .map(|_| ()),
    );
    refused(
        b.inbounds_element_gep::<Fresh, _, i32, _, _>(local_pointer(), foreign_int, "h")
            .map(|_| ()),
    );
    refused(
        b.indirect_call::<fn(i32) -> Fresh, _, _, _>(slot, (foreign_int,), "i")
            .map(|_| ()),
    );
    assert!(m.named_struct("Fresh").is_none());
    assert_eq!(format!("{m}"), before);

    b.typed_load(slot.as_pointer_value().with_pointee::<Point>(), "control")?;
    assert!(m.named_struct("Point").is_some());
    b.ret_void();
    Ok(())
}

/// llvmkit-specific, no upstream counterpart: `matches_type` is a predicate. A
/// check that fails leaves no mark, at any depth: `%HoldInner` passed the first
/// field of a check whose second field failed, so nothing rests on it and
/// `Module::set_struct_name` still renames it. The mark used to be set inside
/// the match, on every struct a failed check passed through.
#[test]
fn a_refused_check_holds_no_struct() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let i32_ty = m.i32_type();
    let inner = m.get_or_insert_named_struct("HoldInner");
    m.set_struct_body_dyn(inner, [i32_ty], false)?;
    let outer = m.get_or_insert_named_struct("HoldOuter");
    m.set_struct_body_dyn(outer, [inner.as_type(), i32_ty.as_type()], false)?;
    let fn_ty = m.function_type(m.void_type(), [outer.as_type()])?;
    let f = m.add_function_dyn("take", fn_ty, Linkage::External)?;

    assert!(SchemaValue::<HoldOuter, _>::try_new(m.view(f).param(0)?).is_err());
    m.set_struct_name(inner, "RenamedInner")?;
    assert_eq!(inner.name(), Some("RenamedInner"));
    Ok(())
}

/// llvmkit-specific, no upstream counterpart: `extract_field` checks a field
/// against its layout and answers with a value of that schema, so that check
/// holds the field's struct. The aggregate it read is not held: only a raw
/// operand passed through it, and the value it answers with says nothing of it.
#[test]
fn a_checked_field_read_holds_the_field_alone() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let inner = m.get_or_insert_named_struct("HoldInner");
    m.set_struct_body_dyn(inner, [m.i32_type()], false)?;
    let outer = m.get_or_insert_named_struct("HoldOuter");
    m.set_struct_body_dyn(outer, [inner.as_type(), m.i64_type().as_type()], false)?;
    let fn_ty = m.function_type(m.void_type(), [outer.as_type()])?;
    let f = m.add_function_dyn("read", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);

    let aggregate = m.view(f).param(0)?;
    let _ = b.extract_field::<HoldOuter, HoldInner, _, _>(aggregate, 0, "field")?;
    assert_eq!(
        m.set_struct_name(inner, "RenamedInner"),
        Err(IrError::SchemaStructNameNotSettable {
            name: "HoldInner".into(),
        })
    );
    m.set_struct_name(outer, "RenamedOuter")?;
    Ok(())
}

/// llvmkit-specific, no upstream counterpart: a typed signature is one check,
/// which ends after the last parameter. A signature whose return matches and
/// whose parameter does not holds neither.
#[test]
fn a_refused_signature_holds_no_struct() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let i32_ty = m.i32_type();
    let inner = m.get_or_insert_named_struct("HoldInner");
    m.set_struct_body_dyn(inner, [i32_ty], false)?;
    let fn_ty = m.function_type(inner.as_type(), [inner.as_type(), i32_ty.as_type()])?;
    let f = m.add_function_dyn("mixed", fn_ty, Linkage::External)?;

    assert!(
        m.view(f)
            .with_typed_signature::<fn(HoldInner, i64) -> HoldInner>()
            .is_err()
    );
    m.set_struct_name(inner, "RenamedInner")?;
    assert_eq!(inner.name(), Some("RenamedInner"));
    Ok(())
}

/// llvmkit-specific, no upstream counterpart: the mirror of
/// `a_refused_signature_holds_no_struct`. A signature that passes holds every
/// schema it names, the return and each parameter, and a schema's own fields.
#[test]
fn a_passing_signature_holds_every_struct_it_names() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let i32_ty = m.i32_type();
    let inner = m.get_or_insert_named_struct("HoldInner");
    m.set_struct_body_dyn(inner, [i32_ty], false)?;
    let outer = m.get_or_insert_named_struct("HoldOuter");
    m.set_struct_body_dyn(outer, [inner.as_type(), m.i64_type().as_type()], false)?;
    let fn_ty = m.function_type(inner.as_type(), [outer.as_type()])?;
    let f = m.add_function_dyn("whole", fn_ty, Linkage::External)?;

    let _ = m
        .view(f)
        .with_typed_signature::<fn(HoldOuter) -> HoldInner>()?;
    for (held, name) in [(inner, "HoldInner"), (outer, "HoldOuter")] {
        assert_eq!(
            m.set_struct_name(held, "Renamed"),
            Err(IrError::SchemaStructNameNotSettable { name: name.into() })
        );
    }
    Ok(())
}

/// llvmkit-specific, no upstream counterpart: what a mint answers on the shapes
/// that decide which struct a refusal names. The checks run on a body as the
/// mint sets it, deepest first, so a cycle is refused where its innermost frame
/// closes; and a schema met twice is compared with the body its first meeting
/// set, even when that first meeting was another schema of the same name. These
/// expectations were read off the mint before the plan existed, which is what
/// makes them a pin on the plan.
#[test]
fn a_mint_answers_by_the_bodies_it_has_set_so_far() {
    assert_eq!(mint_answer::<YTop>(), recursive("YMid"));
    assert_eq!(mint_answer::<RepTop>(), mismatch("RepLeaf"));
    assert_eq!(mint_answer::<Twins>(), Ok(()));
    assert_eq!(mint_answer::<TwinClash>(), mismatch("Twin"));
    assert_eq!(mint_answer::<DeepClash>(), mismatch("DeepNest"));
    assert_eq!(mint_answer::<Diamond>(), Ok(()));
    assert_eq!(mint_answer::<FreshThenCycle>(), recursive("CycleA"));
}

/// llvmkit-specific, no upstream counterpart: a mint confirms an existing body by
/// how each element spells, so every layout's spelling has to agree with the type
/// it builds, a nested name that needs quotes included. Minting twice is that
/// check: a spelling that drifted would refuse a struct it should reuse.
#[test]
fn every_layout_confirms_the_body_it_minted() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let first = m.get_or_insert_struct_of::<EveryLayout>()?;
    let second = m.get_or_insert_struct_of::<EveryLayout>()?;
    assert_eq!(first.as_type(), second.as_type());
    let packed = m.get_or_insert_struct_of::<PackedPair>()?;
    assert_eq!(
        m.get_or_insert_struct_of::<PackedPair>()?.as_type(),
        packed.as_type()
    );
    Ok(())
}

/// llvmkit-specific, no upstream counterpart: a mint sees a body it set earlier
/// in the same call. Over a hand-built `%Rect { %Point, %Point }` with `%Point`
/// opaque, `Point` before `Rect` fills `%Point` and `%Rect` then confirms; `Rect`
/// first meets the opaque field and is refused.
#[test]
fn a_mint_confirms_an_existing_body_against_what_it_set_earlier() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let point = m.get_or_insert_named_struct("Point");
    let rect = m.get_or_insert_named_struct("Rect");
    m.set_struct_body_dyn(rect, [point.as_type(), point.as_type()], false)?;
    assert_eq!(
        m.get_or_insert_struct_of::<RectThenPoint>().map(|_| ()),
        mismatch("Rect")
    );
    assert!(point.is_opaque());
    assert!(m.get_or_insert_struct_of::<PointThenRect>().is_ok());
    assert!(!point.is_opaque());
    Ok(())
}

/// llvmkit-specific regression (D8): `mint_struct_of` minted the fields before
/// it read `%Rect`'s body, so a refused `Rect` left `%Point` in a module that
/// prints every identified struct. The check now comes first, and the module
/// prints the same as before the refused call. `a_different_existing_body_is_refused`
/// in the parser's tests is the parsed form of the same refusal.
#[test]
fn a_refused_mint_leaves_no_nested_struct() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let rect = m.get_or_insert_named_struct("Rect");
    m.set_struct_body_dyn(rect, [m.i32_type()], false)?;
    let before = format!("{m}");
    assert_eq!(
        m.get_or_insert_struct_of::<Rect>(),
        Err(IrError::StructBodyMismatch {
            name: String::from("Rect"),
        })
    );
    assert!(m.named_struct("Point").is_none());
    assert_eq!(format!("{m}"), before);
    Ok(())
}

/// llvmkit-specific, no upstream counterpart: an existing body is confirmed
/// against the whole schema, nested fields included, without filling anything.
/// A `%Rect` over an opaque `%Point` is not `Rect`'s body until `%Point` has
/// one, so it is refused and `%Point` stays opaque; minting `Point` first sets
/// that body, and `Rect` then confirms.
#[test]
fn an_existing_body_over_an_opaque_field_is_refused_until_the_field_has_a_body()
-> Result<(), IrError> {
    let m = module_new!("schema")?;
    let point = m.get_or_insert_named_struct("Point");
    let rect = m.get_or_insert_named_struct("Rect");
    m.set_struct_body_dyn(rect, [point.as_type(), point.as_type()], false)?;
    assert_eq!(
        m.get_or_insert_struct_of::<Rect>(),
        Err(IrError::StructBodyMismatch {
            name: String::from("Rect"),
        })
    );
    assert!(point.is_opaque());
    let _ = m.get_or_insert_struct_of::<Point>()?;
    assert!(m.get_or_insert_struct_of::<Rect>().is_ok());
    Ok(())
}

/// llvmkit-specific typed facade over LLVM function arguments; closest upstream
/// coverage is `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`.
#[test]
fn struct_schema_params_are_branded_wrappers() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let f = m.add_typed_function::<(), (Point,), _>("takes_point", Linkage::External)?;
    let (point,) = m.view(f).params();
    let _: Hand<'_, Point, _> = point;
    assert_eq!(
        point.as_struct_value().ty().as_type(),
        m.get_or_insert_struct_of::<Point>()?.as_type()
    );
    Ok(())
}

/// llvmkit-specific checked wrapper over an existing LLVM struct argument;
/// closest upstream coverage is `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`.
#[test]
fn struct_schema_try_new_wraps_raw_struct() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let point_ty = m.get_or_insert_struct_of::<Point>()?;
    let fn_ty = m.function_type(m.void_type(), [point_ty.as_type()])?;
    let f = m.add_function_dyn("raw_take_point", fn_ty, Linkage::External)?;
    let point = Hand::from_schema_value(SchemaValue::<Point, _>::try_new(m.view(f).param(0)?)?);
    assert_eq!(
        point.as_struct_value().ty().as_type(),
        m.get_or_insert_struct_of::<Point>()?.as_type()
    );
    Ok(())
}

/// Named so `AsmWriter` must quote it.
struct Quoted;

impl StructSchema for Quoted {
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = Hand<'ctx, Self, B, C>;
    type Fields = (i32,);

    const NAME: &'static str = "needs quotes";
}

/// A refused schema check changes nothing: the schema's struct is named in the
/// error, not created to be named, and an opaque struct of that name keeps no
/// body. llvmkit-specific regression (D8): spelling the expected type minted
/// it, so a failed check added a struct the printed module then showed
/// (`docs/divergences.md`, entry 143), and on a verified module filled an
/// opaque struct's body through a read-only handle.
#[test]
fn a_refused_schema_check_mints_nothing() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let rect_ty = m.get_or_insert_struct_of::<Rect>()?;
    let fn_ty = m.function_type(m.void_type(), [rect_ty.as_type()])?;
    let f = m.add_function_dyn("raw_take_rect", fn_ty, Linkage::External)?;
    let before = format!("{m}");

    let Err(IrError::TypeIdentityMismatch { expected, got }) =
        SchemaValue::<Quoted, _>::try_new(m.view(f).param(0)?)
    else {
        panic!("a Rect is not a Quoted");
    };
    assert_eq!(expected.spelling(), "%\"needs quotes\"");
    assert_eq!(expected.kind(), got.kind());
    assert!(m.named_struct("needs quotes").is_none());
    assert_eq!(format!("{m}"), before);

    let void_f = m.function::<()>("raw_take_rect")?.expect("declared above");
    let refused = m.view(void_f).with_typed_signature::<fn(Quoted)>();
    assert!(
        matches!(refused, Err(IrError::TypeIdentityMismatch { .. })),
        "{refused:?}"
    );
    assert!(m.named_struct("needs quotes").is_none());
    assert_eq!(format!("{m}"), before);

    let opaque = m.opaque_struct("needs quotes")?;
    assert!(SchemaValue::<Quoted, _>::try_new(m.view(f).param(0)?).is_err());
    assert!(opaque.is_opaque());
    Ok(())
}

/// The spelling a refused check names is the one `Type::rendered` gives the
/// schema's own type once it exists, quoting included: `TypePrinting::print`
/// spells an identified struct through `printLLVMName`
/// (`lib/IR/AsmWriter.cpp`). llvmkit-specific regression: `Type`'s `Display`
/// wrote `%needs quotes`, which no parser reads.
#[test]
fn a_refused_schema_check_spells_the_type_the_schema_mints() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let rect_ty = m.get_or_insert_struct_of::<Rect>()?;
    let fn_ty = m.function_type(m.void_type(), [rect_ty.as_type()])?;
    let f = m.add_function_dyn("raw_take_rect", fn_ty, Linkage::External)?;
    let Err(IrError::TypeIdentityMismatch { expected, .. }) =
        SchemaValue::<Quoted, _>::try_new(m.view(f).param(0)?)
    else {
        panic!("a Rect is not a Quoted");
    };
    let minted = m.get_or_insert_struct_of::<Quoted>()?;
    assert_eq!(expected, minted.as_type().rendered());
    assert_eq!(minted.as_type().to_string(), "%\"needs quotes\"");
    Ok(())
}

/// A refused field read names its field's type without minting it. Mirrors
/// the same rule as [`a_refused_schema_check_mints_nothing`], on the builder's
/// route: `extract_field` spelled the expected type by building it.
/// llvmkit-specific regression (D8).
#[test]
fn a_refused_field_read_mints_nothing() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let f = m.add_typed_function::<(), (Point,), _>("takes_point", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<()>(&m).position_at_end(entry);
    let (point,) = m.view(f).params();
    let Err(IrError::TypeIdentityMismatch { expected, .. }) =
        b.extract_field::<Point, Rect, _, _>(point, 0, "x")
    else {
        panic!("field 0 of a Point is no Rect");
    };
    assert_eq!(expected.spelling(), "%Rect");
    assert!(m.named_struct("Rect").is_none());
    Ok(())
}

/// llvmkit-specific checked wrapper rejection for schema/name/body mismatch;
/// closest upstream coverage is `unittests/IR/TypeBuilderTest.cpp::TEST(TypeBuilder, NamedStruct)`.
#[test]
fn struct_schema_try_new_rejects_wrong_schema() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let rect_ty = m.get_or_insert_struct_of::<Rect>()?;
    let fn_ty = m.function_type(m.void_type(), [rect_ty.as_type()])?;
    let f = m.add_function_dyn("raw_take_rect", fn_ty, Linkage::External)?;
    // Two identified structs both label `struct`, so reporting only the kinds
    // rendered "type mismatch: expected struct, got struct" — an assertion
    // that would have passed just as well had the schemas been swapped, or
    // had any other struct arrived.
    let point_ty = m.get_or_insert_struct_of::<Point>()?;
    assert_eq!(
        SchemaValue::<Point, _>::try_new(m.view(f).param(0)?),
        Err(IrError::TypeIdentityMismatch {
            expected: point_ty.as_type().rendered(),
            got: rect_ty.as_type().rendered(),
        })
    );
    let error =
        SchemaValue::<Point, _>::try_new(m.view(f).param(0)?).expect_err("a Rect is not a Point");
    assert_eq!(
        error.to_string(),
        "type mismatch: expected '%Point', got '%Rect'"
    );
    Ok(())
}

/// llvmkit-specific flattened schema-parameter facade over LLVM function
/// arguments; closest upstream coverage is
/// `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`.
#[test]
fn struct_fields_unpacks_manual_schema_into_params() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let f =
        m.add_typed_function::<(), StructFields<Point>, _>("take_point_fields", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<()>(&m).position_at_end(entry);
    let (x, y) = m.view(f).params();
    let _: IntValue<'_, i32, _> = x;
    let _: IntValue<'_, i32, _> = y;
    b.ret_void();
    let text = format!("{m}");
    assert!(
        text.contains("define void @take_point_fields(i32 %0, i32 %1)"),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports the aggregate indexing shape from `test/Bitcode/compatibility.ll`
/// — its `@instructions.aggregateops` (`extractvalue { i8, i32 }` /
/// `insertvalue { i8, i32 }`) — with a typed field
/// schema layered over the positional indices.
#[test]
fn struct_schema_extracts_and_inserts_typed_fields() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let point_ty = m.get_or_insert_struct_of::<Point>()?;
    let fn_ty = m.function_type(m.void_type(), [point_ty.as_type()])?;
    let f = m.add_function_dyn("edit", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let point = Hand::from_schema_value(SchemaValue::<Point, _>::try_new(m.view(f).param(0)?)?);
    let x = point.x(&b)?;
    let _: IntValue<'_, i32, _> = x;
    let _updated = point.with_x(&b, 42_i32)?;
    b.ret_void()?;
    let text = format!("{m}");
    assert!(text.contains("extractvalue %Point %0, 0\n"), "got:\n{text}");
    assert!(
        text.contains("insertvalue %Point %0, i32 42, 0\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// llvmkit-specific transactional builder helper check: typed aggregate helpers
/// must reject mismatched field schemas before mutating the block.
#[test]
fn struct_schema_extract_field_mismatch_does_not_append_instruction() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let point_ty = m.get_or_insert_struct_of::<Point>()?;
    let fn_ty = m.function_type(m.void_type(), [point_ty.as_type()])?;
    let f = m.add_function_dyn("bad_extract", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let point = Hand::from_schema_value(SchemaValue::<Point, _>::try_new(m.view(f).param(0)?)?);
    let err = b
        .extract_field::<Point, i64, _, _>(point, 0, "bad")
        .expect_err("field type mismatch must be rejected");
    // Both sides are integers, so `TypeKindLabel` alone rendered "expected
    // integer, got integer" — a sentence with no content, blessed here by a
    // passing assertion until the identity variant landed.
    assert_eq!(
        err,
        IrError::TypeIdentityMismatch {
            expected: m.i64_type().as_type().rendered(),
            got: m.i32_type().as_type().rendered(),
        }
    );
    assert_eq!(err.to_string(), "type mismatch: expected 'i64', got 'i32'");
    assert_eq!(b.insert_block().instructions().len(), 0);
    b.ret_void()?;
    Ok(())
}

/// llvmkit-specific regression, no upstream counterpart —
/// `IRBuilder::CreateInsertValue` returns a `Value *` and knows no schema:
/// `insert_field` checks its result against `S` before wrapping it, because
/// the aggregate's lift is user-implementable. A lift that hands back a
/// literal `{ i32, i32 }` for a `Point` aggregate takes the insert — the
/// element types agree — and is then refused, never wrapped as a `Point`
/// value. `struct_schema_extracts_and_inserts_typed_fields` is the positive
/// control.
#[test]
fn insert_field_refuses_a_lift_of_another_struct_type() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let literal = m.struct_type([m.i32_type(), m.i32_type()])?;
    let point_ty = m.get_or_insert_struct_of::<Point>()?;
    let fn_ty = m.function_type(m.void_type(), [literal.as_type()])?;
    let f = m.add_function_dyn("forged", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let forged = Forged(m.view(f).param(0)?.as_erased());
    assert_eq!(
        b.insert_field::<Point, i32, _, _, _>(forged, 7_i32, 0, "forged")
            .map(|_| ()),
        Err(IrError::TypeIdentityMismatch {
            expected: point_ty.as_type().rendered(),
            got: literal.as_type().rendered(),
        })
    );
    b.ret_void()?;
    Ok(())
}

/// llvmkit-specific typed return facade over LLVM by-value struct returns;
/// closest upstream coverage is `unittests/IR/AsmWriterTest.cpp` for aggregate
/// return printing.
#[test]
fn struct_schema_can_be_function_return() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let f = m.add_typed_function::<Point, (), _>("origin", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new(&m).position_at_end(entry);
    let poison = m
        .get_or_insert_struct_of::<Point>()?
        .as_type()
        .poison()
        .as_constant();
    let point = b.insert_field::<Point, i32, _, _, _>(poison, 1_i32, 0, "p0")?;
    let point = b.insert_field::<Point, i32, _, _, _>(point, 2_i32, 1, "p1")?;
    b.ret(point.as_struct_value())?;
    let text = format!("{m}");
    assert!(text.contains("define %Point @origin()"), "got:\n{text}");
    assert!(
        text.contains("ret %Point { i32 1, i32 2 }\n"),
        "got:\n{text}"
    );
    Ok(())
}

/// Ports the nested aggregate indexing shape from `test/Bitcode/compatibility.ll`
/// — its `@instructions.aggregateops` (`extractvalue { i8, { i32 } } %n, 1, 0`)
/// — through nested named
/// schemas so a `%Rect` field returns the `Point` schema's value, not a raw
/// `StructValue`.
#[test]
fn nested_struct_schema_accessors_return_nested_wrapper() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let rect_ty = m.get_or_insert_struct_of::<Rect>()?;
    let fn_ty = m.function_type(m.void_type(), [rect_ty.as_type()])?;
    let f = m.add_function_dyn("read", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let rect = Hand::from_schema_value(SchemaValue::<Rect, _>::try_new(m.view(f).param(0)?)?);
    let min = rect.min(&b)?;
    let max = rect.max(&b)?;
    let _: Hand<'_, Point, _> = min;
    let _: Hand<'_, Point, _> = max;
    let _ = min.x(&b)?;
    b.ret_void()?;
    let text = format!("{m}");
    assert!(
        text.contains("%Point = type { i32, i32 }\n%Rect = type { %Point, %Point }"),
        "got:\n{text}"
    );
    assert!(text.contains("extractvalue %Rect %0, 0\n"), "got:\n{text}");
    assert!(
        text.contains("extractvalue %Point %min, 0\n"),
        "got:\n{text}"
    );
    Ok(())
}
