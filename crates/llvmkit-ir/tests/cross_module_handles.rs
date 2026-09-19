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
    Align, Analyses, AtomicOrdering, AtomicRmwBinOp, AtomicRmwConfig, BasicBlock, CallSiteConfig,
    CastOpcode, DominatorTreeAnalysis, Dyn, DynBrand, FloatDyn, FloatValue, FnCx, FnReport,
    FunctionId, FunctionPass, GepNoWrapFlags, InlineAsmOptions, InstructionView, IntCastFlags,
    IntDyn, IntValue, IrBuilder, IrError, IrResult, IrStruct, Linkage, Module, OperandBundleDef,
    OperandBundleTag, PatchBody, PointerValue, Positioned, ReshapeCfg, SsaBuilder, SsaState,
    SyncScope, TailCallKind, TruncFlags, Type, UiToFpFlags, Unterminated, Value, ValueId,
    ZextFlags, iter::BlockCursor, run_function_pass,
};

/// A two-field schema, so a struct-typed value exists to hand across modules.
#[derive(IrStruct)]
struct Pair {
    first: i32,
    second: i32,
}

/// A positioned erased-return builder over a `DynBrand` module.
type PositionedBuilder<'m> =
    IrBuilder<'m, 'm, DynBrand, llvmkit_ir::ConstantFolder, Positioned, Dyn>;

/// An unterminated block of an erased-return function in a `DynBrand` module.
type OpenBlock<'m> = BasicBlock<'m, Dyn, Unterminated, DynBrand>;

/// `i32 f()` in `module`, with an empty block named `name` to build into.
fn open_block<'m>(module: &'m Module<DynBrand>, name: &str) -> OpenBlock<'m> {
    let fn_ty = module.function_type_no_parameters(module.i32_type());
    let f = module
        .add_function_dyn(name, fn_ty, Linkage::External)
        .expect("function");
    module.view(f).append_basic_block(module, "entry")
}

/// A builder positioned at the end of a fresh block of `module`.
fn builder<'m>(module: &'m Module<DynBrand>, name: &str) -> PositionedBuilder<'m> {
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
) -> (PositionedBuilder<'m>, [Value<'m, DynBrand>; 5]) {
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

/// A builder positioned in the entry block of a fresh `i32 f()` in `module`,
/// plus two more empty blocks of that function for a terminator to target.
fn builder_with_targets<'m>(
    module: &'m Module<DynBrand>,
    name: &str,
) -> (PositionedBuilder<'m>, OpenBlock<'m>, OpenBlock<'m>) {
    let fn_ty = module.function_type_no_parameters(module.i32_type());
    let f = module
        .add_function_dyn(name, fn_ty, Linkage::External)
        .expect("function");
    let function = module.view(f);
    let entry = function.append_basic_block(module, "entry");
    let first = function.append_basic_block(module, "first");
    let second = function.append_basic_block(module, "second");
    (
        IrBuilder::new_for::<Dyn>(module).position_at_end(entry),
        first,
        second,
    )
}

