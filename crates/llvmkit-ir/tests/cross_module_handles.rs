//! A handle minted by one module is rejected when another module would store
//! its slot.
//!
//! Two `Module::dynamic` modules share the `DynBrand` brand, so a handle from
//! one type-checks as an argument to the other's APIs. Every value, constant
//! and type handle carries only an arena slot plus its owning module, and each
//! module owns its own arenas: a slot stored into the wrong module names a
//! different value or type there, or nothing. The module tag is the backstop
//! (D7), and each test here pins one family of entry points that must check it
//! before anything is looked up, stored or linked into a use list.
//!
//! **llvmkit-specific (D7).** Upstream has no arena slot to mistake: a
//! `Value *` or `Type *` is its own identity. The nearest upstream analogue is
//! the verifier's module-provenance check on *globals*, which rejects an
//! already-built reference rather than refusing to build it:
//! `lib/IR/Verifier.cpp::Verifier::visitGlobalValue` ("Global is referenced in
//! a different module!") and `Verifier::visitInstruction` ("Referencing global
//! in another module!"). For types there is none at all: upstream uniques types
//! per `LLVMContext`, so every module of one context shares them, while
//! llvmkit gives each module its own type arena.

use llvmkit_ir::{
    Align, BasicBlock, CastOpcode, Dyn, DynBrand, FloatDyn, FloatValue, GepNoWrapFlags,
    IntCastFlags, IntDyn, IntValue, IrBuilder, IrError, IrStruct, Linkage, Module, PointerValue,
    Positioned, TruncFlags, UiToFpFlags, Unterminated, Value, ZextFlags,
};

/// A two-field schema, so a struct-typed value exists to hand across modules.
#[derive(IrStruct)]
struct Pair {
    first: i32,
    second: i32,
}

/// `i32 f()` in `module`, with an empty block named `name` to build into.
fn open_block<'m>(
    module: &'m Module<DynBrand>,
    name: &str,
) -> BasicBlock<'m, Dyn, Unterminated, DynBrand> {
    let fn_ty = module.function_type_no_parameters(module.i32_type());
    let f = module
        .add_function_dyn(name, fn_ty, Linkage::External)
        .expect("function");
    module.view(f).append_basic_block(module, "entry")
}

/// A builder positioned at the end of a fresh block of `module`.
fn builder<'m>(
    module: &'m Module<DynBrand>,
    name: &str,
) -> IrBuilder<'m, 'm, DynBrand, llvmkit_ir::ConstantFolder, Positioned, Dyn> {
    IrBuilder::new_for::<Dyn>(module).position_at_end(open_block(module, name))
}

/// A builder positioned in a fresh block of `i32 f(i32, i64, float, double,
/// ptr)` in `module`, with those five parameters as erased values.
///
/// No folder can fold a parameter, so an entry that fails to refuse a foreign
/// type reaches its append and returns `Ok` — the refusal under test is the
/// entry's own, never a fold's.
fn builder_with_parameters<'m>(
    module: &'m Module<DynBrand>,
    name: &str,
) -> (
    IrBuilder<'m, 'm, DynBrand, llvmkit_ir::ConstantFolder, Positioned, Dyn>,
    [Value<'m, DynBrand>; 5],
) {
    let fn_ty = module.function_type(
        module.i32_type(),
        [
            module.i32_type().as_type(),
            module.i64_type().as_type(),
            module.f32_type().as_type(),
            module.f64_type().as_type(),
            module.ptr_type(0).as_type(),
        ],
    );
    let f = module
        .add_function_dyn(name, fn_ty, Linkage::External)
        .expect("function");
    let function = module.view(f);
    let parameter = |index: u32| function.param(index).expect("parameter").as_erased();
    let parameters = [
        parameter(0),
        parameter(1),
        parameter(2),
        parameter(3),
        parameter(4),
    ];
    let block = function.append_basic_block(module, "entry");
    (
        IrBuilder::new_for::<Dyn>(module).position_at_end(block),
        parameters,
    )
}

/// The labels of the outcomes that are not a `ForeignType` refusal, so one
/// assertion names every entry that let a foreign type through.
fn not_refused(outcomes: Vec<(&'static str, Result<(), IrError>)>) -> Vec<String> {
    outcomes
        .into_iter()
        .filter(|(_, outcome)| !matches!(outcome, Err(IrError::ForeignType)))
        .map(|(label, outcome)| format!("{label}: {outcome:?}"))
        .collect()
}

/// A typed value view from another `DynBrand` module is rejected at a typed
/// builder operand (`IntoIntValue`), exactly as an id from another module
/// already is. The operand here is a `ConstantIntValue`.
///
/// No upstream counterpart: `IRBuilderBase::CreateAdd` (`IR/IRBuilder.h`)
/// takes `Value *` operands.
#[test]
fn a_builder_rejects_a_value_view_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let foreign_constant = foreign.i32_type().const_int(7i32);
    let b = builder(&home, "f");

    let before = format!("{home}");
    let result = b.int_add(foreign_constant, home.i32_type().const_int(1i32), "sum");
    assert!(
        matches!(result, Err(IrError::ForeignValueId)),
        "a foreign view reached the builder: {result:?}"
    );
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected operand must not mutate"
    );
}

