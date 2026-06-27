use llvmkit_ir::{IRBuilder, IrError, IrStruct, Linkage, Module, NoFolder};

#[derive(IrStruct)]
struct Point {
    x: i32,
    y: i32,
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

    Module::with_new("derived", |m| {
        let f = m.add_typed_function::<WindowPlacement, (WindowPlacement,), _>(
            "normalize",
            Linkage::External,
        )?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let (placement,) = f.params();
        let rect = placement.normal_position(&b)?;
        let min = rect.min(&b)?;
        let max = rect.max(&b)?;
        let x = min.x(&b)?;
        let adjusted_min = PointValue::build(&m, &b, x, max.y(&b)?, "adjusted_min")?;
        let adjusted_rect = RectValue::build(&m, &b, adjusted_min, max, "adjusted_rect")?;
        let rebuilt =
            WindowPlacementValue::build(&m, &b, placement.show_cmd(&b)?, adjusted_rect, "rebuilt")?;
        b.build_ret(rebuilt)?;

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
    })
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

    Module::with_new("collision", |m| {
        let f = m.add_typed_function::<CollisionNames, (), _>("collision", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let value = CollisionNamesValue::build(&m, &b, 1_i32, 2_i32, 3_i32, "collision")?;
        b.build_ret(value)?;
        let text = format!("{m}");
        assert!(
            text.contains("ret %CollisionNames %collision.name"),
            "got:\n{text}"
        );
        Ok(())
    })
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

    Module::with_new("attrs", |m| {
        let _ = <Pair as llvmkit_ir::StructSchema>::ir_type(&m)?;
        let text = format!("{m}");
        assert!(
            text.contains("%Renamed = type <{ i32, i32 }>"),
            "got:\n{text}"
        );
        Ok(())
    })
}