/// The labels of the outcomes that are not the refusal each expects, so one
/// assertion names every entry that let a foreign handle through. The
/// comparison is by variant: every refusal here is a payload-free variant.
fn not_refused_as_expected(
    outcomes: Vec<(&'static str, IrError, Result<(), IrError>)>,
) -> Vec<String> {
    outcomes
        .into_iter()
        .filter(|(_, expected, outcome)| {
            outcome.as_ref().err().map(core::mem::discriminant)
                != Some(core::mem::discriminant(expected))
        })
        .map(|(label, expected, outcome)| {
            format!("{label}: expected {expected:?}, got {outcome:?}")
        })
        .collect()
}

/// An outcome with its success value dropped, so entries returning different
/// values can share one `not_refused_as_expected` list.
fn without_value<T>(outcome: Result<T, IrError>) -> Result<(), IrError> {
    outcome.map(|_| ())
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

/// A `call` site refuses a callee or a function type from another module
/// before either is read: `call_builder`'s callee and a
/// `CallBuilder::call_site_type` override (both admitted by `build`),
/// `call_erased`'s spelled function type, callee and `CallSiteConfig`
/// override, and the two forwarding entries `indirect_call_dyn` (spelled
/// type) and `inline_asm_call` (the asm's own type).
///
/// No upstream counterpart: `IRBuilderBase::CreateCall` (`IR/IRBuilder.h`)
/// takes a `FunctionType *` uniqued per `LLVMContext` and a `Value *` callee.
#[test]
fn a_call_site_rejects_a_callee_or_function_type_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let home_fn_ty = home.function_type_no_parameters(home.i32_type());
    let foreign_fn_ty = foreign.function_type_no_parameters(foreign.i32_type());
    let h = home
        .add_function_dyn("h", home_fn_ty, Linkage::External)
        .expect("h");
    let g = foreign
        .add_function_dyn("g", foreign_fn_ty, Linkage::External)
        .expect("g");
    let foreign_asm = foreign.inline_asm(foreign_fn_ty, "nop", "=r", InlineAsmOptions::new());
    let own_callee = home.view(h).as_erased();
    let no_args = Vec::<Value<'_, DynBrand>>::new;
    let b = builder(&home, "f");
    let before = format!("{home}");

    let outcomes = vec![
        (
            "call_builder callee",
            IrError::ForeignValueId,
            b.call_builder(foreign.view(g)).build().map(|_| ()),
        ),
        (
            "CallBuilder::call_site_type",
            IrError::ForeignType,
            b.call_builder(home.view(h))
                .call_site_type(foreign_fn_ty)
                .build()
                .map(|_| ()),
        ),
        (
            "call_erased function type",
            IrError::ForeignType,
            b.call_erased::<Dyn, _, _>(
                foreign_fn_ty,
                own_callee,
                no_args(),
                TailCallKind::None,
                CallSiteConfig::new("r"),
            )
            .map(|_| ()),
        ),
        (
            "call_erased callee",
            IrError::ForeignValueId,
            b.call_erased::<Dyn, _, _>(
                home_fn_ty,
                foreign.view(g).as_erased(),
                no_args(),
                TailCallKind::None,
                CallSiteConfig::new("r"),
            )
            .map(|_| ()),
        ),
        (
            "CallSiteConfig::call_site_type on call_erased",
            IrError::ForeignType,
            b.call_erased::<Dyn, _, _>(
                home_fn_ty,
                own_callee,
                no_args(),
                TailCallKind::None,
                CallSiteConfig::new("r").call_site_type(foreign_fn_ty),
            )
            .map(|_| ()),
        ),
        (
            "indirect_call_dyn",
            IrError::ForeignType,
            b.indirect_call_dyn::<Dyn, _, _, _, _>(
                foreign_fn_ty,
                home.ptr_type(0).const_null(),
                no_args(),
                "r",
            )
            .map(|_| ()),
        ),
        (
            "inline_asm_call",
            IrError::ForeignType,
            b.inline_asm_call::<Dyn, _, _, _>(foreign_asm, no_args(), "r")
                .map(|_| ()),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(format!("{home}"), before, "a rejected call must not mutate");
}

/// `invoke` and `callbr` refuse a callee or a function type from another
/// module before anything is read, stored or seeded: the function callee of
/// `invoke_with_config`, `invoke_with_args`, `invoke_dyn_with_config`,
/// `invoke_dyn_with_args` and `callbr_with_config`, a `CallSiteConfig`
/// override on the last two families, the inline-asm callee of
/// `inline_asm_invoke_with_config` and `inline_asm_callbr_with_config`, and
/// the spelled type of `indirect_invoke_dyn_with_config` and
/// `indirect_callbr_with_config`.
///
/// No upstream counterpart: `IRBuilderBase::CreateInvoke` and `CreateCallBr`
/// (`IR/IRBuilder.h`) take a `FunctionType *` uniqued per `LLVMContext` and a
/// `Value *` callee.
#[test]
fn invoke_and_callbr_reject_a_callee_or_function_type_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let home_fn_ty = home.function_type_no_parameters(home.i32_type());
    let foreign_fn_ty = foreign.function_type_no_parameters(foreign.i32_type());
    let h = home
        .add_function_dyn("h", home_fn_ty, Linkage::External)
        .expect("h");
    let g = foreign
        .add_function_dyn("g", foreign_fn_ty, Linkage::External)
        .expect("g");
    let typed_g = foreign
        .add_typed_function::<i32, (), _>("typed_g", Linkage::External)
        .expect("typed_g");
    let foreign_asm = foreign.inline_asm(foreign_fn_ty, "nop", "=r", InlineAsmOptions::new());
    let home_null = home.ptr_type(0).const_null();
    let no_values: Vec<Value<'_, DynBrand>> = Vec::new();
    let no_args = Vec::<Value<'_, DynBrand>>::new;
    let no_indirects = Vec::<BasicBlock<'_, Dyn, Unterminated, DynBrand>>::new;
    let (b1, n1, u1) = builder_with_targets(&home, "f1");
    let (b2, n2, u2) = builder_with_targets(&home, "f2");
    let (b3, n3, u3) = builder_with_targets(&home, "f3");
    let (b4, n4, u4) = builder_with_targets(&home, "f4");
    let (b5, n5, u5) = builder_with_targets(&home, "f5");
    let (b6, n6, u6) = builder_with_targets(&home, "f6");
    let (b7, n7, u7) = builder_with_targets(&home, "f7");
    let (b8, d8, _) = builder_with_targets(&home, "f8");
    let (b9, d9, _) = builder_with_targets(&home, "f9");
    let (b10, d10, _) = builder_with_targets(&home, "f10");
    let (b11, d11, _) = builder_with_targets(&home, "f11");
    let before = format!("{home}");

    let outcomes = vec![
        (
            "invoke_dyn_with_config callee",
            IrError::ForeignValueId,
            b1.invoke_dyn_with_config(foreign.view(g), no_args(), n1, u1, CallSiteConfig::new("r"))
                .map(|_| ()),
        ),
        (
            "invoke_dyn_with_args callee",
            IrError::ForeignValueId,
            b2.invoke_dyn_with_args(
                foreign.view(g),
                no_args(),
                (n2, no_values.as_slice()),
                (u2, no_values.as_slice()),
                "r",
            )
            .map(|_| ()),
        ),
        (
            "CallSiteConfig::call_site_type on invoke_dyn_with_config",
            IrError::ForeignType,
            b3.invoke_dyn_with_config(
                home.view(h),
                no_args(),
                n3,
                u3,
                CallSiteConfig::new("r").call_site_type(foreign_fn_ty),
            )
            .map(|_| ()),
        ),
        (
            "invoke_with_config callee",
            IrError::ForeignValueId,
            b4.invoke_with_config(foreign.view(typed_g), (), n4, u4, CallSiteConfig::new("r"))
                .map(|_| ()),
        ),
        (
            "invoke_with_args callee",
            IrError::ForeignValueId,
            b5.invoke_with_args(
                foreign.view(typed_g),
                (),
                (n5, no_values.as_slice()),
                (u5, no_values.as_slice()),
                "r",
            )
            .map(|_| ()),
        ),
        (
            "indirect_invoke_dyn_with_config function type",
            IrError::ForeignType,
            b6.indirect_invoke_dyn_with_config::<Dyn, _, _, _, _, _>(
                home_null,
                foreign_fn_ty,
                no_args(),
                n6,
                u6,
                CallSiteConfig::new("r"),
            )
            .map(|_| ()),
        ),
        (
            "inline_asm_invoke_with_config callee",
            IrError::ForeignValueId,
            b7.inline_asm_invoke_with_config::<Dyn, _, _, _, _>(
                foreign_asm,
                no_args(),
                n7,
                u7,
                CallSiteConfig::new("r"),
            )
            .map(|_| ()),
        ),
        (
            "callbr_with_config callee",
            IrError::ForeignValueId,
            b8.callbr_with_config(
                foreign.view(g),
                no_args(),
                d8,
                no_indirects(),
                CallSiteConfig::new("r"),
            )
            .map(|_| ()),
        ),
        (
            "CallSiteConfig::call_site_type on callbr_with_config",
            IrError::ForeignType,
            b9.callbr_with_config(
                home.view(h),
                no_args(),
                d9,
                no_indirects(),
                CallSiteConfig::new("r").call_site_type(foreign_fn_ty),
            )
            .map(|_| ()),
        ),
        (
            "indirect_callbr_with_config function type",
            IrError::ForeignType,
            b10.indirect_callbr_with_config(
                home_null,
                foreign_fn_ty,
                no_args(),
                d10,
                no_indirects(),
                CallSiteConfig::new("r"),
            )
            .map(|_| ()),
        ),
        (
            "inline_asm_callbr_with_config callee",
            IrError::ForeignValueId,
            b11.inline_asm_callbr_with_config::<Dyn, _, _, _, _, _>(
                foreign_asm,
                no_args(),
                d11,
                no_indirects(),
                CallSiteConfig::new("r"),
            )
            .map(|_| ()),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected terminator must not mutate"
    );
}

/// Every call-site entry that takes operand bundles refuses a bundle input of
/// another module with `ForeignValueId` before the call is created: each
/// consumer of `CallSiteConfig::operand_bundles`, and the `call_builder` /
/// `typed_call_builder` chains. The foreign input is the bundle's *second*,
/// so an entry that checked only the first would let it through. Both modules
/// print unchanged. Positive control: the same bundle with a home input
/// builds, prints in `AsmWriter`'s bundle form, and reads back through
/// `CallInst::operand_bundle` with the same input.
///
/// No upstream counterpart: `OperandBundleDefT<Value *>` (`IR/InstrTypes.h`)
/// holds `Value *`s, which carry no module to compare.
#[test]
fn an_operand_bundle_input_from_another_module_is_refused() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let home_fn_ty = home.function_type_no_parameters(home.i32_type());
    let h = home
        .add_function_dyn("h", home_fn_ty, Linkage::External)
        .expect("h");
    let typed_h = home
        .add_typed_function::<i32, (), _>("typed_h", Linkage::External)
        .expect("typed_h");
    let home_asm = home.inline_asm(home_fn_ty, "nop", "=r", InlineAsmOptions::new());
    let home_null = home.ptr_type(0).const_null();
    let mixed = || {
        OperandBundleDef::new(
            OperandBundleTag::Deopt,
            [
                home.i32_type().const_int(1i32).as_erased(),
                foreign.i32_type().const_int(7i32).as_erased(),
            ],
        )
    };
    let config = || CallSiteConfig::new("r").operand_bundles([mixed()]);
    let no_args = Vec::<Value<'_, DynBrand>>::new;
    let no_indirects = Vec::<BasicBlock<'_, Dyn, Unterminated, DynBrand>>::new;
    let (b0, _, _) = builder_with_targets(&home, "f0");
    let (b1, n1, u1) = builder_with_targets(&home, "f1");
    let (b2, n2, u2) = builder_with_targets(&home, "f2");
    let (b3, n3, u3) = builder_with_targets(&home, "f3");
    let (b4, n4, u4) = builder_with_targets(&home, "f4");
    let (b5, d5, _) = builder_with_targets(&home, "f5");
    let (b6, d6, _) = builder_with_targets(&home, "f6");
    let (b7, d7, _) = builder_with_targets(&home, "f7");
    let home_before = format!("{home}");
    let foreign_before = format!("{foreign}");

    let outcomes = vec![
        (
            "call_erased",
            IrError::ForeignValueId,
            without_value(b0.call_erased::<Dyn, _, _>(
                home_fn_ty,
                home.view(h).as_erased(),
                no_args(),
                TailCallKind::None,
                config(),
            )),
        ),
        (
            "call_with_config",
            IrError::ForeignValueId,
            without_value(b0.call_with_config(home.view(typed_h), (), config())),
        ),
        (
            "call_builder",
            IrError::ForeignValueId,
            without_value(
                b0.call_builder(home.view(h))
                    .operand_bundles([mixed()])
                    .build(),
            ),
        ),
        (
            "typed_call_builder",
            IrError::ForeignValueId,
            without_value(
                b0.typed_call_builder(home.view(typed_h), ())
                    .operand_bundles([mixed()])
                    .build(),
            ),
        ),
        (
            "invoke_with_config",
            IrError::ForeignValueId,
            without_value(b1.invoke_with_config(home.view(typed_h), (), n1, u1, config())),
        ),
        (
            "invoke_dyn_with_config",
            IrError::ForeignValueId,
            without_value(b2.invoke_dyn_with_config(home.view(h), no_args(), n2, u2, config())),
        ),
        (
            "indirect_invoke_dyn_with_config",
            IrError::ForeignValueId,
            without_value(b3.indirect_invoke_dyn_with_config::<Dyn, _, _, _, _, _>(
                home_null,
                home_fn_ty,
                no_args(),
                n3,
                u3,
                config(),
            )),
        ),
        (
            "inline_asm_invoke_with_config",
            IrError::ForeignValueId,
            without_value(b4.inline_asm_invoke_with_config::<Dyn, _, _, _, _>(
                home_asm,
                no_args(),
                n4,
                u4,
                config(),
            )),
        ),
        (
            "callbr_with_config",
            IrError::ForeignValueId,
            without_value(b5.callbr_with_config(
                home.view(h),
                no_args(),
                d5,
                no_indirects(),
                config(),
            )),
        ),
        (
            "indirect_callbr_with_config",
            IrError::ForeignValueId,
            without_value(b6.indirect_callbr_with_config(
                home_null,
                home_fn_ty,
                no_args(),
                d6,
                no_indirects(),
                config(),
            )),
        ),
        (
            "inline_asm_callbr_with_config",
            IrError::ForeignValueId,
            without_value(b7.inline_asm_callbr_with_config::<Dyn, _, _, _, _, _>(
                home_asm,
                no_args(),
                d7,
                no_indirects(),
                config(),
            )),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(
        format!("{home}"),
        home_before,
        "a refused bundle must not mutate"
    );
    assert_eq!(
        format!("{foreign}"),
        foreign_before,
        "a refused bundle must not mutate its input's module either"
    );

    // Positive control: a bundle of home inputs is accepted, printed, and
    // read back with the same input.
    let (b8, _, _) = builder_with_targets(&home, "f8");
    let seven = home.i32_type().const_int(7i32).as_erased();
    let call = b8
        .call_builder(home.view(h))
        .operand_bundles([OperandBundleDef::new(OperandBundleTag::Deopt, [seven])])
        .build()
        .expect("a same-module bundle is accepted");
    let call = home.view(call);
    assert!(
        format!("{home}").contains(r#"call i32 @h() [ "deopt"(i32 7) ]"#),
        "{home}"
    );
    let bundle = call
        .operand_bundle(&OperandBundleTag::Deopt)
        .expect("one deopt bundle")
        .expect("the deopt bundle is present");
    assert_eq!(bundle.inputs().collect::<Vec<_>>(), vec![seven]);
    assert_eq!(call.operand_bundles().len(), 1);
}

/// `restore_insert_point` refuses an `InsertPoint` saved from another
/// module's builder: the snapshot's block id carries that module's tag.
///
/// No upstream counterpart: `IRBuilderBase::restoreIP` (`IR/IRBuilder.h`)
/// takes an `InsertPoint` holding a `BasicBlock *`, whose identity is its
/// address.
#[test]
fn restore_insert_point_rejects_an_insert_point_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    // A block of `home` too, so a slot the foreign snapshot names can resolve
    // to something here if the tag goes unchecked.
    let _home_builder = builder(&home, "f");
    let saved = builder(&foreign, "g").save_insert_point();

    let before = format!("{home}");
    let restored = IrBuilder::new_for::<Dyn>(&home).restore_insert_point(saved);
    assert!(
        matches!(restored, Err(IrError::ForeignValueId)),
        "{restored:?}"
    );
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected restore must not mutate"
    );
}

/// A block-argument edge refuses an argument, and a predecessor, from another
/// module before any incoming is recorded. The target has two parameters, so
/// an argument admitted only while recording would already have seeded the
/// first; the predecessor comes from a builder positioned at another module's
/// block, which `position_at_end` cannot refuse.
///
/// No upstream counterpart: llvmkit's block-argument edges have none;
/// `PHINode::addIncoming` (`IR/Instructions.h`) takes a `Value *` and a
/// `BasicBlock *`.
#[test]
fn a_block_argument_edge_rejects_a_value_or_predecessor_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let fn_ty = home.function_type_no_parameters(home.i32_type());
    let f = home
        .add_function_dyn("f", fn_ty, Linkage::External)
        .expect("f");
    let entry = home.view(f).append_basic_block(&home, "entry");
    let b = IrBuilder::new_for::<Dyn>(&home).position_at_end(entry);
    let i32_ty = home.i32_type().as_type();
    let (target, _) = b
        .append_block_with_params(home.view(f), &[i32_ty, i32_ty], "target")
        .expect("target");
    let target = target.id();
    let home_one = home.i32_type().const_int(1i32).as_erased();
    let home_two = home.i32_type().const_int(2i32).as_erased();
    let foreign_two = foreign.i32_type().const_int(2i32).as_erased();
    let positioned_elsewhere =
        IrBuilder::new_for::<Dyn>(&home).position_at_end(open_block(&foreign, "g"));
    let before = format!("{home}");

    let outcomes = vec![
        (
            "argument",
            IrError::ForeignValueId,
            b.br_with_args(target, &[home_one, foreign_two]).map(|_| ()),
        ),
        (
            "predecessor",
            IrError::ForeignValueId,
            positioned_elsewhere
                .br_with_args(target, &[home_one, home_two])
                .map(|_| ()),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected edge must not record an incoming"
    );
}

/// The block-parameter appenders refuse a function, and a parameter type,
/// from another module before the block is appended:
/// `append_block_with_params`, `append_block_with_named_params` and
/// `append_block_typed` (whose types it mints itself).
///
/// No upstream counterpart: llvmkit's block parameters have none;
/// `BasicBlock::Create` (`IR/BasicBlock.h`) takes a `Function *` and
/// `PHINode::Create` a `Type *` uniqued per `LLVMContext`.
#[test]
fn append_block_with_params_rejects_a_function_or_type_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let home_fn_ty = home.function_type_no_parameters(home.i32_type());
    let foreign_fn_ty = foreign.function_type_no_parameters(foreign.i32_type());
    let h = home
        .add_function_dyn("h", home_fn_ty, Linkage::External)
        .expect("h");
    let g = foreign
        .add_function_dyn("g", foreign_fn_ty, Linkage::External)
        .expect("g");
    let home_i32 = home.i32_type().as_type();
    let foreign_i32 = foreign.i32_type().as_type();
    let b = IrBuilder::new_for::<Dyn>(&home);
    let before = format!("{home}");
    let foreign_before = format!("{foreign}");

    let outcomes = vec![
        (
            "append_block_with_params function",
            IrError::ForeignValueId,
            b.append_block_with_params(foreign.view(g), &[home_i32], "p")
                .map(|_| ()),
        ),
        (
            "append_block_with_params type",
            IrError::ForeignType,
            b.append_block_with_params(home.view(h), &[foreign_i32], "p")
                .map(|_| ()),
        ),
        (
            "append_block_with_named_params function",
            IrError::ForeignValueId,
            b.append_block_with_named_params(foreign.view(g), [(home_i32, "x")], "p")
                .map(|_| ()),
        ),
        (
            "append_block_with_named_params type",
            IrError::ForeignType,
            b.append_block_with_named_params(home.view(h), [(foreign_i32, "x")], "p")
                .map(|_| ()),
        ),
        (
            "append_block_typed function",
            IrError::ForeignValueId,
            b.append_block_typed::<(i32,), _>(foreign.view(g), "p")
                .map(|_| ()),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected block must not mutate"
    );
    assert_eq!(
        format!("{foreign}"),
        foreign_before,
        "a rejected block must not be appended to the other module either"
    );
}

/// On-the-fly SSA construction refuses a function or a session state from
/// another module: `SsaState::for_function` refuses a function of another
/// module, and `SsaBuilder::for_function` refuses a function or a module
/// other than the state's — including a state opened in another module whose
/// function sits at the same slot, which the state's function id, module tag
/// included, now tells apart.
///
/// No upstream counterpart: llvmkit's SSA builder follows
/// `cranelift-frontend`'s `FunctionBuilder`; the nearest LLVM analogue,
/// `SSAUpdater` (`Transforms/Utils/SSAUpdater.cpp`), works on `Value *`s.
#[test]
fn ssa_construction_rejects_a_function_or_state_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    // Declared in the same order in both modules, so `f` and `g` share a slot:
    // only the module tag tells the two states apart.
    let f = home
        .add_typed_function::<(), (), _>("f", Linkage::External)
        .expect("f")
        .as_function();
    let g = foreign
        .add_typed_function::<(), (), _>("g", Linkage::External)
        .expect("g")
        .as_function();
    let mut home_state = SsaState::for_function(&home, home.view(f)).expect("home state");
    let mut foreign_state =
        SsaState::for_function(&foreign, foreign.view(g)).expect("foreign state");
    let before = format!("{home}");

    let outcomes = vec![
        (
            "SsaState::for_function",
            IrError::ForeignValueId,
            SsaState::for_function(&home, foreign.view(g)).map(|_| ()),
        ),
        (
            "SsaBuilder::for_function function",
            IrError::ForeignValueId,
            SsaBuilder::for_function(&home, foreign.view(g), &mut home_state).map(|_| ()),
        ),
        (
            "SsaBuilder::for_function module",
            IrError::ForeignValueId,
            SsaBuilder::for_function(&foreign, home.view(f), &mut home_state).map(|_| ()),
        ),
        (
            "state from another module",
            IrError::SsaForeignFunction,
            SsaBuilder::for_function(&home, home.view(f), &mut foreign_state).map(|_| ()),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected session must not mutate"
    );
}

/// An SSA session refuses a block or a variable handed out by a session of
/// another module — `SsaForeignBlock` / `SsaForeignVariable` — before the
/// block's slot, the variable's index or its type is read. Both sessions here
/// are the first of their module, so a session identity that did not include
/// the module would match and let the foreign handle through. The positive
/// control is the same session accepting its own block afterwards.
///
/// No upstream counterpart: llvmkit's SSA builder follows
/// `cranelift-frontend`'s `FunctionBuilder`; the nearest LLVM analogue,
/// `SSAUpdater` (`Transforms/Utils/SSAUpdater.cpp`), works on `Value *`s.
#[test]
fn an_ssa_session_rejects_a_block_or_variable_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let declare = |module: &Module<DynBrand>| {
        let fn_ty = module.function_type(
            module.void_type(),
            [module.bool_type().as_type(), module.i32_type().as_type()],
        );
        module
            .add_function_dyn("f", fn_ty, Linkage::External)
            .expect("function")
    };
    let f = declare(&home);
    let g = declare(&foreign);
    let mut home_state = SsaState::for_function(&home, home.view(f)).expect("home state");
    let mut foreign_state =
        SsaState::for_function(&foreign, foreign.view(g)).expect("foreign state");
    let (foreign_block, foreign_variable) = {
        let mut session = SsaBuilder::for_function(&foreign, foreign.view(g), &mut foreign_state)
            .expect("foreign session");
        let _entry = session.create_block("entry");
        (
            session.create_block("target"),
            session.declare_int_var::<i32, _>("x"),
        )
    };

    let mut session =
        SsaBuilder::for_function(&home, home.view(f), &mut home_state).expect("home session");
    let entry = session.create_block("entry");
    let target = session.create_block("target");
    session.switch_to_block(entry).expect("positioned");
    let function = home.view(f);
    let condition: IntValue<'_, bool, DynBrand> = function
        .param(0)
        .expect("i1 parameter")
        .try_into()
        .expect("i1");
    let selector: IntValue<'_, i32, DynBrand> = function
        .param(1)
        .expect("i32 parameter")
        .try_into()
        .expect("i32");
    let home_before = format!("{home}");
    let foreign_before = format!("{foreign}");
    let counts_before = (
        session.state().block_count(),
        session.state().variable_count(),
    );

    let outcomes = vec![
        (
            "seal_block",
            IrError::SsaForeignBlock,
            session.seal_block(foreign_block),
        ),
        ("br", IrError::SsaForeignBlock, session.br(foreign_block)),
        (
            "cond_br",
            IrError::SsaForeignBlock,
            session.cond_br(condition, foreign_block, target),
        ),
        (
            "switch default",
            IrError::SsaForeignBlock,
            session.switch(selector, foreign_block, [(0_i32, target)]),
        ),
        (
            "switch case",
            IrError::SsaForeignBlock,
            session.switch(selector, target, [(0_i32, foreign_block)]),
        ),
        (
            "def_int_var",
            IrError::SsaForeignVariable,
            session.def_int_var(foreign_variable, 1_i32),
        ),
        (
            "use_int_var",
            IrError::SsaForeignVariable,
            without_value(session.use_int_var(foreign_variable)),
        ),
        (
            "switch_to_block",
            IrError::SsaForeignBlock,
            session.switch_to_block(foreign_block),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(
        (
            session.state().block_count(),
            session.state().variable_count(),
        ),
        counts_before,
        "a refused handle must not change the session"
    );
    assert_eq!(
        format!("{home}"),
        home_before,
        "a refused handle must not mutate"
    );
    assert_eq!(
        format!("{foreign}"),
        foreign_before,
        "a refused handle must not mutate its own module either"
    );
    // Positive control: the session still accepts its own block.
    session
        .br(target)
        .expect("the session's own block is accepted");
}

/// `i32 name(i32 %a) { %x = add i32 %a, 1; ret i32 %x }` in `module`: a body
/// whose first instruction a pass or a split can name. Built identically in
/// two modules, the two `add`s sit at the same slot. Returns the function and
/// a storable id of its parameter.
fn function_with_an_add(
    module: &Module<DynBrand>,
    name: &str,
) -> (FunctionId<Dyn, DynBrand>, ValueId<DynBrand>) {
    let fn_ty = module.function_type(module.i32_type(), [module.i32_type().as_type()]);
    let f = module
        .add_function_dyn(name, fn_ty, Linkage::External)
        .expect("function");
    let entry = module.view(f).append_basic_block(module, "entry");
    let b = IrBuilder::new_for::<Dyn>(module).position_at_end(entry);
    let parameter = module.view(f).param(0).expect("parameter").as_erased();
    let a: IntValue<'_, i32, DynBrand> = parameter.try_into().expect("an i32");
    let x = b.int_add(a, 1i32, "x").expect("add");
    b.ret(x).expect("ret");
    (f, parameter.id())
}

/// `BasicBlock::split_at` refuses a split point from another module before
/// the block is read or a new block appended. The two functions are built
/// alike, so the foreign instruction's slot names an instruction of this
/// block too.
///
/// No upstream counterpart: `BasicBlock::splitBasicBlock`
/// (`lib/IR/BasicBlock.cpp`) takes an iterator into its own instruction list.
#[test]
fn split_at_rejects_an_instruction_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let (f, _) = function_with_an_add(&home, "f");
    let (g, _) = function_with_an_add(&foreign, "g");
    let foreign_add = foreign
        .view(g)
        .entry_block()
        .expect("entry")
        .instructions()
        .next()
        .expect("add");

    let before = format!("{home}");
    let result = home
        .view(f)
        .entry_block()
        .expect("entry")
        .split_at(&home, &foreign_add, "tail");
    assert!(
        matches!(result, Err(IrError::ForeignValueId)),
        "{:?}",
        result.as_ref().map(|_| ())
    );
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected split must not mutate"
    );
}

/// A `PatchBody` pass that replaces every use of an instruction another
/// module minted — the foreign handle a pass could smuggle in from its own
/// fields.
struct ReplaceUsesOfForeignView<'s> {
    view: InstructionView<'s, DynBrand>,
    replacement: ValueId<DynBrand>,
}

impl<'s> FunctionPass<DynBrand> for ReplaceUsesOfForeignView<'s> {
    type Access = PatchBody;
    type Requires = ();
    const NAME: &'static str = "replace-uses-of-foreign-view";

    fn run<'m, 'ctx>(
        &mut self,
        cx: FnCx<'m, '_, 'ctx, DynBrand, PatchBody, ()>,
    ) -> IrResult<FnReport<DynBrand>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let patch = cx.mutate();
        patch.replace_all_uses(&self.view, self.replacement)?;
        Ok(patch.done())
    }
}

