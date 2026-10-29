use llvmkit_ir::{
    Capability, Dyn, IntValue, IntoErasedValue, IntoIrField, IntoLayoutValue, IrBuilder, IrError,
    Linkage, ModuleBrand, ModuleRef, Mutable, SchemaValue, StructFields, StructSchema,
    StructSchemaValue, StructValue, Value, module_new,
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

/// llvmkit-specific typed facade over LLVM function arguments; closest upstream
/// coverage is `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`.
#[test]
fn struct_schema_params_are_branded_wrappers() -> Result<(), IrError> {
    let m = module_new!("schema")?;
    let f = m.add_typed_function::<(), (Point,), _>("takes_point", Linkage::External)?;
    let (point,) = m.view(f).params();
    let _: Hand<'_, Point, _> = point;
    assert_eq!(
        point.as_struct_value().ty().as_type().read_only(),
        m.as_view().get_or_insert_struct_of::<Point>()?.as_type()
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
        point.as_struct_value().ty().as_type().read_only(),
        m.as_view().get_or_insert_struct_of::<Point>()?.as_type()
    );
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
        .as_view()
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