/// A value view from another module is rejected at an erased-by-design
/// builder operand (`IntoErasedValue`), the sibling path of the typed one.
///
/// No upstream counterpart: `IRBuilderBase::CreateFreeze` (`IR/IRBuilder.h`)
/// takes a `Value *`.
#[test]
fn a_builder_rejects_an_erased_value_view_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let foreign_constant = foreign.i32_type().const_int(7i32);
    let b = builder(&home, "f");

    let before = format!("{home}");
    let result = b.freeze(foreign_constant, "frozen");
    assert!(
        matches!(result, Err(IrError::ForeignValueId)),
        "a foreign view reached the builder: {result:?}"
    );
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected operand must not mutate"
    );
}

/// The identity arm of `IntoIntValue`: an `IntValue` view from another module
/// is refused, not only a `ConstantIntValue`.
///
/// No upstream counterpart: `IRBuilderBase::CreateAdd` takes `Value *`
/// operands.
#[test]
fn an_int_operand_rejects_an_int_value_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let foreign_value: IntValue<'_, i32, DynBrand> = foreign
        .i32_type()
        .const_int(7i32)
        .as_erased()
        .try_into()
        .expect("an i32 value");
    let b = builder(&home, "f");

    let before = format!("{home}");
    let result = b.int_add(foreign_value, 1i32, "sum");
    assert!(matches!(result, Err(IrError::ForeignValueId)), "{result:?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected operand must not mutate"
    );
}

/// Both handle arms of `IntoFloatValue` — a `ConstantFloatValue` and a
/// `FloatValue` view — refuse a value from another module.
///
/// No upstream counterpart: `IRBuilderBase::CreateFAdd` (`IR/IRBuilder.h`)
/// takes `Value *` operands.
#[test]
fn a_float_operand_rejects_a_value_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let foreign_constant = foreign.f32_type().const_float(1.0);
    let foreign_value: FloatValue<'_, f32, DynBrand> = foreign_constant
        .as_erased()
        .try_into()
        .expect("an f32 value");
    let b = builder(&home, "f");

    let before = format!("{home}");
    let from_constant = b.fp_add(foreign_constant, 2.0f32, "sum");
    assert!(
        matches!(from_constant, Err(IrError::ForeignValueId)),
        "{from_constant:?}"
    );
    let from_value = b.fp_add(foreign_value, 2.0f32, "sum");
    assert!(
        matches!(from_value, Err(IrError::ForeignValueId)),
        "{from_value:?}"
    );
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected operand must not mutate"
    );
}

/// Both handle arms of `IntoPointerValue` — a `ConstantPointerNull` and a
/// `PointerValue` view — refuse a value from another module.
///
/// No upstream counterpart: `IRBuilderBase::CreateIsNull` (`IR/IRBuilder.h`)
/// takes a `Value *`.
#[test]
fn a_pointer_operand_rejects_a_value_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let foreign_null = foreign.ptr_type(0).const_null();
    let foreign_pointer: PointerValue<'_, DynBrand> = foreign_null
        .as_erased()
        .try_into()
        .expect("a pointer value");
    let b = builder(&home, "f");

    let before = format!("{home}");
    let from_null = b.is_null(foreign_null, "isnull");
    assert!(
        matches!(from_null, Err(IrError::ForeignValueId)),
        "{from_null:?}"
    );
    let from_pointer = b.is_null(foreign_pointer, "isnull");
    assert!(
        matches!(from_pointer, Err(IrError::ForeignValueId)),
        "{from_pointer:?}"
    );
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected operand must not mutate"
    );
}

/// Each handle arm of `SelectArm` — int, float and pointer — refuses a value
/// from another module.
///
/// No upstream counterpart: `IRBuilderBase::CreateSelect` (`IR/IRBuilder.h`)
/// takes `Value *` arms.
#[test]
fn a_select_arm_rejects_a_value_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let int_arm: IntValue<'_, i32, DynBrand> = foreign
        .i32_type()
        .const_int(1i32)
        .as_erased()
        .try_into()
        .expect("an i32 value");
    let float_arm: FloatValue<'_, f32, DynBrand> = foreign
        .f32_type()
        .const_float(1.0)
        .as_erased()
        .try_into()
        .expect("an f32 value");
    let pointer_arm: PointerValue<'_, DynBrand> = foreign
        .ptr_type(0)
        .const_null()
        .as_erased()
        .try_into()
        .expect("a pointer value");
    let b = builder(&home, "f");
    let condition = home.bool_type().const_int(true);

    let before = format!("{home}");
    let int_result = b.select(condition, int_arm, int_arm, "s");
    assert!(
        matches!(int_result, Err(IrError::ForeignValueId)),
        "{int_result:?}"
    );
    let float_result = b.select(condition, float_arm, float_arm, "s");
    assert!(
        matches!(float_result, Err(IrError::ForeignValueId)),
        "{float_result:?}"
    );
    let pointer_result = b.select(condition, pointer_arm, pointer_arm, "s");
    assert!(
        matches!(pointer_result, Err(IrError::ForeignValueId)),
        "{pointer_result:?}"
    );
    assert_eq!(format!("{home}"), before, "a rejected arm must not mutate");
}

