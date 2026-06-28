use llvmkit_ir::{
    Constant, IRBuilder, IntValue, IntoIrField, IrError, IrField, Linkage, Module, ModuleBrand,
    StructFields, StructSchema, StructSchemaValue, StructValue, Type, TypeKindLabel, Unverified,
    ValidatedStructValue, Value,
};

struct Point;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PointValue<'ctx, B: ModuleBrand = llvmkit_ir::Brand<'ctx>> {
    raw: StructValue<'ctx, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> PointValue<'ctx, B> {
    fn x<'m, F, R>(
        self,
        b: &llvmkit_ir::IRBuilder<'m, 'ctx, B, F, llvmkit_ir::Positioned, R>,
    ) -> Result<IntValue<'ctx, i32, B>, IrError>
    where
        F: llvmkit_ir::IRBuilderFolder<'ctx, B>,
        R: llvmkit_ir::ReturnMarker,
    {
        b.build_extract_field::<Point, i32, _, _>(self, 0, "x")
    }

    fn with_x<'m, F, R, V>(
        self,
        b: &llvmkit_ir::IRBuilder<'m, 'ctx, B, F, llvmkit_ir::Positioned, R>,
        value: V,
    ) -> Result<Self, IrError>
    where
        F: llvmkit_ir::IRBuilderFolder<'ctx, B>,
        R: llvmkit_ir::ReturnMarker,
        V: IntoIrField<'ctx, i32, B>,
    {
        b.build_insert_field::<Point, i32, _, _, _>(self, value, 0, "with_x")
    }

    fn as_struct_value(self) -> StructValue<'ctx, B> {
        self.raw
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> StructSchemaValue<'ctx, Point, B> for PointValue<'ctx, B> {
    fn as_struct_value(self) -> StructValue<'ctx, B> {
        self.raw
    }

    fn from_struct_value(raw: StructValue<'ctx, B>, _validated: &ValidatedStructValue<'_>) -> Self {
        Self { raw }
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> IntoIrField<'ctx, Point, B> for PointValue<'ctx, B> {
    fn into_ir_field(
        self,
        _module: llvmkit_ir::ModuleRef<'ctx, B>,
    ) -> Result<Value<'ctx, B>, IrError> {
        Ok(self.raw.as_value())
    }
}

impl StructSchema for Point {
    type Value<'ctx, B: ModuleBrand + 'ctx> = PointValue<'ctx, B>;
    type FieldParams = (i32, i32);

    const NAME: &'static str = "Point";

    fn field_types<'ctx, B>(
        module: &Module<'ctx, B, Unverified>,
    ) -> Result<Vec<Type<'ctx, B>>, IrError>
    where
        B: ModuleBrand + 'ctx,
    {
        Ok(vec![
            <i32 as IrField>::ir_type(module)?,
            <i32 as IrField>::ir_type(module)?,
        ])
    }

    fn matches_fields<'ctx, B>(fields: &[Type<'ctx, B>]) -> bool
    where
        B: ModuleBrand + 'ctx,
    {
        fields.len() == 2
            && <i32 as IrField>::matches_ir_type(fields[0])
            && <i32 as IrField>::matches_ir_type(fields[1])
    }
}

struct BadPoint;

impl<'ctx, B: ModuleBrand + 'ctx> StructSchemaValue<'ctx, BadPoint, B> for PointValue<'ctx, B> {
    fn as_struct_value(self) -> StructValue<'ctx, B> {
        self.raw
    }

    fn from_struct_value(raw: StructValue<'ctx, B>, _validated: &ValidatedStructValue<'_>) -> Self {
        Self { raw }
    }
}

impl StructSchema for BadPoint {
    type Value<'ctx, B: ModuleBrand + 'ctx> = PointValue<'ctx, B>;
    type FieldParams = (i64,);

    const NAME: &'static str = "Point";

