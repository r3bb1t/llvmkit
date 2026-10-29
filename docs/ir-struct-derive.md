# `IrStruct` derive macro

`#[derive(IrStruct)]` maps a plain Rust struct to an LLVM identified struct
schema. It is an ergonomics layer over the existing `StructSchema`,
`StructValue`, `extractvalue`, and `insertvalue` APIs; it does not change the
LLVM IR model.

Use it when the LLVM aggregate you want to construct already has a natural Rust
shape:

```rust
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

fn build() -> Result<String, IrError> {
    let m = module_new!("window")?;
    // `add_typed_function_of` returns a storable `TypedFunctionId`; `m.view(f)`
    // is the ephemeral borrowing facade you call methods on.
    let f = m.add_typed_function_of::<NormalizePlacement, _>(
        "normalize_window_placement",
        Linkage::External,
    )?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (placement,) = m.view(f).params();

    let normal_position = placement.normal_position(&b);
    let min = normal_position.min(&b);
    let max = normal_position.max(&b);
    let min_x = min.x(&b);
    let max_y = max.y(&b);

    // The struct types, each with its schema's body: the one fallible
    // step. A build from one needs no `?`.
    let point = m.get_or_insert_struct_of::<Point>()?;
    let rect = m.get_or_insert_struct_of::<Rect>()?;
    let window = m.get_or_insert_struct_of::<WindowPlacement>()?;

    let rebuilt_min = point.build(&b, min_x, max_y, "normal_position.min");
    let rebuilt_rect = rect.build(&b, rebuilt_min, max, "normal_position");
    let rebuilt = window.build(&b, placement.show_cmd(&b), rebuilt_rect, "placement");
    b.ret(rebuilt)?;

    Ok(format!("{m}"))
}
```

The runnable form of this is `crates/llvmkit-ir/examples/derived_struct_function.rs`.

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

- `PointValue<'ctx, B, C = Mutable>`: a branded wrapper around a
  `SchemaValue<'ctx, Point, B, C>`, at the capability of the struct value it
  wraps. Only llvmkit mints a `SchemaValue` — after checking a raw value
  against `Point`, or as the result of its own typed operations — so a
  `PointValue` is a value of `%Point` by construction. A `PointValue` of
  either capability is an operand — a field, a call argument, a return
  value — and is admitted at the receiving module, which refuses another
  module's with `IrError::ForeignValueId`.
- `impl StructSchema for Point`: the name (`NAME = "Point"`), the packed flag
  and the field tuple (`type Fields = (i32, i32)`), and nothing else. llvmkit
  builds `%Point = type { i32, i32 }` from the fields' layouts
  (`m.get_or_insert_struct_of::<Point>()`), reusing an existing matching
  `%Point` body and rejecting a mismatched one with
  `IrError::StructBodyMismatch`; building and matching come from the same
  layouts, so they cannot disagree.
