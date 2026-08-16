use llvmkit_ir::{IrBuilder, IrError, IrStruct, Linkage, NoFolder, module_new};

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

    let ir = {
        let m = module_new!("window")?;
        let f = m.add_typed_function_of::<NormalizePlacement, _>(
            "normalize_window_placement",
            Linkage::External,
        )?;
        let entry = m.view(f).append_basic_block(&m, "entry");
        let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let (placement,) = m.view(f).params();

        let normal_position = placement.normal_position(&b)?;
        let min = normal_position.min(&b)?;
        let max = normal_position.max(&b)?;
        let min_x = min.x(&b)?;
        let max_y = max.y(&b)?;

        let rebuilt_min = PointValue::build(m.as_view(), &b, min_x, max_y, "normal_position.min")?;
        let rebuilt_rect = RectValue::build(m.as_view(), &b, rebuilt_min, max, "normal_position")?;
        let rebuilt = WindowPlacementValue::build(
            m.as_view(),
            &b,
            placement.show_cmd(&b)?,
            rebuilt_rect,
            "placement",
        )?;
        b.ret(rebuilt)?;

        format!("{m}")
    };

    print!("{ir}");
    Ok(())
}