    fn field_types<'ctx, B>(
        module: &Module<'ctx, B, Unverified>,
    ) -> Result<Vec<Type<'ctx, B>>, IrError>
    where
        B: ModuleBrand + 'ctx,
    {
        Ok(vec![<i64 as IrField>::ir_type(module)?])
    }

    fn matches_fields<'ctx, B>(fields: &[Type<'ctx, B>]) -> bool
    where
        B: ModuleBrand + 'ctx,
    {
        fields.len() == 1 && <i64 as IrField>::matches_ir_type(fields[0])
    }
}

struct RecursiveNode;

impl<'ctx, B: ModuleBrand + 'ctx> StructSchemaValue<'ctx, RecursiveNode, B>
    for PointValue<'ctx, B>
{
    fn as_struct_value(self) -> StructValue<'ctx, B> {
        self.raw
    }

    fn from_struct_value(raw: StructValue<'ctx, B>, _validated: &ValidatedStructValue<'_>) -> Self {
        Self { raw }
    }
}

impl StructSchema for RecursiveNode {
    type Value<'ctx, B: ModuleBrand + 'ctx> = PointValue<'ctx, B>;
    type FieldParams = (RecursiveNode,);

    const NAME: &'static str = "RecursiveNode";

    fn field_types<'ctx, B>(
        module: &Module<'ctx, B, Unverified>,
    ) -> Result<Vec<Type<'ctx, B>>, IrError>
    where
        B: ModuleBrand + 'ctx,
    {
        Ok(vec![module.named_struct(Self::NAME).as_type()])
    }

    fn matches_fields<'ctx, B>(fields: &[Type<'ctx, B>]) -> bool
    where
        B: ModuleBrand + 'ctx,
    {
        fields.len() == 1 && <Self as IrField>::matches_ir_type(fields[0])
    }
}

struct EmptyName;

impl<'ctx, B: ModuleBrand + 'ctx> StructSchemaValue<'ctx, EmptyName, B> for PointValue<'ctx, B> {
    fn as_struct_value(self) -> StructValue<'ctx, B> {
        self.raw
    }

    fn from_struct_value(raw: StructValue<'ctx, B>, _validated: &ValidatedStructValue<'_>) -> Self {
        Self { raw }
    }
}

impl StructSchema for EmptyName {
    type Value<'ctx, B: ModuleBrand + 'ctx> = PointValue<'ctx, B>;
    type FieldParams = (i32,);

    const NAME: &'static str = "";

    fn field_types<'ctx, B>(
        module: &Module<'ctx, B, Unverified>,
    ) -> Result<Vec<Type<'ctx, B>>, IrError>
    where
        B: ModuleBrand + 'ctx,
    {
        Ok(vec![<i32 as IrField>::ir_type(module)?])
    }

    fn matches_fields<'ctx, B>(fields: &[Type<'ctx, B>]) -> bool
    where
        B: ModuleBrand + 'ctx,
    {
        fields.len() == 1 && <i32 as IrField>::matches_ir_type(fields[0])
    }
}

struct Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RectValue<'ctx, B: ModuleBrand = llvmkit_ir::Brand<'ctx>> {
    raw: StructValue<'ctx, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> RectValue<'ctx, B> {
    fn min<'m, F, R>(
        self,
        b: &llvmkit_ir::IRBuilder<'m, 'ctx, B, F, llvmkit_ir::Positioned, R>,
    ) -> Result<PointValue<'ctx, B>, IrError>
    where
        F: llvmkit_ir::IRBuilderFolder<'ctx, B>,
        R: llvmkit_ir::ReturnMarker,
    {
        b.build_extract_field::<Rect, Point, _, _>(self, 0, "min")
    }

