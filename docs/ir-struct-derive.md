# `IrStruct` derive macro

`#[derive(IrStruct)]` maps a plain Rust struct to an LLVM identified struct
schema. It is an ergonomics layer over the existing `StructSchema`,
`StructValue`, `extractvalue`, and `insertvalue` APIs; it does not change the
LLVM IR model.

Use it when the LLVM aggregate you want to construct already has a natural Rust
shape:

```rust
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

fn build() -> Result<String, IrError> {
    Module::with_new("window", |m| {
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
    })
}
```

The emitted IR is ordinary LLVM IR:

```llvm
%Point = type { i32, i32 }
%Rect = type { %Point, %Point }
%WindowPlacement = type { i32, %Rect }

define %WindowPlacement @normalize_window_placement(%WindowPlacement %0) {
entry:
  %normal_position = extractvalue %WindowPlacement %0, 1
  %min = extractvalue %Rect %normal_position, 0
  %max = extractvalue %Rect %normal_position, 1
  %x = extractvalue %Point %min, 0
  %y = extractvalue %Point %max, 1
  %normal_position.min.x = insertvalue %Point poison, i32 %x, 0
  %normal_position.min.y = insertvalue %Point %normal_position.min.x, i32 %y, 1
  %normal_position.min = insertvalue %Rect poison, %Point %normal_position.min.y, 0
  %normal_position.max = insertvalue %Rect %normal_position.min, %Point %max, 1
  %show_cmd = extractvalue %WindowPlacement %0, 0
  %placement.show_cmd = insertvalue %WindowPlacement poison, i32 %show_cmd, 0
  %placement.normal_position = insertvalue %WindowPlacement %placement.show_cmd, %Rect %normal_position.max, 1
  ret %WindowPlacement %placement.normal_position
}
```

## What the macro generates

For `struct Point { x: i32, y: i32 }`, the derive creates:

- `PointValue<'ctx, B>`: a branded wrapper around `StructValue<'ctx, B>`.
- `impl StructSchema for Point`: returns `%Point = type { i32, i32 }` in the
  target module, reusing an existing matching `%Point` body and rejecting a
  mismatched one with `IrError::StructBodyMismatch`.
- Field accessors on `PointValue`, such as `x(&builder)` and `y(&builder)`,
  implemented with `IRBuilder::build_extract_field`.
- `PointValue::build(&module, &builder, x, y, name)`, implemented as a poison
  aggregate plus `insertvalue` steps.
- `PointValue::try_from(raw)` for `StructValue`, `Value`, `Argument`,
  `Constant`, and attached `Instruction` sources. Each conversion validates the
  raw struct against the `Point` schema before returning `PointValue`.
- Conversion impls that let the wrapper be used as a struct field, function
  parameter, and `Dyn` return value.
- An `IntoCallArg` impl, so `PointValue` fills a `Point`-typed parameter slot
  in a typed `IRBuilder::build_call` argument tuple directly.

Generated value wrappers intentionally do not implement `IsValue`. Use the
wrapper's typed accessors/builders, or call `as_struct_value()` / `into_erased()`
when you explicitly want to erase the schema.

## Supported input

`IrStruct` accepts only non-generic structs with named fields:

```rust
#[derive(IrStruct)]
struct Pair {
    a: i32,
    b: i32,
}
```

Rejected shapes:

- tuple structs;
- unit structs;
- enums and unions;
- generic structs or structs with a `where` clause;
- field-level `#[llvmkit(...)]` attributes.

This slice intentionally keeps the Rust struct layout visible. Field rename,
skip, default, and method-generation helpers are not supported because LLVM
struct layout is positional; hiding a Rust field change would obscure an ABI or
IR layout change.

## Helper attributes

Container attributes are intentionally small:

```rust
#[derive(IrStruct)]
#[llvmkit(name = "POINT", packed)]
struct Point {
    x: i32,
    y: i32,
}
```

- `#[llvmkit(name = "...")]` overrides the LLVM identified-struct name.
- `#[llvmkit(packed)]` emits a packed body, e.g. `%POINT = type <{ i32, i32 }>`.
- `#[llvmkit(crate = path::to::ir)]` overrides the generated path to the
  `llvmkit-ir` API. Use this when the derive is expanded from a re-exporting
  crate or unusual workspace layout.

The default crate path resolver supports both direct `llvmkit-ir` users
(`llvmkit_ir::IrStruct`) and umbrella-crate users (`llvmkit::ir::IrStruct`).
The `llvmkit-ir` crate enables the `macros` feature by default; disable default
features only if you want the manual schema traits without the proc macro.

## Nested structs

Nested derived structs remain typed:

```rust
let rect = placement.normal_position(&builder)?; // RectValue<'ctx, B>
let min = rect.min(&builder)?;                   // PointValue<'ctx, B>
let x = min.x(&builder)?;                        // IntValue<'ctx, i32, B>
```

A nested field does not degrade to raw `StructValue`; its schema stays in the
Rust type. This is the main reason the derive exists: field position remains the
LLVM source of truth, while ordinary Rust names make the call site readable.

## Wrapping existing IR

Use the generated `TryFrom` impls when a raw value already exists and should be
validated against a known schema:

```rust
let raw = f.as_function().param(0)?;
let placement = WindowPlacementValue::try_from(raw)?;
let normal_position = placement.normal_position(&builder)?;
```

The conversion accepts `StructValue`, erased `Value`, `Argument`, `Constant`,
and attached `Instruction` handles. The wrapper is returned only after the raw
LLVM type has the expected identified-struct name, packed flag, and field
layout.


## Function signatures

A derived struct can appear directly in typed function facades:

```rust
type NormalizePlacement = fn(WindowPlacement) -> WindowPlacement;
let f = m.add_typed_function_of::<NormalizePlacement, _>("normalize", Linkage::External)?;
let (placement,) = f.params(); // WindowPlacementValue<'ctx, B>
```

The function-pointer alias is parsed at compile time. `fn`, `unsafe fn`,
`extern "C" fn`, `unsafe extern "C" fn`, `extern "system" fn`, and
`unsafe extern "system" fn` aliases are supported up to arity 16.

A bare schema parameter remains one by-value LLVM struct parameter. Use
`StructFields<S>` to opt into one LLVM parameter per top-level field:

```rust
use llvmkit_ir::{Linkage, StructFields};

let f = m.add_typed_function::<WindowPlacement, StructFields<WindowPlacement>, _>(
    "normalize_fields",
    Linkage::External,
)?;
let entry = f.append_basic_block(&m, "entry");
let b = f.builder(&m).position_at_end(entry);
let (show_cmd, normal_position) = f.params();
let rebuilt = WindowPlacementValue::build(&m, &b, show_cmd, normal_position, "rebuilt")?;
```

`StructFields<S>` unpacks only `S`'s top-level fields. Nested structs remain
their generated wrapper values, so `normal_position` above is still
`RectValue<'ctx, B>`, not separate `%Point` fields.

## Error behavior

The macro-generated code uses normal `IrResult` paths:

- empty LLVM struct names and mismatched identified-struct bodies return `IrError`;
- field extraction validates the requested field type before appending an
  instruction;
- nested schemas validate their bodies before returning wrapper values;
- cross-module value mixing is rejected by the module brand in the generated
  wrapper type.

These checks mirror LLVM's structural rules, but surface as Rust errors or
compile errors rather than LLVM assertions or late verifier failures.