/// `IntoCallee` refuses a `FunctionValue` from another module.
///
/// No upstream counterpart: `IRBuilderBase::CreateCall` (`IR/IRBuilder.h`)
/// takes a `FunctionCallee`, a `Value *` pair; the verifier's
/// `Verifier::visitInstruction` "Referencing function in another module!"
/// is the nearest check, after the fact.
#[test]
fn a_callee_rejects_a_function_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let foreign_ty = foreign.function_type_no_parameters(foreign.i32_type());
    let g = foreign
        .add_function_dyn("g", foreign_ty, Linkage::External)
        .expect("g");
    let b = builder(&home, "f");

    let before = format!("{home}");
    let result = b.call_dyn(foreign.view(g), Vec::<Value<'_, DynBrand>>::new(), "r");
    assert!(matches!(result, Err(IrError::ForeignValueId)), "{result:?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected callee must not mutate"
    );
}

/// `IntoTypedCallee` refuses a typed function facade from another module.
///
/// No upstream counterpart: `IRBuilderBase::CreateCall` takes a
/// `FunctionCallee`.
#[test]
fn a_typed_callee_rejects_a_function_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let g = foreign
        .add_typed_function::<i32, (), _>("g", Linkage::External)
        .expect("g");
    let b = builder(&home, "f");

    let before = format!("{home}");
    let result = b.call(foreign.view(g), (), "r");
    assert!(matches!(result, Err(IrError::ForeignValueId)), "{result:?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected callee must not mutate"
    );
}

/// Each arm of `IntoBasicBlockLabel` — a `BasicBlockLabel`, a borrowed
/// `BasicBlock` and an owned one — refuses a block from another module.
///
/// No upstream counterpart: `IRBuilderBase::CreateBr` (`IR/IRBuilder.h`)
/// takes a `BasicBlock *`; `Verifier::visitTerminator` checks successors only
/// after the fact.
#[test]
fn a_branch_target_rejects_a_block_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let foreign_block = open_block(&foreign, "g");

    let label_builder = builder(&home, "f1");
    let borrowed_builder = builder(&home, "f2");
    let owned_builder = builder(&home, "f3");
    let before = format!("{home}");

    let from_label = label_builder.br(foreign.view(foreign_block.id()));
    assert!(
        matches!(from_label, Err(IrError::ForeignValueId)),
        "{from_label:?}"
    );
    let from_borrowed = borrowed_builder.br(&foreign_block);
    assert!(
        matches!(from_borrowed, Err(IrError::ForeignValueId)),
        "{from_borrowed:?}"
    );
    let from_owned = owned_builder.br(foreign_block);
    assert!(
        matches!(from_owned, Err(IrError::ForeignValueId)),
        "{from_owned:?}"
    );
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected target must not mutate"
    );
}

/// `IntoConstantValue` refuses a constant from another module, so an
/// aggregate constant cannot intern a foreign element's slot.
///
/// No upstream counterpart: `ConstantArray::get` (`lib/IR/Constants.cpp`)
/// takes `ArrayRef<Constant *>`.
#[test]
fn a_constant_operand_rejects_a_constant_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let array = home.array_type(home.i32_type(), 1);

    let before = format!("{home}");
    let result = array.const_array([foreign.i32_type().const_int(1i32)]);
    assert!(matches!(result, Err(IrError::ForeignValueId)), "{result:?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected element must not mutate"
    );
}

/// The struct-schema arms of `IntoIrField` and `IntoCallArg` refuse a value
/// from another module.
///
/// No upstream counterpart: llvmkit's struct schemas have none, and
/// `IRBuilderBase::CreateExtractValue` / `CreateCall` take `Value *`s.
#[test]
fn a_struct_schema_operand_rejects_a_value_from_another_module() {
    // `#[derive(IrStruct)]` is dual-purpose (Rust data + IR schema); read the
    // Rust side once, as `derived_struct_schema.rs` does, so it is not dead.
    let rust_pair = Pair {
        first: 1,
        second: 2,
    };
    let _ = rust_pair.first + rust_pair.second;
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let g = foreign
        .add_typed_function::<i32, (Pair,), _>("g", Linkage::External)
        .expect("g");
    let foreign_pair = foreign
        .view(g)
        .as_function()
        .param(0)
        .expect("parameter")
        .as_erased();
    let h = home
        .add_typed_function::<i32, (Pair,), _>("h", Linkage::External)
        .expect("h");
    let b = builder(&home, "f");

    let before = format!("{home}");
    let field = b.extract_field::<Pair, i32, _, _>(foreign_pair, 0, "first");
    assert!(matches!(field, Err(IrError::ForeignValueId)), "{field:?}");
    let call = b.call(home.view(h), (foreign_pair,), "r");
    assert!(matches!(call, Err(IrError::ForeignValueId)), "{call:?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected operand must not mutate"
    );
}