    fn max<'m, F, R>(
        self,
        b: &llvmkit_ir::IRBuilder<'m, 'ctx, B, F, llvmkit_ir::Positioned, R>,
    ) -> Result<PointValue<'ctx, B>, IrError>
    where
        F: llvmkit_ir::IRBuilderFolder<'ctx, B>,
        R: llvmkit_ir::ReturnMarker,
    {
        b.build_extract_field::<Rect, Point, _, _>(self, 1, "max")
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> StructSchemaValue<'ctx, Rect, B> for RectValue<'ctx, B> {
    fn as_struct_value(self) -> StructValue<'ctx, B> {
        self.raw
    }

    fn from_struct_value(raw: StructValue<'ctx, B>, _validated: &ValidatedStructValue<'_>) -> Self {
        Self { raw }
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> IntoIrField<'ctx, Rect, B> for RectValue<'ctx, B> {
    fn into_ir_field(
        self,
        _module: llvmkit_ir::ModuleRef<'ctx, B>,
    ) -> Result<Value<'ctx, B>, IrError> {
        Ok(self.raw.as_value())
    }
}

impl StructSchema for Rect {
    type Value<'ctx, B: ModuleBrand + 'ctx> = RectValue<'ctx, B>;
    type FieldParams = (Point, Point);

    const NAME: &'static str = "Rect";

    fn field_types<'ctx, B>(
        module: &Module<'ctx, B, Unverified>,
    ) -> Result<Vec<Type<'ctx, B>>, IrError>
    where
        B: ModuleBrand + 'ctx,
    {
        Ok(vec![
            <Point as IrField>::ir_type(module)?,
            <Point as IrField>::ir_type(module)?,
        ])
    }

    fn matches_fields<'ctx, B>(fields: &[Type<'ctx, B>]) -> bool
    where
        B: ModuleBrand + 'ctx,
    {
        fields.len() == 2
            && <Point as IrField>::matches_ir_type(fields[0])
            && <Point as IrField>::matches_ir_type(fields[1])
    }
}

fn poison_point<'ctx, B: ModuleBrand + 'ctx>(
    module: &Module<'ctx, B>,
) -> Result<Constant<'ctx, B>, IrError> {
    Ok(<Point as StructSchema>::ir_type(module)?
        .as_type()
        .get_poison()
        .as_constant())
}

/// llvmkit-specific schema facade over LLVM named structs; closest upstream
/// coverage is `unittests/IR/TypeBuilderTest.cpp::TEST(TypeBuilder, NamedStruct)`
/// for idempotent identified-struct lookup.
#[test]
fn struct_schema_reuses_matching_named_body() -> Result<(), IrError> {
    Module::with_new("schema", |m| {
        let first = <Point as StructSchema>::ir_type(&m)?;
        let second = <Point as StructSchema>::ir_type(&m)?;
        assert_eq!(first.as_type(), second.as_type());
        let text = format!("{m}");
        assert_eq!(text.matches("%Point = type { i32, i32 }").count(), 1);
        Ok(())
    })
}

/// llvmkit-specific schema facade over LLVM named structs; closest upstream
/// coverage is `lib/IR/LLVMContextImpl.cpp::getOrCreateNamedStruct` reuse rules.
#[test]
fn struct_schema_rejects_mismatched_existing_named_body() -> Result<(), IrError> {
    Module::with_new("schema", |m| {
        let _ = <Point as StructSchema>::ir_type(&m)?;
        assert_eq!(
            <BadPoint as StructSchema>::ir_type(&m),
            Err(IrError::StructBodyMismatch {
                name: String::from("Point"),
            })
        );
        Ok(())
    })
}

/// Mirrors `StructType::setBodyOrError` / `StructType::checkBody`
/// (`lib/IR/Type.cpp`): identified struct bodies may not recursively contain
/// the struct being defined.
#[test]
fn struct_schema_rejects_recursive_named_body() -> Result<(), IrError> {
    Module::with_new("schema", |m| {
        assert_eq!(
            <RecursiveNode as StructSchema>::ir_type(&m),
            Err(IrError::InvalidOperation {
                message: "recursive struct body",
            })
        );
        Ok(())
    })
}

