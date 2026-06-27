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

type NormalizePlacement = fn(WindowPlacement) -> WindowPlacement;

fn main() -> Result<(), IrError> {
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

    let ir = Module::with_new("window", |m| {
        let f = m.add_typed_function_of::<NormalizePlacement, _>(
            "normalize_window_placement",
            Linkage::External,
        )?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let (placement,) = f.params();

        let normal_position = placement.normal_position(&b)?;
        let min = normal_position.min(&b)?;
        let max = normal_position.max(&b)?;
        let min_x = min.x(&b)?;
        let max_y = max.y(&b)?;

        let rebuilt_min = PointValue::build(&m, &b, min_x, max_y, "normal_position.min")?;
        let rebuilt_rect = RectValue::build(&m, &b, rebuilt_min, max, "normal_position")?;
        let rebuilt = WindowPlacementValue::build(
            &m,
            &b,
            placement.show_cmd(&b)?,
            rebuilt_rect,
            "placement",
        )?;
        b.build_ret(rebuilt)?;

        Ok(format!("{m}"))
    })?;

    print!("{ir}");
    Ok(())
}