/// A `ReshapeCfg` pass that inserts an operandless phi of a type another
/// module minted at the entry block's head.
struct InsertPhiOfForeignType<'s> {
    ty: Type<'s, DynBrand>,
}

impl<'s> FunctionPass<DynBrand> for InsertPhiOfForeignType<'s> {
    type Access = ReshapeCfg;
    type Requires = (DominatorTreeAnalysis,);
    const NAME: &'static str = "insert-phi-of-foreign-type";

    fn run<'m, 'ctx>(
        &mut self,
        cx: FnCx<'m, '_, 'ctx, DynBrand, ReshapeCfg, (DominatorTreeAnalysis,)>,
    ) -> IrResult<FnReport<DynBrand>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let mut reshape = cx.mutate();
        let entry = reshape
            .function()
            .entry_block()
            .expect("definition has an entry block")
            .id();
        reshape.insert_phi_dyn(entry, self.ty, &[])?;
        Ok(reshape.done())
    }
}

/// A pass cannot turn a handle from another module into a slot of the module
/// it runs over: `FnPatch::replace_all_uses` refuses an instruction view, and
/// `FnReshape::insert_phi_dyn` a phi type, from another module, before any
/// use is rewired or phi created. The two modules are built alike, so each
/// foreign handle's slot names something in the module under the pass.
///
/// No upstream counterpart: `Value::replaceAllUsesWith` (`lib/IR/Value.cpp`)
/// and `PHINode::Create` (`IR/Instructions.h`) take a `Value *` and a
/// `Type *` uniqued per `LLVMContext`.
#[test]
fn a_pass_rejects_an_instruction_or_type_from_another_module() {
    let foreign = Module::dynamic("foreign");
    let (g, _) = function_with_an_add(&foreign, "g");
    let foreign_add = foreign
        .view(g)
        .entry_block()
        .expect("entry")
        .instructions()
        .next()
        .expect("add");
    let foreign_i32 = foreign.i32_type().as_type();

    let patched = Module::dynamic("patched");
    let (patched_f, replacement) = function_with_an_add(&patched, "f");
    let reshaped = Module::dynamic("reshaped");
    let (reshaped_f, _) = function_with_an_add(&reshaped, "f");
    let mut analyses = Analyses::new();

    let outcomes = vec![
        (
            "FnPatch::replace_all_uses",
            IrError::ForeignValueId,
            run_function_pass(
                ReplaceUsesOfForeignView {
                    view: foreign_add,
                    replacement,
                },
                patched.verify().expect("verifies"),
                patched_f,
                &mut analyses,
            )
            .map(|_| ()),
        ),
        (
            "FnReshape::insert_phi_dyn",
            IrError::ForeignType,
            run_function_pass(
                InsertPhiOfForeignType { ty: foreign_i32 },
                reshaped.verify().expect("verifies"),
                reshaped_f,
                &mut analyses,
            )
            .map(|_| ()),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
}

/// Rewiring or relocating an instruction refuses a handle from another module
/// before anything is read or moved: `Instruction::replace_all_uses_with`
/// (the replacement), `move_before` / `move_after` and the detached
/// `insert_before` / `insert_after` (the anchor), and `append_to` (the
/// block).
///
/// No upstream counterpart: `Value::replaceAllUsesWith` (`lib/IR/Value.cpp`)
/// takes a `Value *`, and `Instruction::moveBefore` / `moveAfter` /
/// `insertBefore` / `insertAfter` / `insertInto` (`lib/IR/Instruction.cpp`)
/// an `Instruction *` or `BasicBlock *`.
#[test]
fn an_instruction_move_rejects_a_handle_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let (g, _) = function_with_an_add(&foreign, "g");
    let foreign_add = foreign
        .view(g)
        .entry_block()
        .expect("entry")
        .instructions()
        .next()
        .expect("add");
    let foreign_value = foreign.i32_type().const_int(5i32).as_erased();
    let foreign_block = open_block(&foreign, "h");
    // An `i32` instruction with a user, so an unadmitted `i32` replacement
    // would pass the type check and be written into the user's operand.
    let (replacing, [narrow, ..]) = builder_with_parameters(&home, "f1");
    let narrow: IntValue<'_, i32, DynBrand> = narrow.try_into().expect("an i32");
    let x = replacing.int_add(narrow, 1i32, "x").expect("add");
    replacing.int_add(x, 2i32, "y").expect("a user of x");
    let (replaced, _) = BlockCursor::at_start(replacing.into_insert_block())
        .step()
        .expect("the add");
    let zero = home.i32_type().const_int(0i32);
    let ret = |name: &str| builder(&home, name).ret(zero).expect("ret").1;
    let moved_before = ret("f2");
    let moved_after = ret("f3");
    let inserted_before = ret("f4").detach_from_parent(&home);
    let inserted_after = ret("f5").detach_from_parent(&home);
    let appended = ret("f6").detach_from_parent(&home);
    let before = format!("{home}");

    let outcomes = vec![
        (
            "replace_all_uses_with",
            IrError::ForeignValueId,
            replaced.replace_all_uses_with(&home, foreign_value),
        ),
        (
            "move_before",
            IrError::ForeignValueId,
            moved_before.move_before(&home, &foreign_add),
        ),
        (
            "move_after",
            IrError::ForeignValueId,
            moved_after.move_after(&home, &foreign_add),
        ),
        (
            "insert_before",
            IrError::ForeignValueId,
            inserted_before
                .insert_before(&home, &foreign_add)
                .map(|_| ()),
        ),
        (
            "insert_after",
            IrError::ForeignValueId,
            inserted_after.insert_after(&home, &foreign_add).map(|_| ()),
        ),
        (
            "append_to",
            IrError::ForeignValueId,
            appended.append_to(&home, &foreign_block).map(|_| ()),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(format!("{home}"), before, "a rejected move must not mutate");
}

/// The operand, clause and function-data setters refuse a value or constant
/// from another module before anything is stored:
/// `AtomicRmwInst::set_value_operand`, the width-erased `SwitchInst::add_case`,
/// `LandingPadInst::add_catch_clause` / `add_filter_clause`, and
/// `FunctionValue::set_prefix_data` / `set_prologue_data` /
/// `set_personality_fn`.
///
/// No upstream counterpart: `AtomicRMWInst::setOperand`,
/// `SwitchInst::addCase` and `LandingPadInst::addClause` (`IR/Instructions.h`)
/// take `Value *` / `Constant *` operands, and `Function::setPrefixData` /
/// `setPrologueData` / `setPersonalityFn` (`lib/IR/Function.cpp`) a
/// `Constant *`.
#[test]
fn an_operand_or_clause_setter_rejects_a_handle_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let foreign_value = foreign.i32_type().const_int(5i32);
    let foreign_null = foreign.ptr_type(0).const_null();
    let (b, [narrow, _, _, _, pointer]) = builder_with_parameters(&home, "f");
    let narrow: IntValue<'_, i32, DynBrand> = narrow.try_into().expect("an i32");
    let pointer: PointerValue<'_, DynBrand> = pointer.try_into().expect("a pointer");
    let rmw = b
        .atomicrmw(
            AtomicRmwBinOp::Add,
            pointer,
            narrow,
            AtomicRmwConfig::new(AtomicOrdering::Monotonic, SyncScope::System),
            "old",
        )
        .expect("atomicrmw");
    let catch_pad = b
        .landingpad(home.ptr_type(0).as_type(), false, "catch")
        .expect("landingpad");
    let filter_pad = b
        .landingpad(home.ptr_type(0).as_type(), false, "filter")
        .expect("landingpad");
    let (switcher, default, case_target) = builder_with_targets(&home, "s");
    let (_, switch) = switcher
        .switch_dyn(home.i32_type().const_int(0i32), default, "")
        .expect("switch");
    let h_ty = home.function_type_no_parameters(home.i32_type());
    let h = home
        .add_function_dyn("h", h_ty, Linkage::External)
        .expect("h");
    let before = format!("{home}");

    let outcomes = vec![
        (
            "AtomicRmwInst::set_value_operand",
            IrError::ForeignValueId,
            home.view(rmw)
                .set_value_operand(&home, foreign_value.as_erased()),
        ),
        (
            "SwitchInst::add_case",
            IrError::ForeignValueId,
            switch.add_case(foreign_value, case_target).map(|_| ()),
        ),
        (
            "LandingPadInst::add_catch_clause",
            IrError::ForeignValueId,
            catch_pad.add_catch_clause(foreign_null).map(|_| ()),
        ),
        (
            "LandingPadInst::add_filter_clause",
            IrError::ForeignValueId,
            filter_pad.add_filter_clause(foreign_null).map(|_| ()),
        ),
        (
            "FunctionValue::set_prefix_data",
            IrError::ForeignValueId,
            home.view(h).set_prefix_data(&home, foreign_value),
        ),
        (
            "FunctionValue::set_prologue_data",
            IrError::ForeignValueId,
            home.view(h).set_prologue_data(&home, foreign_value),
        ),
        (
            "FunctionValue::set_personality_fn",
            IrError::ForeignValueId,
            home.view(h).set_personality_fn(&home, foreign_null),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(
        format!("{home}"),
        before,
        "a rejected setter must not mutate"
    );
}

/// Every public constant-fold entry that takes more than one handle refuses a
/// later one minted by another module before it reads either: the
/// target-independent folds of `constant_fold`, the `DataLayout`-aware folds
/// of `constant_folding`, and `lossless_inv_cast` with its two truncation
/// forwarders. Each fold builds in its first handle's module, so the foreign
/// handle is always a later argument.
///
/// No upstream counterpart: `llvm/lib/IR/ConstantFold.cpp` and
/// `llvm/lib/Analysis/ConstantFolding.cpp` take `Constant *` operands and
/// `Type *`s uniqued per `LLVMContext`.
#[test]
fn a_constant_fold_rejects_an_operand_or_type_from_another_module() {
    use llvmkit_ir::constant_folding::constant_fold_call;
    use llvmkit_ir::{
        ApInt, BinaryIntrinsic, BinaryOpcode, CmpPredicate, DataLayout, DenormalMode,
        FastMathFlags, FoldNonDeterminism, IntPredicate, LibFunc, Signedness, TargetLibraryInfo,
        constant_fold_binary_instruction, constant_fold_binary_intrinsic,
        constant_fold_binary_op_operands, constant_fold_cast_instruction,
        constant_fold_cast_operand, constant_fold_compare_inst_operands,
        constant_fold_compare_instruction, constant_fold_extract_element_instruction,
        constant_fold_fp_inst_operands, constant_fold_get_element_ptr,
        constant_fold_insert_element_instruction, constant_fold_insert_value_instruction,
        constant_fold_inst_operands, constant_fold_integer_cast, constant_fold_load_from_const,
        constant_fold_load_from_const_ptr, constant_fold_load_from_uniform_value,
        constant_fold_load_through_bitcast, constant_fold_select_instruction,
        constant_fold_shuffle_vector_instruction, lossless_inv_cast, lossless_signed_trunc,
        lossless_unsigned_trunc,
    };

    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let one = home.i32_type().const_int(1i32).as_constant();
    let two = foreign.i32_type().const_int(2i32).as_constant();
    let null = home.ptr_type(0).const_null().as_constant();
    let home_i32 = home.i32_type().as_type();
    let foreign_i32 = foreign.i32_type().as_type();
    let foreign_i64 = foreign.i64_type().as_type();
    let foreign_i8 = foreign.i8_type().as_type();
    let home_double = home.f64_type().const_double(4.0).as_constant();
    let foreign_double = foreign.f64_type().const_double(4.0).as_constant();
    let (f, _) = function_with_an_add(&home, "f");
    let add = home
        .view(f)
        .entry_block()
        .expect("entry")
        .instructions()
        .next()
        .expect("add");
    let dl = DataLayout::default();
    let tli = TargetLibraryInfo::default();
    let equal: CmpPredicate = IntPredicate::Eq.into();
    let before = (format!("{home}"), format!("{foreign}"));

    let outcomes = vec![
        (
            "constant_fold_binary_instruction",
            IrError::ForeignValueId,
            without_value(constant_fold_binary_instruction(
                BinaryOpcode::Add,
                one,
                two,
            )),
        ),
        (
            "constant_fold_cast_instruction",
            IrError::ForeignType,
            without_value(constant_fold_cast_instruction(
                CastOpcode::Zext,
                one,
                foreign_i64,
            )),
        ),
        (
            "constant_fold_compare_instruction",
            IrError::ForeignValueId,
            without_value(constant_fold_compare_instruction(equal, one, two)),
        ),
        (
            "constant_fold_select_instruction",
            IrError::ForeignValueId,
            without_value(constant_fold_select_instruction(one, two, one)),
        ),
        (
            "constant_fold_extract_element_instruction",
            IrError::ForeignValueId,
            without_value(constant_fold_extract_element_instruction(one, two)),
        ),
        (
            "constant_fold_insert_element_instruction",
            IrError::ForeignValueId,
            without_value(constant_fold_insert_element_instruction(one, one, two)),
        ),
        (
            "constant_fold_shuffle_vector_instruction",
            IrError::ForeignValueId,
            without_value(constant_fold_shuffle_vector_instruction(one, two, &[])),
        ),
        (
            "constant_fold_insert_value_instruction",
            IrError::ForeignValueId,
            without_value(constant_fold_insert_value_instruction(one, two, &[])),
        ),
        (
            "constant_fold_get_element_ptr (source type)",
            IrError::ForeignType,
            without_value(constant_fold_get_element_ptr(
                foreign_i32,
                null,
                &[one],
                None,
            )),
        ),
        (
            "constant_fold_get_element_ptr (index)",
            IrError::ForeignValueId,
            without_value(constant_fold_get_element_ptr(home_i32, null, &[two], None)),
        ),
        (
            "constant_fold_load_from_const_ptr",
            IrError::ForeignType,
            without_value(constant_fold_load_from_const_ptr(
                null,
                foreign_i32,
                ApInt::zero(64),
                &dl,
            )),
        ),
        (
            "constant_fold_load_from_const",
            IrError::ForeignType,
            without_value(constant_fold_load_from_const(
                one,
                foreign_i32,
                ApInt::zero(64),
                &dl,
            )),
        ),
        (
            "constant_fold_load_from_uniform_value",
            IrError::ForeignType,
            without_value(constant_fold_load_from_uniform_value(one, foreign_i32, &dl)),
        ),
        (
            "constant_fold_cast_operand",
            IrError::ForeignType,
            without_value(constant_fold_cast_operand(
                CastOpcode::Zext,
                one,
                foreign_i64,
                &dl,
            )),
        ),
        (
            "constant_fold_integer_cast",
            IrError::ForeignType,
            without_value(constant_fold_integer_cast(
                one,
                foreign_i64,
                Signedness::Signed,
                &dl,
            )),
        ),
        (
            "constant_fold_inst_operands",
            IrError::ForeignValueId,
            without_value(constant_fold_inst_operands(
                &add,
                &[two, one],
                &dl,
                None,
                FoldNonDeterminism::Allow,
            )),
        ),
        (
            "constant_fold_compare_inst_operands",
            IrError::ForeignValueId,
            without_value(constant_fold_compare_inst_operands(
                equal, one, two, &dl, None,
            )),
        ),
        (
            "constant_fold_binary_op_operands",
            IrError::ForeignValueId,
            without_value(constant_fold_binary_op_operands(
                BinaryOpcode::Add,
                one,
                two,
                &dl,
            )),
        ),
        (
            "constant_fold_fp_inst_operands",
            IrError::ForeignValueId,
            without_value(constant_fold_fp_inst_operands(
                BinaryOpcode::Fadd,
                home_double,
                foreign_double,
                &dl,
                DenormalMode::default(),
                FastMathFlags::default(),
                FoldNonDeterminism::Allow,
            )),
        ),
        (
            "constant_fold_binary_intrinsic (operand)",
            IrError::ForeignValueId,
            without_value(constant_fold_binary_intrinsic(
                BinaryIntrinsic::Umax,
                one,
                two,
                home_i32,
                &dl,
            )),
        ),
        (
            "constant_fold_binary_intrinsic (type)",
            IrError::ForeignType,
            without_value(constant_fold_binary_intrinsic(
                BinaryIntrinsic::Umax,
                one,
                one,
                foreign_i32,
                &dl,
            )),
        ),
        (
            "constant_fold_load_through_bitcast",
            IrError::ForeignType,
            without_value(constant_fold_load_through_bitcast(one, foreign_i32, &dl)),
        ),
        (
            "constant_fold_call",
            IrError::ForeignValueId,
            without_value(constant_fold_call(
                LibFunc::Sqrt,
                &[foreign_double],
                home.f64_type().as_type(),
                &tli,
                FoldNonDeterminism::Allow,
            )),
        ),
        // A narrower type, so an ungated call folds the truncation instead
        // of reaching `constant_expr`'s gate with an invalid one.
        (
            "lossless_inv_cast",
            IrError::ForeignType,
            without_value(lossless_inv_cast(one, foreign_i8, CastOpcode::Zext, &dl)),
        ),
        (
            "lossless_unsigned_trunc",
            IrError::ForeignType,
            without_value(lossless_unsigned_trunc(one, foreign_i8, &dl)),
        ),
        (
            "lossless_signed_trunc",
            IrError::ForeignType,
            without_value(lossless_signed_trunc(one, foreign_i8, &dl)),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(
        (format!("{home}"), format!("{foreign}")),
        before,
        "a rejected fold must not mutate"
    );
}

/// Every `ConstantFolder` hook that takes more than one handle refuses a later
/// one minted by another module before it reads either, so a direct call
/// cannot pair two modules' values or types. The foreign handle is a
/// parameter, which no fold can fold: an ungated hook declines with `Ok(None)`
/// rather than reaching a module constructor that would refuse it for the
/// hook.
///
/// No upstream counterpart: `llvm/include/llvm/IR/ConstantFolder.h` takes
/// `Value *` operands and `Type *`s uniqued per `LLVMContext`.
#[test]
fn the_constant_folder_rejects_an_operand_or_type_from_another_module() {
    use llvmkit_ir::{
        BinaryIntrinsic, BinaryOpcode, CmpPredicate, ConstantFolder, ExactFlags, FastMathFlags,
        FloatPredicate, IntPredicate, IrBuilderFolder, OverflowFlags,
    };

    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let (_, [h_i32, h_i64, h_f32, _, h_ptr]) = builder_with_parameters(&home, "f");
    let (_, [f_i32, f_i64, f_f32, _, _]) = builder_with_parameters(&foreign, "g");
    let h_int: IntValue<'_, i32, DynBrand> = h_i32.try_into().expect("an i32");
    let f_int: IntValue<'_, i32, DynBrand> = f_i32.try_into().expect("an i32");
    let h_float: FloatValue<'_, f32, DynBrand> = h_f32.try_into().expect("a float");
    let f_float: FloatValue<'_, f32, DynBrand> = f_f32.try_into().expect("a float");
    let null = home.ptr_type(0).const_null().as_constant();
    let home_i32 = home.i32_type().as_type();
    let foreign_i32 = foreign.i32_type().as_type();
    let foreign_i64 = foreign.i64_type().as_type();
    let foreign_float = foreign.f32_type().as_type();
    let equal: CmpPredicate = IntPredicate::Eq.into();
    let folder = ConstantFolder;
    let before = (format!("{home}"), format!("{foreign}"));

    let outcomes = vec![
        (
            "fold_bin_op_dyn",
            IrError::ForeignValueId,
            without_value(folder.fold_bin_op_dyn(BinaryOpcode::Add, h_i32, f_i32)),
        ),
        (
            "fold_exact_bin_op_dyn",
            IrError::ForeignValueId,
            without_value(folder.fold_exact_bin_op_dyn(
                BinaryOpcode::Udiv,
                h_i32,
                f_i32,
                ExactFlags::default(),
            )),
        ),
        (
            "fold_no_wrap_bin_op_dyn",
            IrError::ForeignValueId,
            without_value(folder.fold_no_wrap_bin_op_dyn(
                BinaryOpcode::Add,
                h_i32,
                f_i32,
                OverflowFlags::default(),
            )),
        ),
        (
            "fold_bin_op_fmf_dyn",
            IrError::ForeignValueId,
            without_value(folder.fold_bin_op_fmf_dyn(
                BinaryOpcode::Fadd,
                h_f32,
                f_f32,
                FastMathFlags::default(),
            )),
        ),
        (
            "fold_cmp_dyn",
            IrError::ForeignValueId,
            without_value(folder.fold_cmp_dyn(equal, h_i32, f_i32)),
        ),
        (
            "fold_gep_dyn (source type)",
            IrError::ForeignType,
            without_value(folder.fold_gep_dyn(
                foreign_i32,
                h_ptr,
                &[h_i64],
                GepNoWrapFlags::default(),
            )),
        ),
        (
            "fold_gep_dyn (index)",
            IrError::ForeignValueId,
            without_value(folder.fold_gep_dyn(
                home_i32,
                h_ptr,
                &[f_i64],
                GepNoWrapFlags::default(),
            )),
        ),
        (
            "fold_select_dyn",
            IrError::ForeignValueId,
            without_value(folder.fold_select_dyn(h_i32, f_i32, h_i32)),
        ),
        (
            "fold_insert_value_dyn",
            IrError::ForeignValueId,
            without_value(folder.fold_insert_value_dyn(h_i32, f_i32, &[0])),
        ),
        (
            "fold_extract_element_dyn",
            IrError::ForeignValueId,
            without_value(folder.fold_extract_element_dyn(h_i32, f_i32)),
        ),
        (
            "fold_insert_element_dyn",
            IrError::ForeignValueId,
            without_value(folder.fold_insert_element_dyn(h_i32, f_i32, h_i32)),
        ),
        (
            "fold_shuffle_vector_dyn",
            IrError::ForeignValueId,
            without_value(folder.fold_shuffle_vector_dyn(h_i32, f_i32, &[])),
        ),
        (
            "fold_cast_dyn",
            IrError::ForeignType,
            without_value(folder.fold_cast_dyn(CastOpcode::Zext, h_i32, foreign_i64)),
        ),
        (
            "fold_binary_intrinsic_dyn (operand)",
            IrError::ForeignValueId,
            without_value(folder.fold_binary_intrinsic_dyn(
                BinaryIntrinsic::Umax,
                h_i32,
                f_i32,
                home_i32,
            )),
        ),
        (
            "fold_binary_intrinsic_dyn (type)",
            IrError::ForeignType,
            without_value(folder.fold_binary_intrinsic_dyn(
                BinaryIntrinsic::Umax,
                h_i32,
                h_i32,
                foreign_i32,
            )),
        ),
        // A non-pointer destination, so an ungated hook fails its own
        // pointer-cast check rather than reaching `constant_expr`'s gate.
        (
            "create_pointer_cast",
            IrError::ForeignType,
            without_value(folder.create_pointer_cast(null, foreign_float)),
        ),
        (
            "create_pointer_bitcast_or_addrspace_cast",
            IrError::ForeignType,
            without_value(folder.create_pointer_bitcast_or_addrspace_cast(null, foreign_float)),
        ),
        (
            "fold_int_bin_op",
            IrError::ForeignValueId,
            without_value(folder.fold_int_bin_op(BinaryOpcode::Add, h_int, f_int)),
        ),
        (
            "fold_int_bin_op_no_wrap",
            IrError::ForeignValueId,
            without_value(folder.fold_int_bin_op_no_wrap(
                BinaryOpcode::Add,
                h_int,
                f_int,
                OverflowFlags::default(),
            )),
        ),
        (
            "fold_int_bin_op_exact",
            IrError::ForeignValueId,
            without_value(folder.fold_int_bin_op_exact(
                BinaryOpcode::Udiv,
                h_int,
                f_int,
                ExactFlags::default(),
            )),
        ),
        (
            "fold_fp_bin_op",
            IrError::ForeignValueId,
            without_value(folder.fold_fp_bin_op(
                BinaryOpcode::Fadd,
                h_float,
                f_float,
                FastMathFlags::default(),
            )),
        ),
        (
            "fold_int_cmp",
            IrError::ForeignValueId,
            without_value(folder.fold_int_cmp(IntPredicate::Eq, h_int, f_int)),
        ),
        (
            "fold_fp_cmp",
            IrError::ForeignValueId,
            without_value(folder.fold_fp_cmp(FloatPredicate::Oeq, h_float, f_float)),
        ),
        (
            "fold_cast_to_int",
            IrError::ForeignType,
            without_value(folder.fold_cast_to_int(CastOpcode::Trunc, h_i64, foreign.i32_type())),
        ),
        (
            "fold_cast_to_fp",
            IrError::ForeignType,
            without_value(folder.fold_cast_to_fp(CastOpcode::FpExt, h_f32, foreign.f64_type())),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(
        (format!("{home}"), format!("{foreign}")),
        before,
        "a rejected fold must not mutate"
    );
}

/// Moving, reopening or splicing a block refuses a block of another module
/// before either block is read: `FunctionValue::move_basic_block_to_end`,
/// `FunctionValue::basic_block_for_construction` and `BasicBlock::splice_into`
/// (the destination). The two functions are built alike, so the foreign
/// block's slot and its parent's slot both name something real here.
///
/// No upstream counterpart: `Function::splice` and `BasicBlock::splice`
/// (`lib/IR/Function.cpp`, `lib/IR/BasicBlock.cpp`) take `BasicBlock *`s.
#[test]
fn a_block_move_or_splice_rejects_a_block_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let (f, _) = function_with_an_add(&home, "f");
    let (g, _) = function_with_an_add(&foreign, "g");
    let foreign_entry = || foreign.view(g).entry_block().expect("entry");
    let home_entry = home.view(f).entry_block().expect("entry");
    let before = (format!("{home}"), format!("{foreign}"));

    let outcomes = vec![
        (
            "FunctionValue::move_basic_block_to_end",
            IrError::ForeignValueId,
            home.view(f).move_basic_block_to_end(&home, foreign_entry()),
        ),
        (
            "FunctionValue::basic_block_for_construction",
            IrError::ForeignValueId,
            without_value(
                home.view(f)
                    .basic_block_for_construction(&home, foreign_entry().to_erased()),
            ),
        ),
        (
            "BasicBlock::splice_into",
            IrError::ForeignValueId,
            home_entry.splice_into(&home, foreign_entry()),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(
        (format!("{home}"), format!("{foreign}")),
        before,
        "a rejected move must not mutate"
    );
}

/// `GlobalVariable::try_delta_from` and `try_delta_from_plus` refuse the other
/// global when it belongs to another module, instead of interning its slot
/// into a constant of this one.
///
/// No upstream counterpart: the delta is llvmkit's own spelling of
/// `ConstantExpr::getSub` over two `ptrtoint`s (`lib/IR/Constants.cpp`), whose
/// operands are `Constant *`s.
#[test]
fn a_symbol_delta_rejects_a_global_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let real = home
        .add_global("real", home.i32_type().const_int(1i32))
        .expect("global");
    let anchor = foreign
        .add_global("anchor", foreign.i32_type().const_int(2i32))
        .expect("global");
    let before = (format!("{home}"), format!("{foreign}"));

    let outcomes = vec![
        (
            "GlobalVariable::try_delta_from",
            IrError::ForeignValueId,
            without_value(home.view(real).try_delta_from(foreign.view(anchor))),
        ),
        (
            "GlobalVariable::try_delta_from_plus",
            IrError::ForeignValueId,
            without_value(home.view(real).try_delta_from_plus(foreign.view(anchor), 7)),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(
        (format!("{home}"), format!("{foreign}")),
        before,
        "a rejected delta must not mutate"
    );
}

/// `FunctionBuilder::build` refuses a signature or a prefix, prologue or
/// personality constant of another module, parked by the infallible setters,
/// before the function is created.
///
/// No upstream counterpart: `Function::Create` takes a `FunctionType *`
/// uniqued per `LLVMContext`, and `Function::setPrefixData` /
/// `setPrologueData` / `setPersonalityFn` (`lib/IR/Function.cpp`) a
/// `Constant *`.
#[test]
fn a_function_builder_rejects_a_signature_or_constant_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let home_ty = home.function_type_no_parameters(home.i32_type());
    let foreign_ty = foreign.function_type_no_parameters(foreign.i32_type());
    let foreign_constant = foreign.i32_type().const_int(5i32);
    let foreign_null = foreign.ptr_type(0).const_null();
    let before = (format!("{home}"), format!("{foreign}"));

    let outcomes = vec![
        (
            "signature",
            IrError::ForeignType,
            without_value(home.function_builder::<Dyn, _>("a", foreign_ty).build()),
        ),
        (
            "prefix_data",
            IrError::ForeignValueId,
            without_value(
                home.function_builder::<Dyn, _>("b", home_ty)
                    .prefix_data(foreign_constant)
                    .build(),
            ),
        ),
        (
            "prologue_data",
            IrError::ForeignValueId,
            without_value(
                home.function_builder::<Dyn, _>("c", home_ty)
                    .prologue_data(foreign_constant)
                    .build(),
            ),
        ),
        (
            "personality_fn",
            IrError::ForeignValueId,
            without_value(
                home.function_builder::<Dyn, _>("d", home_ty)
                    .personality_fn(foreign_null)
                    .build(),
            ),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(
        (format!("{home}"), format!("{foreign}")),
        before,
        "a rejected build must not create a function"
    );
}

/// `must_trigger_ub`'s known-poison set holds tagged value ids, and a value of
/// another module is never one of the instruction's operands — even at the
/// operand's slot. The home divisor is the positive control.
///
/// No upstream counterpart: `llvm::mustTriggerUB`
/// (`lib/Analysis/ValueTracking.cpp`) takes a `SmallPtrSetImpl<const Value *>`,
/// whose members are compared by address.
#[test]
fn must_trigger_ub_matches_a_known_poison_value_in_its_own_module_only() {
    use llvmkit_ir::must_trigger_ub;
    use std::collections::HashSet;

    /// `udiv i32 %0, %0` in a fresh `name`; returns the divisor `%0`.
    fn udiv_by_parameter<'m>(module: &'m Module<DynBrand>, name: &str) -> Value<'m, DynBrand> {
        let (b, [divisor, ..]) = builder_with_parameters(module, name);
        let divisor_int: IntValue<'_, i32, DynBrand> = divisor.try_into().expect("an i32");
        b.int_udiv(divisor_int, divisor_int, "q").expect("udiv");
        divisor
    }
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let home_divisor = udiv_by_parameter(&home, "f");
    let foreign_divisor = udiv_by_parameter(&foreign, "f");
    let udiv = home_divisor.users().next().expect("the udiv");

    assert!(
        must_trigger_ub(&udiv, &HashSet::from([home_divisor.id()])),
        "positive control: a poison divisor of this udiv is UB"
    );
    assert!(
        !must_trigger_ub(&udiv, &HashSet::from([foreign_divisor.id()])),
        "a divisor of another module is not this udiv's operand"
    );
}

use llvmkit_ir::{
    BinaryOpcode, FastMathFlags, FloatKind, FloatType, IntBinOpFlags, IntPredicate, IntType,
    IntWidth, IrBuilderFolder, OverflowFlags,
};

/// A folder whose erased hooks answer with a value another module minted.
/// Its typed hooks keep the trait's defaults, which narrow that answer.
struct ErasedAnswerFolder<'m> {
    answer: Value<'m, DynBrand>,
}

impl<'m> IrBuilderFolder<'m, DynBrand> for ErasedAnswerFolder<'m> {
    fn fold_bin_op_dyn(
        &self,
        _: BinaryOpcode,
        _: Value<'m, DynBrand>,
        _: Value<'m, DynBrand>,
    ) -> IrResult<Option<Value<'m, DynBrand>>> {
        Ok(Some(self.answer))
    }

    fn fold_no_wrap_bin_op_dyn(
        &self,
        _: BinaryOpcode,
        _: Value<'m, DynBrand>,
        _: Value<'m, DynBrand>,
        _: OverflowFlags,
    ) -> IrResult<Option<Value<'m, DynBrand>>> {
        Ok(Some(self.answer))
    }

    fn fold_bin_op_fmf_dyn(
        &self,
        _: BinaryOpcode,
        _: Value<'m, DynBrand>,
        _: Value<'m, DynBrand>,
        _: FastMathFlags,
    ) -> IrResult<Option<Value<'m, DynBrand>>> {
        Ok(Some(self.answer))
    }

    fn fold_cast_dyn(
        &self,
        _: CastOpcode,
        _: Value<'m, DynBrand>,
        _: Type<'m, DynBrand>,
    ) -> IrResult<Option<Value<'m, DynBrand>>> {
        Ok(Some(self.answer))
    }
}

/// A folder whose typed hooks, overridden natively, answer with values
/// another module minted — narrowed to the hook's own marker, so no default
/// narrow runs and the builder's acceptance is the only check.
struct TypedAnswerFolder<'m> {
    int32: Value<'m, DynBrand>,
    int64: Value<'m, DynBrand>,
    float: Value<'m, DynBrand>,
    double: Value<'m, DynBrand>,
    boolean: Value<'m, DynBrand>,
}

impl<'m> IrBuilderFolder<'m, DynBrand> for TypedAnswerFolder<'m> {
    fn fold_int_bin_op<W: IntWidth>(
        &self,
        _: BinaryOpcode,
        _: IntValue<'m, W, DynBrand>,
        _: IntValue<'m, W, DynBrand>,
    ) -> IrResult<Option<IntValue<'m, W, DynBrand>>> {
        W::narrow(self.int32).map(Some)
    }

    fn fold_int_bin_op_no_wrap<W: IntWidth>(
        &self,
        _: BinaryOpcode,
        _: IntValue<'m, W, DynBrand>,
        _: IntValue<'m, W, DynBrand>,
        _: OverflowFlags,
    ) -> IrResult<Option<IntValue<'m, W, DynBrand>>> {
        W::narrow(self.int32).map(Some)
    }

    fn fold_fp_bin_op<K: FloatKind>(
        &self,
        _: BinaryOpcode,
        _: FloatValue<'m, K, DynBrand>,
        _: FloatValue<'m, K, DynBrand>,
        _: FastMathFlags,
    ) -> IrResult<Option<FloatValue<'m, K, DynBrand>>> {
        K::narrow(self.float).map(Some)
    }

    fn fold_int_cmp<W: IntWidth>(
        &self,
        _: IntPredicate,
        _: IntValue<'m, W, DynBrand>,
        _: IntValue<'m, W, DynBrand>,
    ) -> IrResult<Option<IntValue<'m, bool, DynBrand>>> {
        <bool as IntWidth>::narrow(self.boolean).map(Some)
    }

    fn fold_cast_to_int<W: IntWidth>(
        &self,
        _: CastOpcode,
        _: Value<'m, DynBrand>,
        _: IntType<'m, W, DynBrand>,
    ) -> IrResult<Option<IntValue<'m, W, DynBrand>>> {
        W::narrow(self.int64).map(Some)
    }

    fn fold_cast_to_fp<K: FloatKind>(
        &self,
        _: CastOpcode,
        _: Value<'m, DynBrand>,
        _: FloatType<'m, K, DynBrand>,
    ) -> IrResult<Option<FloatValue<'m, K, DynBrand>>> {
        K::narrow(self.double).map(Some)
    }
}