/// `GlobalBuilder::initializer` then `build()` rejects a foreign constant and
/// installs nothing.
///
/// No upstream counterpart: `GlobalVariable::GlobalVariable`
/// (`lib/IR/Globals.cpp`) takes the initializer as a `Constant *`.
#[test]
fn a_global_builder_rejects_an_initializer_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let foreign_zero = foreign.i32_type().const_int(0i32);
    let result = home
        .global_builder("g", home.i32_type().as_type())
        .initializer(foreign_zero)
        .build();
    assert!(matches!(result, Err(IrError::ForeignValueId)), "{result:?}");
    assert!(
        home.global("g").is_none(),
        "a rejected build must not install"
    );
}

/// `GlobalVariable::set_initializer` rejects a foreign constant and leaves the
/// existing initializer in place.
///
/// No upstream counterpart: `GlobalVariable::setInitializer`
/// (`lib/IR/Globals.cpp`) stores a `Constant *`.
#[test]
fn set_initializer_rejects_a_constant_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let global = home
        .add_global("g", home.i32_type().const_int(3i32))
        .expect("global");
    let before = format!("{home}");
    let result = home
        .view(global)
        .set_initializer(&home, foreign.i32_type().const_int(9i32));
    assert!(matches!(result, Err(IrError::ForeignValueId)), "{result:?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected store must not mutate"
    );
}

/// Regression guard for the site that already checked by hand, so migrating it
/// to the checked door cannot silently drop the check.
///
/// No upstream counterpart: `ValueAsMetadata::get` (`lib/IR/Metadata.cpp`)
/// takes a `Value *`.
#[test]
fn metadata_constant_still_rejects_a_constant_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let result = home.metadata_constant(foreign.i32_type().const_int(1i32));
    assert!(matches!(result, Err(IrError::ForeignValueId)), "{result:?}");
}

/// `global_builder` rejects a value type from another module and installs
/// nothing: the type's slot would name a different type in this module's
/// type arena.
///
/// No upstream counterpart: `GlobalVariable::GlobalVariable`
/// (`lib/IR/Globals.cpp`) takes a `Type *`, uniqued per `LLVMContext`.
#[test]
fn a_global_builder_rejects_a_value_type_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let result = home
        .global_builder("g", foreign.i32_type().as_type())
        .build();
    assert!(matches!(result, Err(IrError::ForeignType)), "{result:?}");
    assert!(
        home.global("g").is_none(),
        "a rejected build must not install"
    );
}

/// `alias_builder(..).build()` rejects a value type from another module and
/// installs nothing, even with an aliasee this module owns.
///
/// No upstream counterpart: `GlobalAlias::create` (`lib/IR/Globals.cpp`)
/// takes a `Type *`, uniqued per `LLVMContext`.
#[test]
fn alias_builder_rejects_a_value_type_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let target = home
        .add_global("target", home.i32_type().const_int(0i32))
        .expect("target");
    let result = home
        .alias_builder("alias", foreign.i32_type().as_type(), home.view(target))
        .build();
    assert!(matches!(result, Err(IrError::ForeignType)), "{result:?}");
    assert!(
        home.alias("alias").is_none(),
        "a rejected build must not install"
    );
}

/// `ifunc_builder(..).build()` rejects a value type from another module and
/// installs nothing, even with a resolver this module owns.
///
/// No upstream counterpart: `GlobalIFunc::create` (`lib/IR/Globals.cpp`)
/// takes a `Type *`, uniqued per `LLVMContext`.
#[test]
fn ifunc_builder_rejects_a_value_type_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let resolver = home
        .add_global("resolver", home.i32_type().const_int(0i32))
        .expect("resolver");
    let result = home
        .ifunc_builder("ifunc", foreign.i32_type().as_type(), home.view(resolver))
        .build();
    assert!(matches!(result, Err(IrError::ForeignType)), "{result:?}");
    assert!(
        home.ifunc("ifunc").is_none(),
        "a rejected build must not install"
    );
}