/// LLVM `StructType::create(Context, "")` creates an anonymous struct, not a
/// named entry with an empty printed `%` name; schema names must therefore be
/// non-empty.
#[test]
fn struct_schema_rejects_empty_identified_name() -> Result<(), IrError> {
    Module::with_new("schema", |m| {
        assert_eq!(
            <EmptyName as StructSchema>::ir_type(&m),
            Err(IrError::InvalidOperation {
                message: "struct schema name must not be empty",
            })
        );
        Ok(())
    })
}

/// llvmkit-specific typed facade over LLVM function arguments; closest upstream
/// coverage is `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`.
#[test]
fn struct_schema_params_are_branded_wrappers() -> Result<(), IrError> {
    Module::with_new("schema", |m| {
        let f = m.add_typed_function::<(), (Point,), _>("takes_point", Linkage::External)?;
        let (point,) = f.params();
        let _: PointValue<'_, _> = point;
        assert_eq!(
            point.as_struct_value().ty().as_type(),
            <Point as StructSchema>::ir_type(&m)?.as_type()
        );
        Ok(())
    })
}

/// llvmkit-specific checked wrapper over an existing LLVM struct argument;
/// closest upstream coverage is `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`.
#[test]
fn struct_schema_try_value_from_ir_wraps_raw_struct() -> Result<(), IrError> {
    Module::with_new("schema", |m| {
        let point_ty = <Point as StructSchema>::ir_type(&m)?;
        let fn_ty = m.fn_type(m.void_type(), [point_ty.as_type()], false);
        let f = m.add_function::<(), _>("raw_take_point", fn_ty, Linkage::External)?;
        let point = Point::try_value_from_ir(f.param(0)?)?;
        assert_eq!(
            point.as_struct_value().ty().as_type(),
            <Point as StructSchema>::ir_type(&m)?.as_type()
        );
        Ok(())
    })
}

/// llvmkit-specific checked wrapper rejection for schema/name/body mismatch;
/// closest upstream coverage is `unittests/IR/TypeBuilderTest.cpp::TEST(TypeBuilder, NamedStruct)`.
#[test]
fn struct_schema_try_value_from_ir_rejects_wrong_schema() -> Result<(), IrError> {
    Module::with_new("schema", |m| {
        let rect_ty = <Rect as StructSchema>::ir_type(&m)?;
        let fn_ty = m.fn_type(m.void_type(), [rect_ty.as_type()], false);
        let f = m.add_function::<(), _>("raw_take_rect", fn_ty, Linkage::External)?;
        assert_eq!(
            Point::try_value_from_ir(f.param(0)?),
            Err(IrError::TypeMismatch {
                expected: TypeKindLabel::Struct,
                got: TypeKindLabel::Struct,
            })
        );
        Ok(())
    })
}

/// llvmkit-specific flattened schema-parameter facade over LLVM function
/// arguments; closest upstream coverage is
/// `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`.
#[test]
fn struct_fields_unpacks_manual_schema_into_params() -> Result<(), IrError> {
    Module::with_new("schema", |m| {
        let f = m.add_typed_function::<(), StructFields<Point>, _>(
            "take_point_fields",
            Linkage::External,
        )?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let (x, y) = f.params();
        let _: IntValue<'_, i32, _> = x;
        let _: IntValue<'_, i32, _> = y;
        b.build_ret_void();
        let text = format!("{m}");
        assert!(
            text.contains("define void @take_point_fields(i32 %0, i32 %1)"),
            "got:\n{text}"
        );
        Ok(())
    })
}

