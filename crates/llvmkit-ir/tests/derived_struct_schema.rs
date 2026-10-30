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
    let adjusted_min =
        m.get_or_insert_struct_of::<Point>()?
            .build(&b, x, max.y(&b), "adjusted_min");
    let adjusted_rect =
        m.get_or_insert_struct_of::<Rect>()?
            .build(&b, adjusted_min, max, "adjusted_rect");
    let rebuilt = m.get_or_insert_struct_of::<WindowPlacement>()?.build(
        &b,
        placement.show_cmd(&b),
        adjusted_rect,
        "rebuilt",
    );
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
    let rebuilt = m.get_or_insert_struct_of::<WindowPlacement>()?.build(
        &b,
        show_cmd,
        normal_position,
        "rebuilt",
    );
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
    let value =
        m.get_or_insert_struct_of::<CollisionNames>()?
            .build(&b, 1_i32, 2_i32, 3_i32, "collision");
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

/// llvmkit-specific (D6, D10), no upstream counterpart — a C++ `StructType`
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

/// llvmkit-specific (D6, D10), no upstream counterpart: a field read checks
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

/// The module `PointValue::build(m.as_view(), &b, x, 7_i32, "p")?` printed
/// before builds moved to the `StructOf` token — captured from that build, so
/// the token's build is compared with the output it replaces.
const TOKEN_BUILD_EXPECTED: &str = "; ModuleID = 'token_build'

%Point = type { i32, i32 }

define %Point @make(i32 %0) {
entry:
  %p.x = insertvalue %Point poison, i32 %0, 0
  %p.y = insertvalue %Point %p.x, i32 7, 1
  ret %Point %p.y
}
";