/// `constant_expr` refuses a result type and an operand from another module
/// before anything is canonicalized, folded or interned.
///
/// No upstream counterpart: `ConstantExpr::get` (`lib/IR/Constants.cpp`)
/// takes `Constant *` operands and a `Type *` uniqued per `LLVMContext`.
#[test]
fn constant_expr_rejects_a_type_or_operand_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let own = home.i32_type().const_int(1i32).as_erased();
    let before = format!("{home}");

    let from_type = home.constant_expr(
        foreign.i32_type().as_type(),
        llvmkit_ir::ConstantExprOpcode::Add,
        [own, own],
        [],
        [],
        llvmkit_ir::ConstantExprFlags::none(),
    );
    assert!(
        matches!(from_type, Err(IrError::ForeignType)),
        "{from_type:?}"
    );
    let from_operand = home.constant_expr(
        home.i32_type().as_type(),
        llvmkit_ir::ConstantExprOpcode::Add,
        [foreign.i32_type().const_int(1i32).as_erased(), own],
        [],
        [],
        llvmkit_ir::ConstantExprFlags::none(),
    );
    assert!(
        matches!(from_operand, Err(IrError::ForeignValueId)),
        "{from_operand:?}"
    );
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected expression must not mutate"
    );
}

/// `block_address` refuses a function and block from another module.
///
/// No upstream counterpart: `BlockAddress::get` (`lib/IR/Constants.cpp`) takes
/// a `BasicBlock *`, whose parent is its own function.
#[test]
fn block_address_rejects_a_block_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let foreign_ty = foreign.function_type_no_parameters(foreign.i32_type());
    let g = foreign
        .add_function_dyn("g", foreign_ty, Linkage::External)
        .expect("g");
    let foreign_block = foreign.view(g).append_basic_block(&foreign, "bb");

    let before = format!("{home}");
    let result = home.block_address(foreign.view(g), &foreign_block);
    assert!(matches!(result, Err(IrError::ForeignValueId)), "{result:?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected constant must not mutate"
    );
}

/// The forward-reference placeholder refuses a type from another module, and
/// retiring one refuses a replacement from another module.
///
/// No upstream counterpart: `LLParser`'s sentinels are ordinary `Value`s of a
/// `Type *` uniqued per `LLVMContext`, retired by `Value::replaceAllUsesWith`
/// (`lib/IR/Value.cpp`), which takes a `Value *`.
#[test]
fn forward_ref_placeholder_rejects_a_type_or_replacement_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");

    let foreign_type = home.forward_ref_value_placeholder(foreign.i32_type().as_type());
    assert!(
        matches!(foreign_type, Err(IrError::ForeignType)),
        "{foreign_type:?}"
    );

    let placeholder = home
        .forward_ref_value_placeholder(home.i32_type().as_type())
        .expect("placeholder");
    let result = placeholder.replace_all_uses_with(foreign.i32_type().const_int(1i32).as_erased());
    assert!(matches!(result, Err(IrError::ForeignValueId)), "{result:?}");
}

/// `dso_local_equivalent_global` and `no_cfi_global` refuse a global from
/// another module before reading this module's arena at its slot.
///
/// No upstream counterpart: `DSOLocalEquivalent::get` and `NoCFIValue::get`
/// (`lib/IR/Constants.cpp`) take a `GlobalValue *`.
#[test]
fn global_wrapping_constants_reject_a_global_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let foreign_ty = foreign.function_type_no_parameters(foreign.i32_type());
    let g = foreign
        .add_function_dyn("g", foreign_ty, Linkage::External)
        .expect("g");
    let foreign_global = foreign.view(g).as_global_constant_ptr();

    let before = format!("{home}");
    let dso = home.dso_local_equivalent_global(foreign_global);
    assert!(matches!(dso, Err(IrError::ForeignValueId)), "{dso:?}");
    let no_cfi = home.no_cfi_global(foreign_global);
    assert!(matches!(no_cfi, Err(IrError::ForeignValueId)), "{no_cfi:?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected constant must not mutate"
    );
}

/// `ptr_auth` refuses a pointer from another module before any operand is
/// read through this module.
///
/// No upstream counterpart: `ConstantPtrAuth::get` (`lib/IR/Constants.cpp`)
/// takes `Constant *` operands.
#[test]
fn ptr_auth_rejects_a_pointer_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let own_null = home.ptr_type(0).const_null();

    let before = format!("{home}");
    let result = home.ptr_auth(
        foreign.ptr_type(0).const_null(),
        home.i32_type().const_int(0i32),
        home.i64_type().const_int(0i64),
        own_null,
        own_null,
    );
    assert!(matches!(result, Err(IrError::ForeignValueId)), "{result:?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected constant must not mutate"
    );
}

/// `target_ext_none` refuses a target extension type from another module.
///
/// No upstream counterpart: `ConstantTargetNone::get` (`lib/IR/Constants.cpp`)
/// takes a `TargetExtType *` uniqued per `LLVMContext`.
#[test]
fn target_ext_none_rejects_a_type_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let foreign_ty = foreign
        .target_ext_type(
            "spirv.Event",
            Vec::<llvmkit_ir::Type<'_, DynBrand>>::new(),
            Vec::<u32>::new(),
        )
        .as_type();

    let before = format!("{home}");
    let result = home.target_ext_none(foreign_ty);
    assert!(matches!(result, Err(IrError::ForeignType)), "{result:?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected constant must not mutate"
    );
}

