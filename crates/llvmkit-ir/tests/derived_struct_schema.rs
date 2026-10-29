use llvmkit_ir::{
    ArrLen, ArrayValue, Capability, ConstantFolder, FloatValue, IntValue, IrBuilder, IrError,
    IrField, IrStruct, LayoutValue, Linkage, Module, ModuleBrand, NoFolder, PointerValue, Ptr,
    Space, StructFields, module_new,
};

#[derive(IrStruct)]
struct Point {
    x: i32,
    y: i32,
}

/// One field of each kind a layout names: an unsigned integer, a Rust array
/// of a scalar, a pointer and a nested schema.
#[derive(IrStruct)]
struct Sample {
    a: u32,
    b: [f32; 4],
    c: Ptr,
    d: Point,
}

/// The spec's hand-written field kind: a distance, laid out as a `double`.
struct Meters;

impl IrField for Meters {
    type Layout = f64;
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = FloatValue<'ctx, f64, B, C>;

    fn wrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        raw: LayoutValue<'ctx, f64, B, C>,
    ) -> Self::Value<'ctx, B, C> {
        raw
    }

    fn unwrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        value: Self::Value<'ctx, B, C>,
    ) -> LayoutValue<'ctx, f64, B, C> {
        value
    }
}

#[derive(IrStruct)]
struct Run {
    distance: Meters,
}

#[derive(IrStruct)]
struct Rect {
    min: Point,
    max: Point,
}

#[derive(IrStruct)]
struct WindowPlacement {
    show_cmd: i32,
    normal_position: Rect,
}

/// llvmkit-specific derive facade over LLVM identified structs; closest upstream
/// coverage is `unittests/IR/TypeBuilderTest.cpp::TEST(TypeBuilder, NamedStruct)`
/// for named struct body reuse and `test/Bitcode/compatibility.ll` aggregate
/// `extractvalue` / `insertvalue` print forms.
#[test]
fn derive_builds_nested_named_structs_and_accessors() -> Result<(), IrError> {
    let rust_point = Point { x: 1, y: 2 };
    let rust_rect = Rect {
        min: Point {
            x: rust_point.x,
            y: rust_point.y,
        },
        max: Point { x: 3, y: 4 },
    };
    let rust_window = WindowPlacement {
        show_cmd: 1,
        normal_position: rust_rect,
    };
    let _ = rust_window.show_cmd
        + rust_window.normal_position.min.x
        + rust_window.normal_position.max.y;

    let m = module_new!("derived")?;
    let f = m.add_typed_function::<WindowPlacement, (WindowPlacement,), _>(
        "normalize",
        Linkage::External,
    )?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (placement,) = m.view(f).params();
    let rect = placement.normal_position(&b);
    let min = rect.min(&b);
    let max = rect.max(&b);
    let x = min.x(&b);
    let adjusted_min = PointValue::build(m.as_view(), &b, x, max.y(&b), "adjusted_min")?;
    let adjusted_rect = RectValue::build(m.as_view(), &b, adjusted_min, max, "adjusted_rect")?;
    let rebuilt = WindowPlacementValue::build(
        m.as_view(),
        &b,
        placement.show_cmd(&b),
        adjusted_rect,
        "rebuilt",
    )?;
    b.ret(rebuilt)?;

    let text = format!("{m}");
    assert!(text.contains("%Point = type { i32, i32 }"), "got:\n{text}");
    assert!(
        text.contains("%Rect = type { %Point, %Point }"),
        "got:\n{text}"
    );
    assert!(
        text.contains("%WindowPlacement = type { i32, %Rect }"),
        "got:\n{text}"
    );
    assert!(
        text.contains("define %WindowPlacement @normalize(%WindowPlacement %0)"),
        "got:\n{text}"
    );
    assert!(
        text.contains("extractvalue %WindowPlacement %0, 1"),
        "got:\n{text}"
    );
    assert!(
        text.contains("extractvalue %Rect %normal_position, 0"),
        "got:\n{text}"
    );
    assert!(text.contains("extractvalue %Point %min, 0"), "got:\n{text}");
    assert!(
        text.contains("insertvalue %Point poison, i32 %x, 0"),
        "got:\n{text}"
    );
    assert!(
        text.contains("insertvalue %Rect poison, %Point %adjusted_min.y, 0"),
        "got:\n{text}"
    );
    assert!(
        text.contains("insertvalue %WindowPlacement poison, i32 %show_cmd, 0"),
        "got:\n{text}"
    );
    assert!(
        text.contains("ret %WindowPlacement %rebuilt.normal_position"),
        "got:\n{text}"
    );
    Ok(())
}