/// Ports the aggregate indexing shape from `test/Bitcode/compatibility.ll`
/// lines 1549 and 1558 (`extractvalue` / `insertvalue`), with a typed field
/// schema layered over the positional indices.
#[test]
fn struct_schema_extracts_and_inserts_typed_fields() -> Result<(), IrError> {
    Module::with_new("schema", |m| {
        let point_ty = <Point as StructSchema>::ir_type(&m)?;
        let fn_ty = m.fn_type(m.void_type(), [point_ty.as_type()], false);
        let f = m.add_function::<(), _>("edit", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let point = PointValue {
            raw: StructValue::try_from(f.param(0)?)?,
        };
        let x = point.x(&b)?;
        let _: IntValue<'_, i32, _> = x;
        let _updated = point.with_x(&b, 42_i32)?;
        b.build_ret_void();
        let text = format!("{m}");
        assert!(text.contains("extractvalue %Point %0, 0\n"), "got:\n{text}");
        assert!(
            text.contains("insertvalue %Point %0, i32 42, 0\n"),
            "got:\n{text}"
        );
        Ok(())
    })
}

/// llvmkit-specific transactional builder helper check: typed aggregate helpers
/// must reject mismatched field schemas before mutating the block.
#[test]
fn struct_schema_extract_field_mismatch_does_not_append_instruction() -> Result<(), IrError> {
    Module::with_new("schema", |m| {
        let point_ty = <Point as StructSchema>::ir_type(&m)?;
        let fn_ty = m.fn_type(m.void_type(), [point_ty.as_type()], false);
        let f = m.add_function::<(), _>("bad_extract", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let point = PointValue {
            raw: StructValue::try_from(f.param(0)?)?,
        };
        let err = b
            .build_extract_field::<Point, i64, _, _>(point, 0, "bad")
            .expect_err("field type mismatch must be rejected");
        assert_eq!(
            err,
            IrError::TypeMismatch {
                expected: llvmkit_ir::TypeKindLabel::Integer,
                got: llvmkit_ir::TypeKindLabel::Integer,
            }
        );
        assert_eq!(b.insert_block().instructions().len(), 0);
        b.build_ret_void();
        Ok(())
    })
}

/// llvmkit-specific typed return facade over LLVM by-value struct returns;
/// closest upstream coverage is `unittests/IR/AsmWriterTest.cpp` for aggregate
/// return printing.
#[test]
fn struct_schema_can_be_function_return() -> Result<(), IrError> {
    Module::with_new("schema", |m| {
        let f = m.add_typed_function::<Point, (), _>("origin", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new(&m).position_at_end(entry);
        let point =
            b.build_insert_field::<Point, i32, _, _, _>(poison_point(&m)?, 1_i32, 0, "p0")?;
        let point = b.build_insert_field::<Point, i32, _, _, _>(point, 2_i32, 1, "p1")?;
        b.build_ret(point.as_struct_value())?;
        let text = format!("{m}");
        assert!(text.contains("define %Point @origin()"), "got:\n{text}");
        assert!(
            text.contains("ret %Point { i32 1, i32 2 }\n"),
            "got:\n{text}"
        );
        Ok(())
    })
}

/// Ports the nested aggregate indexing shape from `test/Bitcode/compatibility.ll`
/// line 1555 (`extractvalue { i8, { i32 } } %n, 1, 0`) through nested named
/// schemas so a `%Rect` field returns `PointValue`, not raw `StructValue`.
#[test]
fn nested_struct_schema_accessors_return_nested_wrapper() -> Result<(), IrError> {
    Module::with_new("schema", |m| {
        let rect_ty = <Rect as StructSchema>::ir_type(&m)?;
        let fn_ty = m.fn_type(m.void_type(), [rect_ty.as_type()], false);
        let f = m.add_function::<(), _>("read", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let rect = RectValue {
            raw: StructValue::try_from(f.param(0)?)?,
        };
        let min = rect.min(&b)?;
        let max = rect.max(&b)?;
        let _: PointValue<'_, _> = min;
        let _: PointValue<'_, _> = max;
        let _ = min.x(&b)?;
        b.build_ret_void();
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
    })
}