/// Every integer-cast entry refuses a destination type from another module
/// before its folder or this module reads it: the typed `trunc`, `zext`,
/// `sext`, their flag-carrying twins and `bitcast_int_to_int`, the
/// runtime-width `trunc_dyn`, `trunc_with_flags_dyn`, `zext_dyn` / `sext_dyn`
/// (one shared helper) and `zext_with_flags_dyn`, and `int_cast_erased`.
///
/// No upstream counterpart: `IRBuilderBase::CreateTrunc`, `CreateZExt`,
/// `CreateSExt` and `CreateCast` (`IR/IRBuilder.h`) take a destination
/// `Type *` uniqued per `LLVMContext`.
#[test]
fn an_integer_cast_rejects_a_destination_type_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let (b, [narrow, wide, ..]) = builder_with_parameters(&home, "f");
    let narrow_typed: IntValue<'_, i32, DynBrand> = narrow.try_into().expect("an i32");
    let wide_typed: IntValue<'_, i64, DynBrand> = wide.try_into().expect("an i64");
    let narrow_dyn: IntValue<'_, IntDyn, DynBrand> = narrow.try_into().expect("an integer");
    let wide_dyn: IntValue<'_, IntDyn, DynBrand> = wide.try_into().expect("an integer");
    let before = format!("{home}");

    let outcomes = vec![
        (
            "trunc",
            b.trunc::<i64, i32, _, _>(wide_typed, foreign.i32_type(), "t")
                .map(|_| ()),
        ),
        (
            "trunc_with_flags",
            b.trunc_with_flags::<i64, i32, _, _>(
                wide_typed,
                foreign.i32_type(),
                TruncFlags::new(),
                "t",
            )
            .map(|_| ()),
        ),
        (
            "zext",
            b.zext::<i32, i64, _, _>(narrow_typed, foreign.i64_type(), "z")
                .map(|_| ()),
        ),
        (
            "zext_with_flags",
            b.zext_with_flags::<i32, i64, _, _>(
                narrow_typed,
                foreign.i64_type(),
                ZextFlags::new(),
                "z",
            )
            .map(|_| ()),
        ),
        (
            "sext",
            b.sext::<i32, i64, _, _>(narrow_typed, foreign.i64_type(), "s")
                .map(|_| ()),
        ),
        (
            "bitcast_int_to_int",
            b.bitcast_int_to_int::<i32, i32, _, _>(narrow_typed, foreign.i32_type(), "c")
                .map(|_| ()),
        ),
        (
            "trunc_dyn",
            b.trunc_dyn(wide_dyn, foreign.i32_type().as_dyn(), "t")
                .map(|_| ()),
        ),
        (
            "trunc_with_flags_dyn",
            b.trunc_with_flags_dyn(
                wide_dyn,
                foreign.i32_type().as_dyn(),
                TruncFlags::new(),
                "t",
            )
            .map(|_| ()),
        ),
        (
            "zext_dyn",
            b.zext_dyn(narrow_dyn, foreign.i64_type().as_dyn(), "z")
                .map(|_| ()),
        ),
        (
            "sext_dyn",
            b.sext_dyn(narrow_dyn, foreign.i64_type().as_dyn(), "s")
                .map(|_| ()),
        ),
        (
            "zext_with_flags_dyn",
            b.zext_with_flags_dyn(
                narrow_dyn,
                foreign.i64_type().as_dyn(),
                ZextFlags::new(),
                "z",
            )
            .map(|_| ()),
        ),
        (
            "int_cast_erased",
            b.int_cast_erased(
                CastOpcode::Zext,
                narrow,
                foreign.i64_type().as_type(),
                IntCastFlags::new(),
                "z",
            )
            .map(|_| ()),
        ),
    ];
    let let_through = not_refused(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(format!("{home}"), before, "a rejected cast must not mutate");
}