/// llvmkit-specific checked wrapper for derive-generated struct values; closest
/// upstream coverage is `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`.
#[test]
fn derive_try_from_raw_ir_values() -> Result<(), IrError> {
    let m = module_new!("derived_try_from")?;
    let f = m.add_typed_function::<(), (WindowPlacement,), _>("read_raw", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let arg = m.view(f).as_function().param(0)?;
    let placement = WindowPlacementValue::try_from(arg)?;
    let normal_position = placement.normal_position(&b);
    let _: RectValue<'_, _> = normal_position;
    b.ret_void();
    Ok(())
}

/// llvmkit-specific flattened schema-parameter facade over LLVM function
/// arguments, with aggregate return coverage from `test/Bitcode/compatibility.ll`
/// `extractvalue` / `insertvalue` forms.
#[test]
fn derive_struct_fields_unpacks_top_level_fields() -> Result<(), IrError> {
    let m = module_new!("derived_fields")?;
    let f = m.add_typed_function::<WindowPlacement, StructFields<WindowPlacement>, _>(
        "normalize_fields",
        Linkage::External,
    )?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (show_cmd, normal_position) = m.view(f).params();
    let _: IntValue<'_, i32, _> = show_cmd;
    let _: RectValue<'_, _> = normal_position;
    let rebuilt =
        WindowPlacementValue::build(m.as_view(), &b, show_cmd, normal_position, "rebuilt")?;
    b.ret(rebuilt)?;
    let text = format!("{m}");
    assert!(
        text.contains("define %WindowPlacement @normalize_fields(i32 %0, %Rect %1)"),
        "got:\n{text}"
    );
    assert!(text.contains("ret %WindowPlacement"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific derive hygiene regression: Rust field names may collide
/// with generated helper parameter names without breaking the generated builder.
#[test]
fn derive_build_accepts_fields_named_like_helper_parameters() -> Result<(), IrError> {
    #[derive(IrStruct)]
    struct CollisionNames {
        module: i32,
        builder: i32,
        name: i32,
    }

    let rust_value = CollisionNames {
        module: 1,
        builder: 2,
        name: 3,
    };
    let _ = rust_value.module + rust_value.builder + rust_value.name;

    let m = module_new!("collision")?;
    let f = m.add_typed_function::<CollisionNames, (), _>("collision", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let value = CollisionNamesValue::build(m.as_view(), &b, 1_i32, 2_i32, 3_i32, "collision")?;
    b.ret(value)?;
    let text = format!("{m}");
    assert!(
        text.contains("ret %CollisionNames %collision.name"),
        "got:\n{text}"
    );
    Ok(())
}

/// llvmkit-specific helper attributes; closest upstream coverage is the same
/// named struct reuse path as `TypeBuilder.NamedStruct`.
#[test]
fn derive_supports_name_override_and_packed() -> Result<(), IrError> {
    #[derive(IrStruct)]
    #[llvmkit(name = "Renamed", packed)]
    struct Pair {
        a: i32,
        b: i32,
    }

    let rust_pair = Pair { a: 1, b: 2 };
    let _ = rust_pair.a + rust_pair.b;

    let m = module_new!("attrs")?;
    let _ = m.get_or_insert_struct_of::<Pair>()?;
    let text = format!("{m}");
    assert!(
        text.contains("%Renamed = type <{ i32, i32 }>"),
        "got:\n{text}"
    );
    Ok(())
}

/// llvmkit-specific (D2, D10), no upstream counterpart — a C++ `StructType`
/// is built from `Type *`s, never from a Rust declaration: a derived body is
/// minted from its fields' layouts alone. An unsigned field is the signless
/// integer of its width (LLVM integers carry no sign), a Rust array is an
/// LLVM array of its element's layout, `Ptr` is `ptr`, and a nested schema is
/// its own named type.
#[test]
fn a_derived_body_comes_from_its_field_layouts() -> Result<(), IrError> {
    let rust_sample = Sample {
        a: 1,
        b: [0.5; 4],
        c: Ptr::null(),
        d: Point { x: 1, y: 2 },
    };
    let Sample { a, b, c, d } = rust_sample;
    let _ = (a, b, c, d.x + d.y);

    let m = module_new!("layouts")?;
    let _ = m.get_or_insert_struct_of::<Sample>()?;
    let text = format!("{m}");
    assert!(
        text.contains("%Sample = type { i32, [4 x float], ptr, %Point }"),
        "got:\n{text}"
    );
    assert!(text.contains("%Point = type { i32, i32 }"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific (D2, D10), no upstream counterpart: a field read checks
/// the element's type against the field's layout before it wraps the value,
/// so each layout must recognise its own type — an unsigned field reads back
/// the signless `i32`, an array field its typed `[4 x float]`, a `Ptr` field
/// an address-space-0 pointer and a nested schema its value.
/// `a_derived_body_comes_from_its_field_layouts` is the minting half.
#[test]
fn a_derived_field_of_each_layout_reads_its_value() -> Result<(), IrError> {
    let m = module_new!("layout_reads")?;
    let f = m.add_typed_function::<(), (Sample,), _>("read", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (sample,) = m.view(f).params();
    let a: IntValue<'_, i32, _> = sample.a(&b);
    let array: ArrayValue<'_, f32, ArrLen<4>, _> = sample.b(&b);
    let c: PointerValue<'_, Space<0>, _> = sample.c(&b);
    let d: PointValue<'_, _> = sample.d(&b);
    let _ = (a, array, c, d);
    b.ret_void();
    let text = format!("{m}");
    for (field, index) in [("a", 0), ("b", 1), ("c", 2), ("d", 3)] {
        assert!(
            text.contains(&format!("%{field} = extractvalue %Sample %0, {index}")),
            "got:\n{text}"
        );
    }
    Ok(())
}

/// llvmkit-specific (D7, D10), no upstream counterpart —
/// `IRBuilderBase::CreateExtractValue` returns a `Value *` and cannot report a
/// foreign or mistyped aggregate at all: on a `UniqueBrand` module with one of
/// llvmkit's own folders, a derived field read needs no `?`. The registry keeps
/// one live module per brand, so the value is the builder's module's, and the
/// schema value proves the field's type.
#[test]
fn a_field_read_needs_no_question_mark() -> Result<(), IrError> {
    let m = module_new!("infallible_read")?;
    let f = m.add_typed_function::<i32, (Point,), _>("first", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (p,) = m.view(f).params();
    let x: IntValue<'_, i32, _> = p.x(&b);
    b.ret(x)?;
    let text = format!("{m}");
    assert!(
        text.contains("%x = extractvalue %Point %0, 0"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `ConstantFolder::FoldExtractValue`, which hands a constant
/// aggregate to `ConstantFoldExtractValueInstruction` (`lib/IR/ConstantFold.cpp`)
/// and gets back its element: read through `ConstantFolder`, field `x` of a
/// constant `%Point { i32 1, i32 2 }` is the constant `i32 1`, and no
/// instruction is appended.
#[test]
fn a_constant_struct_field_read_folds() -> Result<(), IrError> {
    let m = module_new!("folded_read")?;
    let i32_ty = m.i32_type();
    let point = m
        .get_or_insert_struct_of::<Point>()?
        .const_struct([i32_ty.const_int(1_i32), i32_ty.const_int(2_i32)])?;
    let p = PointValue::try_from(point.as_constant())?;
    let f = m.add_typed_function::<i32, (), _>("one", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, ConstantFolder).position_at_end(entry);
    let x = p.x(&b);
    assert_eq!(x.as_erased(), i32_ty.const_int(1_i32).as_erased());
    assert_eq!(b.insert_block().instructions().len(), 0);
    b.ret(x)?;
    Ok(())
}

/// llvmkit-specific (D7), no upstream counterpart: a `Module::dynamic`
/// module's `DynBrand` names many live modules, so its derived reads keep the
/// checked `try_` form — the value is admitted at the builder's module and the
/// folder's result is checked. `field_read_on_dyn_brand.rs` is the
/// compile-fail half.
#[test]
fn a_dynamic_module_reads_with_try() -> Result<(), IrError> {
    let m = Module::dynamic("dynamic_read");
    let f = m.add_typed_function::<i32, (Point,), _>("first", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (p,) = m.view(f).params();
    let x: IntValue<'_, i32, _> = p.try_x(&b)?;
    b.ret(x)?;
    let text = format!("{m}");
    assert!(
        text.contains("%x = extractvalue %Point %0, 0"),
        "got:\n{text}"
    );
    Ok(())
}

/// llvmkit-specific (D2), no upstream counterpart: a field kind the user
/// writes names a layout, and llvmkit builds the field's type from that
/// layout — `Meters` is laid out as `f64`, so `%Run` is `{ double }` and the
/// derived getter hands back the layout's `FloatValue<f64>`.
#[test]
fn a_custom_field_kind_names_its_layout() -> Result<(), IrError> {
    let Run { distance: Meters } = Run { distance: Meters };

    let m = module_new!("custom_kind")?;
    let f = m.add_typed_function::<(), (Run,), _>("pace", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (run,) = m.view(f).params();
    let distance: FloatValue<'_, f64, _> = run.distance(&b);
    let _ = distance;
    b.ret_void();
    let text = format!("{m}");
    assert!(text.contains("%Run = type { double }"), "got:\n{text}");
    assert!(
        text.contains("%distance = extractvalue %Run %0, 0"),
        "got:\n{text}"
    );
    Ok(())
}

/// Review Focus 1 of the type-constructor validity plan (llvmkit-specific; the
/// rule is upstream's — `StructType::setBody` is never called twice on one
/// named struct): a module already holding `%Point` with another body, built
/// by hand, refuses the derived schema with `StructBodyMismatch` rather than
/// minting a second struct. The parsed form is
/// `llvmkit-asmparser/tests/parser_facade.rs::a_different_existing_body_is_refused`.
#[test]
fn a_hand_built_different_body_is_refused() -> Result<(), IrError> {
    let m = module_new!("hand_built_point")?;
    let point = m.get_or_insert_named_struct("Point");
    m.set_struct_body_dyn(point, [m.i64_type()], false)?;
    assert_eq!(
        m.get_or_insert_struct_of::<Point>(),
        Err(IrError::StructBodyMismatch {
            name: "Point".into(),
        })
    );
    Ok(())
}