/// A positioned builder over a custom folder `F`.
type FoldingBuilder<'m, F> = IrBuilder<'m, 'm, DynBrand, F, Positioned, Dyn>;

/// `i32 name(i32, i64, float, double, ptr)` in `module` with a builder over
/// `folder` at its entry, plus the `i32` and `float` parameters, typed.
fn folding_builder<'m, F: IrBuilderFolder<'m, DynBrand>>(
    module: &'m Module<DynBrand>,
    name: &str,
    folder: F,
) -> (
    FoldingBuilder<'m, F>,
    IntValue<'m, i32, DynBrand>,
    FloatValue<'m, f32, DynBrand>,
) {
    let (b, [narrow, _, single, _, _]) = builder_with_parameters(module, name);
    let block = b.into_insert_block();
    (
        IrBuilder::with_folder(module, folder).position_at_end(block),
        narrow.try_into().expect("an i32"),
        single.try_into().expect("a float"),
    )
}

/// A fold result a custom folder hands back is refused when another module
/// minted it, instead of being returned as this builder's result: through
/// the builder's own acceptance (`checked_folded_value` for the erased
/// entries, the typed `accept_folded_*` for natively overridden typed hooks,
/// and the compare acceptance) and through the default typed hooks' narrow.
///
/// No upstream counterpart: `IRBuilderFolder`'s hooks
/// (`llvm/include/llvm/IR/IRBuilderFolder.h`) return a `Value *`, which
/// carries its own identity.
#[test]
fn a_builder_rejects_a_fold_result_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let int32 = foreign.i32_type().const_int(9i32).as_erased();
    let erased = ErasedAnswerFolder { answer: int32 };
    let typed = TypedAnswerFolder {
        int32,
        int64: foreign.i64_type().const_int(9i64).as_erased(),
        float: foreign.f32_type().const_float(1.0).as_erased(),
        double: foreign.f64_type().const_double(1.0).as_erased(),
        boolean: foreign.bool_type().const_int(true).as_erased(),
    };
    let (e, e_int, e_float) = folding_builder(&home, "e", erased);
    let (t, t_int, t_float) = folding_builder(&home, "t", typed);
    let before = format!("{home}");

    let outcomes = vec![
        (
            "checked_folded_value (int_binop_erased)",
            IrError::ForeignValueId,
            without_value(e.int_binop_erased(
                BinaryOpcode::Add,
                e_int,
                e_int,
                IntBinOpFlags::default(),
                "x",
            )),
        ),
        (
            "narrow_folded_int (int_add, default hook)",
            IrError::ForeignValueId,
            without_value(e.int_add(e_int, e_int, "x")),
        ),
        (
            "narrow_folded_fp (fp_add, default hook)",
            IrError::ForeignValueId,
            without_value(e.fp_add(e_float, e_float, "x")),
        ),
        (
            "narrow_folded_cast_int (zext, default hook)",
            IrError::ForeignValueId,
            without_value(e.zext::<i32, i64, _, _>(e_int, home.i64_type(), "x")),
        ),
        (
            "narrow_folded_cast_fp (fp_ext, default hook)",
            IrError::ForeignValueId,
            without_value(e.fp_ext::<f32, f64, _, _>(e_float, home.f64_type(), "x")),
        ),
        (
            "accept_folded_int (int_add, native hook)",
            IrError::ForeignValueId,
            without_value(t.int_add(t_int, t_int, "x")),
        ),
        (
            "accept_folded_fp (fp_add, native hook)",
            IrError::ForeignValueId,
            without_value(t.fp_add(t_float, t_float, "x")),
        ),
        (
            "accept_folded_cast_int (zext, native hook)",
            IrError::ForeignValueId,
            without_value(t.zext::<i32, i64, _, _>(t_int, home.i64_type(), "x")),
        ),
        (
            "accept_folded_cast_fp (fp_ext, native hook)",
            IrError::ForeignValueId,
            without_value(t.fp_ext::<f32, f64, _, _>(t_float, home.f64_type(), "x")),
        ),
        (
            "accept_folded_compare (int_cmp, native hook)",
            IrError::ForeignValueId,
            without_value(t.int_cmp(IntPredicate::Eq, t_int, t_int, "x")),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert_eq!(format!("{home}"), before, "a refused fold must not mutate");
}

/// The fallible type entries refuse a type of another module before anything
/// is read, set or declared: `set_struct_body` / `set_struct_body_dyn` (the
/// struct and its elements), `add_function_dyn` (the signature),
/// `intrinsic_descriptor_from_signature` (the function type),
/// `get_or_insert_intrinsic_declaration_by_id` (an overload type), and the
/// intrinsic signature builders `IntrinsicDescriptor::function_type`,
/// `IntrinsicId::function_type` (an overload type), `IntrinsicId::match_signature`
/// (the function type) and `IntrinsicDescriptor::new` (overloads of two
/// modules). Each intrinsic entry also has a same-module positive control
/// answering the signature LLVM declares for `llvm.abs` / `llvm.memcpy`.
///
/// No upstream counterpart: `StructType::setBody` (`llvm/lib/IR/Type.cpp`),
/// `Function::Create` (`llvm/lib/IR/Function.cpp`),
/// `Intrinsic::getOrInsertDeclaration`, `Intrinsic::getType` and
/// `Intrinsic::matchIntrinsicSignature` (`llvm/lib/IR/Intrinsics.cpp`) take
/// `Type *`s uniqued per `LLVMContext`.
#[test]
fn a_type_entry_rejects_a_type_from_another_module() {
    let home = Module::dynamic("home");
    let foreign = Module::dynamic("foreign");
    let home_i32 = home.i32_type().as_type();
    let foreign_i32 = foreign.i32_type().as_type();
    let foreign_fn = foreign.function_type_no_parameters(foreign.void_type());
    let home_struct = home.opaque_struct("S").expect("opaque");
    let home_struct_dyn = home.opaque_struct("T").expect("opaque").as_dyn();
    let foreign_struct = foreign.opaque_struct("S").expect("opaque");
    let abs = llvmkit_ir::IntrinsicId::ABS;
    let memcpy = llvmkit_ir::IntrinsicId::MEMCPY;
    let home_abs_ty = home.function_type(home.i32_type(), [home_i32, home.i1_type().as_type()]);
    let foreign_abs_ty = foreign.function_type(
        foreign.i32_type(),
        [foreign_i32, foreign.i1_type().as_type()],
    );
    let foreign_abs =
        llvmkit_ir::IntrinsicDescriptor::new(abs, [foreign_i32]).expect("foreign descriptor");
    let home_ptr = home.ptr_type(0).as_type();
    let foreign_ptr = foreign.ptr_type(0).as_type();
    let home_i64 = home.i64_type().as_type();

    // Positive controls: each intrinsic entry answers LLVM's signature when
    // every type is the receiving module's own.
    let home_abs =
        llvmkit_ir::IntrinsicDescriptor::new(abs, [home_i32]).expect("same-module descriptor");
    assert_eq!(
        home_abs.function_type(&home).expect("descriptor signature"),
        home_abs_ty
    );
    assert_eq!(
        abs.function_type(&home, &[home_i32]).expect("id signature"),
        home_abs_ty
    );
    assert_eq!(
        abs.match_signature((&home).into(), home_abs_ty)
            .expect("matched signature")
            .overloads(),
        &[home_i32]
    );
    assert_eq!(
        llvmkit_ir::IntrinsicDescriptor::new(memcpy, [home_ptr, home_ptr, home_i64])
            .expect("same-module memcpy descriptor")
            .overloads(),
        &[home_ptr, home_ptr, home_i64]
    );
    let before = format!("{home}");
    let foreign_before = format!("{foreign}");

    let outcomes = vec![
        (
            "set_struct_body (element)",
            IrError::ForeignType,
            without_value(home.set_struct_body(home_struct, [foreign_i32], false)),
        ),
        (
            "set_struct_body (struct)",
            IrError::ForeignType,
            without_value(home.set_struct_body(foreign_struct, [home_i32], false)),
        ),
        (
            "set_struct_body_dyn (element)",
            IrError::ForeignType,
            home.set_struct_body_dyn(home_struct_dyn, [foreign_i32], false),
        ),
        (
            "set_struct_body_dyn (struct)",
            IrError::ForeignType,
            home.set_struct_body_dyn(foreign_struct.as_dyn(), [home_i32], false),
        ),
        (
            "add_function_dyn",
            IrError::ForeignType,
            without_value(home.add_function_dyn("f", foreign_fn, Linkage::External)),
        ),
        (
            "intrinsic_descriptor_from_signature",
            IrError::ForeignType,
            without_value(home.intrinsic_descriptor_from_signature("llvm.trap", foreign_fn)),
        ),
        (
            "get_or_insert_intrinsic_declaration_by_id",
            IrError::ForeignType,
            without_value(home.get_or_insert_intrinsic_declaration_by_id(
                llvmkit_ir::IntrinsicId::ABS,
                [foreign_i32],
            )),
        ),
        (
            "IntrinsicDescriptor::function_type",
            IrError::ForeignType,
            without_value(foreign_abs.function_type(&home)),
        ),
        (
            "IntrinsicId::function_type",
            IrError::ForeignType,
            without_value(abs.function_type(&home, &[foreign_i32])),
        ),
        (
            "IntrinsicId::match_signature",
            IrError::ForeignType,
            without_value(abs.match_signature((&home).into(), foreign_abs_ty)),
        ),
        (
            "IntrinsicDescriptor::new (overloads of two modules)",
            IrError::ForeignType,
            without_value(llvmkit_ir::IntrinsicDescriptor::new(
                memcpy,
                [home_ptr, foreign_ptr, home_i64],
            )),
        ),
    ];
    let let_through = not_refused_as_expected(outcomes);
    assert!(let_through.is_empty(), "{let_through:#?}");
    let foreign_after = format!("{foreign}");
    assert!(
        !foreign_after.contains("%S = type {"),
        "the foreign struct gained a body: {foreign_after}"
    );
    assert_eq!(
        foreign_after, foreign_before,
        "a refused entry must not mutate"
    );
    assert_eq!(format!("{home}"), before, "a refused entry must not mutate");
}