/// Every floating-point cast entry refuses a destination type from another
/// module before its folder or this module reads it: the typed `fp_ext` /
/// `fp_trunc` (one shared helper), `fp_to_ui` / `fp_to_si` (one helper),
/// `ui_to_fp` / `si_to_fp` (one helper), `ui_to_fp_with_flags`, the three
/// float-side static bitcasts, and the runtime-kind `fp_ext_dyn`,
/// `fp_trunc_dyn` and `ui_to_fp_with_flags_dyn`.
///
/// No upstream counterpart: `IRBuilderBase::CreateFPExt`, `CreateFPTrunc`,
/// `CreateFPToUI`, `CreateUIToFP` and `CreateBitCast` (`IR/IRBuilder.h`) take
/// a destination `Type *` uniqued per `LLVMContext`.
#[test]
fn a_float_cast_rejects_a_destination_type_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let (b, [narrow, _, single, double, _]) = builder_with_parameters(&home, "f");
    let narrow_typed: IntValue<'_, i32, DynBrand> = narrow.try_into().expect("an i32");
    let narrow_dyn: IntValue<'_, IntDyn, DynBrand> = narrow.try_into().expect("an integer");
    let single_typed: FloatValue<'_, f32, DynBrand> = single.try_into().expect("a float");
    let double_typed: FloatValue<'_, f64, DynBrand> = double.try_into().expect("a double");
    let single_dyn: FloatValue<'_, FloatDyn, DynBrand> = single.try_into().expect("a float");
    let double_dyn: FloatValue<'_, FloatDyn, DynBrand> = double.try_into().expect("a double");
    let before = format!("{home}");

    let outcomes = vec![
        (
            "fp_ext",
            b.fp_ext::<f32, f64, _, _>(single_typed, foreign.f64_type(), "e")
                .map(|_| ()),
        ),
        (
            "fp_trunc",
            b.fp_trunc::<f64, f32, _, _>(double_typed, foreign.f32_type(), "t")
                .map(|_| ()),
        ),
        (
            "fp_to_ui",
            b.fp_to_ui::<f32, i32, _, _>(single_typed, foreign.i32_type(), "c")
                .map(|_| ()),
        ),
        (
            "fp_to_si",
            b.fp_to_si::<f32, i32, _, _>(single_typed, foreign.i32_type(), "c")
                .map(|_| ()),
        ),
        (
            "ui_to_fp",
            b.ui_to_fp::<i32, f32, _, _>(narrow_typed, foreign.f32_type(), "c")
                .map(|_| ()),
        ),
        (
            "si_to_fp",
            b.si_to_fp::<i32, f32, _, _>(narrow_typed, foreign.f32_type(), "c")
                .map(|_| ()),
        ),
        (
            "ui_to_fp_with_flags",
            b.ui_to_fp_with_flags::<i32, f32, _, _>(
                narrow_typed,
                foreign.f32_type(),
                UiToFpFlags::new(),
                "c",
            )
            .map(|_| ()),
        ),
        (
            "bitcast_int_to_fp",
            b.bitcast_int_to_fp::<i32, f32, _, _>(narrow_typed, foreign.f32_type(), "c")
                .map(|_| ()),
        ),
        (
            "bitcast_fp_to_int",
            b.bitcast_fp_to_int::<f32, i32, _, _>(single_typed, foreign.i32_type(), "c")
                .map(|_| ()),
        ),
        (
            "bitcast_fp_to_fp",
            b.bitcast_fp_to_fp::<f32, f32, _, _>(single_typed, foreign.f32_type(), "c")
                .map(|_| ()),
        ),
        (
            "fp_ext_dyn",
            b.fp_ext_dyn(single_dyn, foreign.f64_type().as_dyn(), "e")
                .map(|_| ()),
        ),
        (
            "fp_trunc_dyn",
            b.fp_trunc_dyn(double_dyn, foreign.f32_type().as_dyn(), "t")
                .map(|_| ()),
        ),
        (
            "ui_to_fp_with_flags_dyn",
            b.ui_to_fp_with_flags_dyn(
                narrow_dyn,
                foreign.f32_type().as_dyn(),
                UiToFpFlags::new(),
                "c",
            )
            .map(|_| ()),
        ),
    ];
    let let_through = not_refused(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(format!("{home}"), before, "a rejected cast must not mutate");
}

/// Every pointer-cast entry refuses a destination type from another module
/// before its folder or this module reads it: `ptr_to_int`, `int_to_ptr`,
/// `addrspace_cast`, `pointer_cast`, `ptr_to_addr_dyn` and `bitcast_dyn`.
///
/// No upstream counterpart: `IRBuilderBase::CreatePtrToInt`,
/// `CreateIntToPtr`, `CreateAddrSpaceCast`, `CreatePtrToAddr` and
/// `CreateBitCast` (`IR/IRBuilder.h`) take a destination `Type *` uniqued per
/// `LLVMContext`.
#[test]
fn a_pointer_cast_rejects_a_destination_type_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let (b, [_, wide, _, _, pointer]) = builder_with_parameters(&home, "f");
    let wide_typed: IntValue<'_, i64, DynBrand> = wide.try_into().expect("an i64");
    let pointer_typed: PointerValue<'_, DynBrand> = pointer.try_into().expect("a pointer");
    let before = format!("{home}");

    let outcomes = vec![
        (
            "ptr_to_int",
            b.ptr_to_int::<i64, _, _>(pointer_typed, foreign.i64_type(), "c")
                .map(|_| ()),
        ),
        (
            "int_to_ptr",
            b.int_to_ptr::<i64, _, _>(wide_typed, foreign.ptr_type(0), "c")
                .map(|_| ()),
        ),
        (
            "addrspace_cast",
            b.addrspace_cast(pointer_typed, foreign.ptr_type(1), "c")
                .map(|_| ()),
        ),
        (
            "pointer_cast",
            b.pointer_cast(pointer_typed, foreign.ptr_type(0), "c")
                .map(|_| ()),
        ),
        (
            "ptr_to_addr_dyn",
            b.ptr_to_addr_dyn(pointer, foreign.i64_type().as_type(), "c")
                .map(|_| ()),
        ),
        (
            "bitcast_dyn",
            b.bitcast_dyn(pointer, foreign.ptr_type(0).as_type(), "c")
                .map(|_| ()),
        ),
    ];
    let let_through = not_refused(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(format!("{home}"), before, "a rejected cast must not mutate");
}