- Field accessors on `PointValue`, two per field:
  - `x(&builder)` returns the field's value with no `Result`. It needs a
    `UniqueBrand` module (every `module_new!` module is one) and one of
    llvmkit's own folders (`NoFolder`, `ConstantFolder` — a `TrustedFolder`).
    The registry keeps one live module per such brand, so the value is the
    builder's module's; the schema value proves the field's type; and
    llvmkit's folders fold a field read only to a value of that type
    (`ConstantFolder` reads a constant struct's element). So nothing is left
    to check.
  - `try_x(&builder)` returns `IrResult` and works on any brand, through any
    folder (`IrBuilder::extract_field`): a `Module::dynamic` module, whose
    `DynBrand` names many live modules, or a folder of your own, which is
    consulted on every read and whose answer is checked. Calling `x` where
    either requirement fails is a compile error that names the missing bound.
- A `PointBuild` trait, implemented for `StructOf<'ctx, Point, B, C>` — the
  struct type `m.get_or_insert_struct_of::<Point>()` answers once it has
  confirmed or set `%Point`'s body. Its methods build a `PointValue` as a
  poison aggregate plus one `insertvalue` per field, named `name.x`,
  `name.y` (or `x`, `y` for an empty name). The trait is how a method lands on
  a type llvmkit owns; it is in scope wherever `Point` is, and a module that
  builds a `Point` elsewhere imports it (`use crate::shapes::PointBuild;`).
  - `point.build(&builder, x, y, name)` returns the value with no `Result`,
    on the same terms as `x(&builder)`: a `UniqueBrand` module and a
    `TrustedFolder`. Each argument is a `FieldArg` of its field's layout — a
    view handle of the field's own type at either capability (a `PointerValue`
    in address space 0 for a `Ptr` field, a `PointValue` for a `Point` field)
    or, for a scalar field, a Rust literal of that scalar (`7_i32`, `7_u32`
    for a `u32` field, `0.5_f64`), which becomes a constant. A storable id is
    no `FieldArg`: an id outlives its module, and the brand is issued again
    once the module drops. The `StructOf` token is what makes the build
    infallible: it proves `%Point` has `Point`'s body, so no build can meet
    `StructBodyMismatch`.
  - `point.try_build(&builder, x, y, name)` returns `IrResult` and takes
    anything a field admits (`IntoIrField`, ids included) on any brand,
    through any folder: each argument is admitted at the builder's module
    (`IrError::ForeignValueId` for another module's, a stale id included) and
    checked against its field (`IrBuilder::insert_field`).
- `PointValue::try_from(raw)` for `StructValue`, `Value`, `Argument`,
  `Constant`, and attached `Instruction` sources. Each conversion validates the
  raw struct against the `Point` schema (`SchemaValue::try_new`) before
  returning `PointValue`.
- Conversion impls that let the wrapper be used as a struct field, function
  parameter, and `Dyn` return value.
- An `IntoCallArg` impl, so `PointValue` fills a `Point`-typed parameter slot
  in a typed `IrBuilder::call` argument tuple directly.

Generated value wrappers intentionally do not implement `IsValue`. Use the
wrapper's typed accessors/builders, or call `as_struct_value()` / `as_erased()`
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

The derive intentionally keeps the Rust struct layout visible. Field rename,
skip, default, and method-generation helpers are not supported because LLVM
struct layout is positional; hiding a Rust field change would obscure an ABI or
IR layout change.

## Field kinds

Each field's type is an `IrField`, and each names a `FieldLayout` — one of a
sealed set of valid struct elements whose type llvmkit builds itself, in the
module that asks:

| Rust field | LLVM element | field read returns |
|---|---|---|
| `bool`, `i8` … `i128` | `i1`, `i8` … `i128` | `IntValue` of that marker |
| `u8` … `u128` | `i8` … `i128` (LLVM integers carry no sign) | `IntValue<i8>` … `IntValue<i128>` |
| `Width<N>` | `iN` | `IntValue<Width<N>>` |
| `f32`, `f64`, `Half`, `Bfloat`, `Fp128`, `X86Fp80`, `PpcFp128` | the float type | `FloatValue` of that kind |
| `Ptr`, `Ptr<AS>` | `ptr`, `ptr addrspace(AS)` | `PointerValue<Space<AS>>` |
| `[T; N]`, `T` a kind laid out as `bool`, `i8` … `i128` or a float kind | `[N x T's element]` | `ArrayValue<T's layout, ArrLen<N>>` |
| another derived (or hand-written) schema | its named struct | its value type |

So no field can be laid out as an invalid element (`label`, `void`, …) or as
another module's type, and a mistake is a compile error at the field.

A kind of your own names a layout and converts its values:

```rust
use llvmkit_ir::{Capability, FloatValue, IrField, IrStruct, LayoutValue, ModuleBrand};

/// A distance, laid out as a `double`.
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
    distance: Meters, // %Run = type { double }
}
```

A field takes, as a build argument, whatever its layout takes — a `Meters`
field takes an `f64` value — so a kind of your own needs no conversion impl.

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
let rect = placement.normal_position(&builder); // RectValue<'ctx, B>
let min = rect.min(&builder);                   // PointValue<'ctx, B>
let x = min.x(&builder);                        // IntValue<'ctx, i32, B>
```

A nested field does not degrade to raw `StructValue`; its schema stays in the
Rust type. This is the main reason the derive exists: field position remains the
LLVM source of truth, while ordinary Rust names make the call site readable.

## Wrapping existing IR

Use the generated `TryFrom` impls when a raw value already exists and should be
validated against a known schema:

```rust
let raw = m.view(f).as_function().param(0)?;
let placement = WindowPlacementValue::try_from(raw)?;
let normal_position = placement.normal_position(&builder);
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
let (placement,) = m.view(f).params(); // WindowPlacementValue<'ctx, B>
```

The function-pointer alias is parsed at compile time. `fn`, `unsafe fn`,
`extern "C" fn`, `unsafe extern "C" fn`, `extern "system" fn`, and
`unsafe extern "system" fn` aliases are supported up to arity 16 (the widest
`impl_function_signature!` and `impl_param_list_tuple!` invocations in
`crates/llvmkit-ir/src/function_signature.rs` take sixteen parameters; derived
with `rg "^impl_(function_signature|param_list_tuple)!" crates/llvmkit-ir/src/function_signature.rs`
at `7d0540c`).

A bare schema parameter remains one by-value LLVM struct parameter. Use
`StructFields<S>` to opt into one LLVM parameter per top-level field:

```rust
use llvmkit_ir::{Linkage, StructFields};

let f = m.add_typed_function::<WindowPlacement, StructFields<WindowPlacement>, _>(
    "normalize_fields",
    Linkage::External,
)?;
let entry = m.view(f).append_basic_block(&m, "entry");
let b = m.view(f).builder(&m).position_at_end(entry);
let (show_cmd, normal_position) = m.view(f).params();
let rebuilt = m
    .get_or_insert_struct_of::<WindowPlacement>()?
    .build(&b, show_cmd, normal_position, "rebuilt");
```

`StructFields<S>` unpacks only `S`'s top-level fields. Nested structs remain
their generated wrapper values, so `normal_position` above is still
`RectValue<'ctx, B>` (at `Mutable`), not separate `%Point` fields. It needs
every field kind to be a function parameter as well: unsigned integers, arrays
and kinds of your own are struct fields but not yet typed parameters, so a
struct with one is passed whole.

## Error behavior

The macro-generated code uses normal `IrResult` paths:

- an empty LLVM struct name is the derive's own compile error
  (`#[llvmkit(name = "")]`), and a hand-written schema's empty `NAME` fails to
  compile where its type is minted; a mismatched identified-struct body
  returns `IrError::StructBodyMismatch`;
- field extraction validates the requested field type before appending an
  instruction;
- nested schemas validate their bodies before returning wrapper values;
- cross-module value mixing is rejected by the module brand in the generated
  wrapper type.

These checks mirror LLVM's structural rules, but surface as Rust errors or
compile errors rather than LLVM assertions or late verifier failures.