/// llvmkit-specific (D6, D10), no upstream counterpart —
/// `IRBuilderBase::CreateInsertValue` takes any `Value *` and checks nothing:
/// once `get_or_insert_struct_of` has answered — the one fallible step, where
/// `%Point`'s body is confirmed or set — a build on a `UniqueBrand` module
/// with llvmkit's own folder needs no `?`, and prints the `insertvalue` chain
/// the old fallible build printed.
#[test]
fn a_build_needs_no_question_mark() -> Result<(), IrError> {
    let m = module_new!("token_build")?;
    let f = m.add_typed_function::<Point, (i32,), _>("make", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (x,) = m.view(f).params();
    let point = m.get_or_insert_struct_of::<Point>()?;
    let p = point.build(&b, x, 7_i32, "p");
    b.ret(p)?;
    assert_eq!(format!("{m}"), TOKEN_BUILD_EXPECTED);
    Ok(())
}

/// A struct with one pointer field, in address space 0.
#[derive(IrStruct)]
struct Node {
    next: Ptr,
}

/// llvmkit-specific (D3, D10), no upstream counterpart: a `Ptr` field's build
/// argument is an address-space-0 pointer by type — here the `alloca_in::<0>`
/// result — so no build can store a `ptr addrspace(1)` in it.
/// `compile_fail/build_with_dyn_pointer.rs` is the refused half.
#[test]
fn a_pointer_field_takes_a_space_zero_pointer() -> Result<(), IrError> {
    let Node { next } = Node { next: Ptr::null() };
    let _ = next;

    let m = module_new!("pointer_build")?;
    let f = m.add_typed_function::<Node, (), _>("make", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let slot = m.view(b.alloca_in::<0, _, _>(m.i32_type(), "slot")?);
    let node = m.get_or_insert_struct_of::<Node>()?;
    let n = node.build(&b, slot, "n");
    b.ret(n)?;
    let text = format!("{m}");
    assert!(
        text.contains("%n.next = insertvalue %Node poison, ptr %slot, 0"),
        "got:\n{text}"
    );
    Ok(())
}

/// A schema named as a generated generic parameter's old spelling: `B`, `F`,
/// `R`, `Name` and `Field0`. Each is the user's own type, and a field of that
/// type must not be read as the derive's parameter.
#[derive(IrStruct)]
struct B {
    b: i32,
}
#[derive(IrStruct)]
struct F {
    f: i32,
}
#[derive(IrStruct)]
struct R {
    r: i32,
}
#[derive(IrStruct)]
struct Name {
    name: i32,
}
#[derive(IrStruct)]
struct Field0 {
    field: i32,
}

/// The review's own examples: `A { b: B }` and `Person { name: Name }`.
#[derive(IrStruct)]
struct A {
    b: B,
}
#[derive(IrStruct)]
struct Person {
    name: Name,
    first: Field0,
    folder: F,
    ret: R,
}

/// llvmkit-specific, no upstream counterpart (LLVM has no derive): a field's
/// type is the user's own whatever it is spelled, so a struct called `B`, `F`,
/// `R`, `Name` or `Field0` works as a field. The derive's generic parameters
/// shadowed each of them, and the user got an unrelated error about the derive.
#[test]
fn a_field_type_is_not_shadowed_by_the_derive() -> Result<(), IrError> {
    let _ = A { b: B { b: 1 } }.b.b;
    let Person {
        name,
        first,
        folder,
        ret,
    } = Person {
        name: Name { name: 1 },
        first: Field0 { field: 2 },
        folder: F { f: 3 },
        ret: R { r: 4 },
    };
    let _ = name.name + first.field + folder.f + ret.r;

    let m = module_new!("shadowed")?;
    let f = m.add_typed_function::<(), (A, Person), _>("take", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (a, person) = m.view(f).params();
    let inner: BValue<'_, _> = a.b(&b);
    let _ = inner.b(&b);
    let name: NameValue<'_, _> = person.try_name(&b)?;
    let _ = name.name(&b);
    let rebuilt = m
        .get_or_insert_struct_of::<A>()?
        .try_build(&b, inner, "again")?;
    let _ = rebuilt;
    b.ret_void();
    let text = format!("{m}");
    assert!(text.contains("%A = type { %B }"), "{text}");
    assert!(
        text.contains("%Person = type { %Name, %Field0, %F, %R }"),
        "{text}"
    );
    Ok(())
}

/// A field whose Rust spelling is a keyword, and a struct written raw.
#[derive(IrStruct)]
struct r#Raw {
    r#type: i32,
}

/// llvmkit-specific, no upstream counterpart: `r#` is Rust's, not the IR's. A
/// raw field names its instruction as `try_type` names its accessor, and a raw
/// struct its IR type. Both kept the prefix: `%"r#type"` and `%"r#Raw"`.
#[test]
fn a_raw_identifier_names_nothing_in_the_ir() -> Result<(), IrError> {
    let Raw { r#type } = Raw { r#type: 1 };
    let _ = r#type;

    let m = module_new!("raw_names")?;
    let f = m.add_typed_function::<(), (Raw,), _>("take", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (value,) = m.view(f).params();
    let _ = value.r#type(&b);
    let _ = value.try_type(&b)?;
    let token = m.get_or_insert_struct_of::<Raw>()?;
    let _ = token.build(&b, 1_i32, "built");
    b.ret_void();
    let text = format!("{m}");
    assert!(text.contains("%Raw = type { i32 }"), "{text}");
    assert!(text.contains("%type = extractvalue %Raw %0, 0"), "{text}");
    assert!(
        text.contains("%built.type = insertvalue %Raw poison, i32 1, 0"),
        "{text}"
    );
    assert!(!text.contains("r#"), "{text}");
    Ok(())
}

/// llvmkit-specific (D3, D10), no upstream counterpart: spec §2 says
/// `PointerValue::try_from` is the one door out of `SpaceDyn`, so a `SpaceDyn`
/// pointer goes through it, and the `Space<0>` pointer it answers with goes to
/// `try_build`. `compile_fail/build_with_dyn_pointer.rs` and the `FieldArg`
/// diagnostic send a user here; the pointer in another space is the negative
/// half.
#[test]
fn a_narrowed_space_dyn_pointer_goes_through_try_build() -> Result<(), IrError> {
    let m = module_new!("narrowed_pointer")?;
    let f = m.add_typed_function::<Node, (), _>("make", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let slot = m.view(b.alloca(m.i32_type(), "slot")?);
    let narrowed = PointerValue::<Space<0>, _>::try_from(slot)?;
    let node = m.get_or_insert_struct_of::<Node>()?;
    let n = node.try_build(&b, narrowed, "n")?;

    let far = m
        .view(b.alloca_in::<1, _, _>(m.i32_type(), "far")?)
        .as_dyn();
    assert_eq!(
        PointerValue::<Space<0>, _>::try_from(far).map(|_| ()),
        Err(IrError::AddressSpaceMismatch {
            expected: 0,
            got: 1,
        })
    );

    b.ret(n)?;
    let text = format!("{m}");
    assert!(
        text.contains("%n.next = insertvalue %Node poison, ptr %slot, 0"),
        "got:\n{text}"
    );
    Ok(())
}

/// llvmkit-specific (D6), no upstream counterpart: a nested field's build
/// argument is the nested schema's own value — two `Point`s built on their
/// token become a `Rect`'s fields.
#[test]
fn a_nested_field_takes_a_derived_value() -> Result<(), IrError> {
    let m = module_new!("nested_build")?;
    let f = m.add_typed_function::<Rect, (i32,), _>("make", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let (x,) = m.view(f).params();
    let point = m.get_or_insert_struct_of::<Point>()?;
    let rect = m.get_or_insert_struct_of::<Rect>()?;
    let lo = point.build(&b, x, 1_i32, "lo");
    let hi = point.build(&b, 2_i32, x, "hi");
    let r = rect.build(&b, lo, hi, "r");
    b.ret(r)?;
    let text = format!("{m}");
    for line in [
        "%r.min = insertvalue %Rect poison, %Point %lo.y, 0",
        "%r.max = insertvalue %Rect %r.min, %Point %hi.y, 1",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
    Ok(())
}

/// Review Focus 5 of the type-constructor validity plan (llvmkit-specific, D7;
/// LLVM's `Module` has no brand): a storable id outlives its module, and a
/// named brand is issued again once that module drops, so an id from the dead
/// module type-checks against its successor. `try_build` admits each argument
/// against the builder's module, so the stale id is refused with
/// `ForeignValueId` — never resolved in the new module. (`build` takes no ids
/// at all: `compile_fail/build_with_id.rs`.)
#[test]
fn a_stale_id_is_refused_by_try_build() -> Result<(), IrError> {
    struct Generation;
    impl ModuleBrand for Generation {}

    let stale = {
        let first = Module::branded::<Generation, _>("first")?;
        let f = first.add_typed_function::<(), (i32,), _>("f", Linkage::External)?;
        let (x,) = first.view(f).params();
        x.id()
        // `first` drops here, releasing the brand; the id survives it.
    };

    let second = Module::branded::<Generation, _>("second")?;
    let f = second.add_typed_function::<Point, (), _>("make", Linkage::External)?;
    let entry = second.view(f).append_basic_block(&second, "entry");
    let b = IrBuilder::with_folder(&second, NoFolder).position_at_end(entry);
    let point = second.get_or_insert_struct_of::<Point>()?;
    assert_eq!(
        point.try_build(&b, stale, 7_i32, "p").map(|_| ()),
        Err(IrError::ForeignValueId)
    );
    // The same build with a live value of this module succeeds.
    let live = point.try_build(&b, 1_i32, 7_i32, "p")?;
    b.ret(live)?;
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
        .struct_type()
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

/// Mirrors `ConstantFolder::FoldInsertValue`, which hands a constant aggregate
/// and a constant value to `ConstantFoldInsertValueInstruction`
/// (`lib/IR/ConstantFold.cpp`) and gets back the rebuilt aggregate: built
/// through `ConstantFolder` from literals, a `Point` is the constant
/// `%Point { i32 1, i32 2 }` — `poison` with each field replaced in turn — and
/// no instruction is appended. `a_constant_struct_field_read_folds` is the
/// read half.
#[test]
fn a_constant_struct_build_folds() -> Result<(), IrError> {
    let m = module_new!("folded_build")?;
    let i32_ty = m.i32_type();
    let point = m.get_or_insert_struct_of::<Point>()?;
    let expected = point
        .struct_type()
        .const_struct([i32_ty.const_int(1_i32), i32_ty.const_int(2_i32)])?;
    let f = m.add_typed_function::<Point, (), _>("one_two", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, ConstantFolder).position_at_end(entry);
    let p = point.build(&b, 1_i32, 2_i32, "p");
    assert_eq!(p.as_erased(), expected.as_erased());
    assert_eq!(b.insert_block().instructions().len(), 0);
    b.ret(p)?;
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

/// llvmkit-specific (D6), no upstream counterpart: a field kind the user
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

/// llvmkit-specific (D10), no upstream counterpart: `StructType::setName`
/// renames anything, and LLVM has no schemas. A `StructOf` proves its struct is
/// `%Point`, so `Module::set_struct_name` refuses it, and through
/// `ConstantFolder` the rebuild of a `%Rect` constant from it neither reaches
/// `fold_field_insert`'s `unreachable!` nor leaves a second `%Point`.
#[test]
fn renaming_a_schema_struct_is_refused_with_constant_folder() -> Result<(), IrError> {
    let m = module_new!("rename_constant")?;
    let f = m.add_typed_function::<(), (), _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, ConstantFolder).position_at_end(entry);
    let point = m.get_or_insert_struct_of::<Point>()?;
    assert_eq!(
        m.set_struct_name(point.struct_type(), "Old"),
        Err(IrError::SchemaStructNameNotSettable {
            name: "Point".into(),
        })
    );
    let rect = m.get_or_insert_struct_of::<Rect>()?;
    let p = point.build(&b, 1_i32, 2_i32, "p");
    let r = rect.build(&b, p, p, "r");
    let _ = r;
    b.ret_void();
    let text = format!("{m}");
    assert_eq!(text.matches("%Point = type").count(), 1, "got:\n{text}");
    assert!(m.verify().is_ok());
    Ok(())
}

/// llvmkit-specific (D10), no upstream counterpart: the `NoFolder` half of
/// `renaming_a_schema_struct_is_refused_with_constant_folder`, where a rename
/// the module allowed printed `insertvalue %Rect poison, %Old %p.y, 0` and only
/// `verify` caught it.
#[test]
fn renaming_a_schema_struct_is_refused_with_no_folder() -> Result<(), IrError> {
    let m = module_new!("rename_no_folder")?;
    let f = m.add_typed_function::<(), (), _>("f", Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let point = m.get_or_insert_struct_of::<Point>()?;
    assert_eq!(
        m.set_struct_name(point.struct_type(), "Old"),
        Err(IrError::SchemaStructNameNotSettable {
            name: "Point".into(),
        })
    );
    let rect = m.get_or_insert_struct_of::<Rect>()?;
    let p = point.build(&b, 1_i32, 2_i32, "p");
    let r = rect.build(&b, p, p, "r");
    let _ = r;
    b.ret_void();
    let text = format!("{m}");
    assert!(
        text.contains("%r.min = insertvalue %Rect poison, %Point %p.y, 0"),
        "got:\n{text}"
    );
    assert!(m.verify().is_ok());
    Ok(())
}

/// llvmkit-specific (D10), no upstream counterpart: a `%Point` minted only as a
/// `Rect` field is held too, because the `Rect` token's build takes a value of
/// exactly that struct.
#[test]
fn a_nested_schema_struct_keeps_its_name() -> Result<(), IrError> {
    let m = module_new!("rename_nested")?;
    let _ = m.get_or_insert_struct_of::<Rect>()?;
    let point = m.named_struct("Point").expect("the nested mint created it");
    assert_eq!(
        m.set_struct_name(point, "Old"),
        Err(IrError::SchemaStructNameNotSettable {
            name: "Point".into(),
        })
    );
    Ok(())
}

/// llvmkit-specific (D10), no upstream counterpart: an existing `%Rect` of the
/// right body is confirmed, not minted, and the confirmation holds both it and
/// the nested `%Point`.
#[test]
fn a_confirmed_struct_keeps_its_name_and_so_do_its_fields() -> Result<(), IrError> {
    let m = module_new!("rename_confirmed")?;
    let i32_ty = m.i32_type();
    let point = m.get_or_insert_named_struct("Point");
    m.set_struct_body_dyn(point, [i32_ty, i32_ty], false)?;
    let rect = m.get_or_insert_named_struct("Rect");
    m.set_struct_body_dyn(rect, [point.as_type(), point.as_type()], false)?;
    let _ = m.get_or_insert_struct_of::<Rect>()?;
    for (held, name) in [(point, "Point"), (rect, "Rect")] {
        assert_eq!(
            m.set_struct_name(held, "Old"),
            Err(IrError::SchemaStructNameNotSettable { name: name.into() })
        );
    }
    Ok(())
}

/// llvmkit-specific (D10), no upstream counterpart: a `SchemaValue` needs no
/// mint. `PointValue::try_from` checks a hand-built `%Point` and proves the
/// value is one, so the check holds the struct as a mint would; a rename the
/// module allowed here gave `rect.build` an `%Old` for a `%Point` field.
#[test]
fn a_struct_a_schema_value_checked_keeps_its_name() -> Result<(), IrError> {
    let m = module_new!("rename_checked")?;
    let i32_ty = m.i32_type();
    let hand = m.get_or_insert_named_struct("Point");
    m.set_struct_body_dyn(hand, [i32_ty, i32_ty], false)?;
    let fn_ty = m.function_type(m.void_type(), [hand.as_type()])?;
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let p = PointValue::try_from(m.view(f).param(0)?)?;
    assert_eq!(
        m.set_struct_name(hand, "Old"),
        Err(IrError::SchemaStructNameNotSettable {
            name: "Point".into(),
        })
    );
    let rect = m.get_or_insert_struct_of::<Rect>()?;
    let r = rect.build(&b, p, p, "r");
    let _ = r;
    b.ret_void()?;
    assert!(m.verify().is_ok());
    Ok(())
}

/// llvmkit-specific (D8, D10), no upstream counterpart: a read-only check holds
/// its struct, because its proof does not stay read-only. A `ReadOnly`
/// `PointValue` is a `FieldArg` at any capability, so `rect.build` on a mutable
/// builder takes it; and a typed function id carries no capability, so a check on
/// a verified module reaches a mutable view through `unverify`. Each route would
/// write an `%Old` into a `%Point` field if the rename went through.
#[test]
fn a_read_only_proof_reaches_a_build_so_its_check_holds() -> Result<(), IrError> {
    let m = module_new!("rename_read_only")?;
    let i32_ty = m.i32_type();
    let hand = m.get_or_insert_named_struct("Point");
    m.set_struct_body_dyn(hand, [i32_ty, i32_ty], false)?;
    let fn_ty = m.function_type(m.void_type(), [hand.as_type()])?;
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);

    let p = PointValue::try_from(m.as_view().view(f).param(0)?)?;
    assert_eq!(
        m.set_struct_name(hand, "Old"),
        Err(IrError::SchemaStructNameNotSettable {
            name: "Point".into(),
        })
    );
    let rect = m.get_or_insert_struct_of::<Rect>()?;
    let r = rect.build(&b, p, p, "r");
    let _ = r;
    b.ret_void()?;
    assert!(m.verify_borrowed().is_ok());

    let second = module_new!("rename_verified")?;
    let i32_ty = second.i32_type();
    let hand = second.get_or_insert_named_struct("Point");
    second.set_struct_body_dyn(hand, [i32_ty, i32_ty], false)?;
    let fn_ty = second.function_type(second.void_type(), [hand.as_type()])?;
    second.add_function_dyn("g", fn_ty, Linkage::External)?;
    let g = second.function::<()>("g")?.expect("declared above");
    let verified = second.verify()?;
    let typed = verified.view(g).with_typed_signature::<fn(Point)>()?;
    let second = verified.unverify();
    let hand = second.named_struct("Point").expect("defined above");
    assert_eq!(
        second.set_struct_name(hand, "Old"),
        Err(IrError::SchemaStructNameNotSettable {
            name: "Point".into(),
        })
    );
    let (point,) = second.view(typed).params();
    let _: PointValue<'_, _> = point;
    Ok(())
}

/// llvmkit-specific (D10), no upstream counterpart: a typed signature that
/// names a schema proves the function's return and parameter are `%Point`, by
/// the same check as `a_struct_a_schema_value_checked_keeps_its_name`, so it
/// holds the struct too.
#[test]
fn a_struct_a_typed_signature_checked_keeps_its_name() -> Result<(), IrError> {
    let m = module_new!("rename_signature")?;
    let i32_ty = m.i32_type();
    let hand = m.get_or_insert_named_struct("Point");
    m.set_struct_body_dyn(hand, [i32_ty, i32_ty], false)?;
    let fn_ty = m.function_type(hand.as_type(), [hand.as_type()])?;
    let f = m.add_function_dyn("echo", fn_ty, Linkage::External)?;
    let _ = m.view(f).with_typed_signature::<fn(Point) -> Point>()?;
    assert_eq!(
        m.set_struct_name(hand, "Old"),
        Err(IrError::SchemaStructNameNotSettable {
            name: "Point".into(),
        })
    );
    Ok(())
}

/// llvmkit-specific positive control, no upstream counterpart: only a struct a
/// schema holds is refused. `StructType::setName`'s behaviour stands for a
/// struct no schema touched and for one whose body a schema refused.
#[test]
fn a_struct_no_schema_holds_can_be_renamed() -> Result<(), IrError> {
    let m = module_new!("rename_free")?;
    let free = m.get_or_insert_named_struct("Free");
    m.set_struct_name(free, "Renamed")?;
    assert_eq!(free.name(), Some("Renamed"));

    let point = m.get_or_insert_named_struct("Point");
    m.set_struct_body_dyn(point, [m.i64_type()], false)?;
    assert!(m.get_or_insert_struct_of::<Point>().is_err());
    m.set_struct_name(point, "Old")?;
    assert_eq!(point.name(), Some("Old"));
    Ok(())
}