/// Every memory entry refuses a type from another module before this module
/// reads it: the allocated type of `alloca`, `alloca_with_align`,
/// `array_alloca`, `array_alloca_with_align` and `alloca_builder`, the load
/// type of `load`, `load_with_align`, `int_load_dyn`, `fp_load_dyn` and
/// `LoadBuilder::erased`, and the source element type of `gep` and
/// `gep_erased`.
///
/// No upstream counterpart: `IRBuilderBase::CreateAlloca`, `CreateLoad` and
/// `CreateGEP` (`IR/IRBuilder.h`) take a `Type *` uniqued per `LLVMContext`.
#[test]
fn a_memory_entry_rejects_a_type_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let (b, [_, wide, _, _, pointer]) = builder_with_parameters(&home, "f");
    let wide_dyn: IntValue<'_, IntDyn, DynBrand> = wide.try_into().expect("an integer");
    let pointer_typed: PointerValue<'_, DynBrand> = pointer.try_into().expect("a pointer");
    let align = Align::new(4).expect("a power of two");
    let before = format!("{home}");

    let outcomes = vec![
        ("alloca", b.alloca(foreign.i32_type(), "a").map(|_| ())),
        (
            "alloca_with_align",
            b.alloca_with_align(foreign.i32_type(), align, "a")
                .map(|_| ()),
        ),
        (
            "array_alloca",
            b.array_alloca(foreign.i32_type(), wide_dyn, "a")
                .map(|_| ()),
        ),
        (
            "array_alloca_with_align",
            b.array_alloca_with_align(foreign.i32_type(), wide_dyn, align, "a")
                .map(|_| ()),
        ),
        (
            "alloca_builder",
            b.alloca_builder(foreign.i32_type()).build().map(|_| ()),
        ),
        (
            "load",
            b.load(foreign.i32_type(), pointer_typed, "l").map(|_| ()),
        ),
        (
            "load_with_align",
            b.load_with_align(foreign.i32_type(), pointer_typed, align, "l")
                .map(|_| ()),
        ),
        (
            "int_load_dyn",
            b.int_load_dyn(foreign.i32_type().as_dyn(), pointer_typed, "l")
                .map(|_| ()),
        ),
        (
            "fp_load_dyn",
            b.fp_load_dyn(foreign.f32_type().as_dyn(), pointer_typed, "l")
                .map(|_| ()),
        ),
        (
            "LoadBuilder::erased",
            b.load_from(pointer_typed)
                .erased(foreign.i32_type(), "l")
                .map(|_| ()),
        ),
        (
            "gep",
            b.gep(
                foreign.i32_type(),
                pointer_typed,
                Vec::<IntValue<'_, IntDyn, DynBrand>>::new(),
                "g",
            )
            .map(|_| ()),
        ),
        (
            "gep_erased",
            b.gep_erased(
                foreign.i32_type(),
                pointer,
                Vec::<Value<'_, DynBrand>>::new(),
                GepNoWrapFlags::inbounds(),
                "g",
            )
            .map(|_| ()),
        ),
    ];
    let let_through = not_refused(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected memory entry must not mutate"
    );
}

/// Every phi entry that takes its type, `landingpad` and `va_arg` refuse a
/// type from another module before this module reads it.
///
/// No upstream counterpart: `IRBuilderBase::CreatePHI`, `CreateLandingPad`
/// and `CreateVAArg` (`IR/IRBuilder.h`) take a `Type *` uniqued per
/// `LLVMContext`.
#[test]
fn a_phi_pad_or_va_arg_rejects_a_type_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let (b, [.., pointer]) = builder_with_parameters(&home, "f");
    let pointer_typed: PointerValue<'_, DynBrand> = pointer.try_into().expect("a pointer");
    let before = format!("{home}");

    let outcomes = vec![
        (
            "int_phi_dyn",
            b.int_phi_dyn(foreign.i32_type().as_dyn(), "p").map(|_| ()),
        ),
        (
            "fp_phi_dyn",
            b.fp_phi_dyn(foreign.f32_type().as_dyn(), "p").map(|_| ()),
        ),
        (
            "pointer_phi_in_addrspace",
            b.pointer_phi_in_addrspace(foreign.ptr_type(0), "p")
                .map(|_| ()),
        ),
        (
            "phi_dyn",
            b.phi_dyn(foreign.i32_type().as_type(), "p").map(|_| ()),
        ),
        (
            "landingpad",
            b.landingpad(foreign.i32_type().as_type(), true, "pad")
                .map(|_| ()),
        ),
        (
            "va_arg",
            b.va_arg(pointer_typed, foreign.i32_type().as_type(), "v")
                .map(|_| ()),
        ),
    ];
    let let_through = not_refused(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected entry must not mutate"
    );
}
