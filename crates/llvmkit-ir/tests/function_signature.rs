use llvmkit_ir::{
    FloatValue, IRBuilder, IntValue, IrError, Linkage, Module, ModuleBrand, PointerValue, Ptr,
    TypeKindLabel, TypedFunctionValue, Width,
};

/// Closest upstream coverage:
/// `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`
/// for `Function::getArg` argument ordering, plus
/// `unittests/IR/AsmWriterTest.cpp` for add+ret printing.
#[test]
fn typed_function_facade_builds_signature_and_params() -> Result<(), IrError> {
    Module::with_new("demo", |m| {
        let f = m.add_typed_function::<i32, (i32, i32), _>("add", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let (lhs, rhs) = f.params();
        let sum = b.build_int_add::<i32, _, _, _>(lhs, rhs, "sum")?;
        b.build_ret(sum)?;

        let text = format!("{m}");
        let expected = "; ModuleID = 'demo'\n\
            define i32 @add(i32 %0, i32 %1) {\n\
            entry:\n\
            \x20\x20%sum = add i32 %0, %1\n\
            \x20\x20ret i32 %sum\n\
            }\n";
        assert_eq!(text, expected, "got:\n{text}");
        Ok(())
    })
}

fn expect_pointer<'ctx, B: ModuleBrand + 'ctx>(v: PointerValue<'ctx, B>) -> PointerValue<'ctx, B> {
    v
}

fn expect_float<'ctx, B: ModuleBrand + 'ctx>(
    v: FloatValue<'ctx, f32, B>,
) -> FloatValue<'ctx, f32, B> {
    v
}

fn expect_int17<'ctx, B: ModuleBrand + 'ctx>(
    v: IntValue<'ctx, Width<17>, B>,
) -> IntValue<'ctx, Width<17>, B> {
    v
}

/// Closest upstream coverage:
/// `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`
/// for ordered arguments; type narrowing mirrors `Value::getType` category
/// checks.
#[test]
fn typed_function_facade_supports_pointer_and_float_params() -> Result<(), IrError> {
    Module::with_new("mixed", |m| {
        let f =
            m.add_typed_function::<i32, (Ptr, f32, Width<17>), _>("mixed", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let (p, x, bits) = f.params();
        let p = expect_pointer(p);
        let x = expect_float(x);
        let bits = expect_int17(bits);

        assert_eq!(p.as_value().ty().kind_label(), TypeKindLabel::Pointer);
        assert_eq!(x.as_value().ty().kind_label(), TypeKindLabel::Float);
        assert_eq!(bits.as_value().ty().kind_label(), TypeKindLabel::Integer);

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        b.build_ret(0_i32)?;
        Ok(())
    })
}

/// Closest upstream coverage:
/// `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`
/// for raw function argument counts.
#[test]
fn typed_function_facade_rejects_wrong_arity_when_wrapping_raw_function() -> Result<(), IrError> {
    Module::with_new("arity", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let raw = m.add_function::<i32, _>("one", fn_ty, Linkage::External)?;

        let err = TypedFunctionValue::<i32, (i32, i32), _>::try_from_function(raw)
            .expect_err("wrong arity must be rejected");
        assert_eq!(
            err,
            IrError::FunctionParameterCountMismatch {
                expected: 2,
                got: 1,
            }
        );
        Ok(())
    })
}

/// Closest upstream coverage:
/// `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`
/// for raw ordered arguments; wrong-kind rejection mirrors `Value::getType`
/// narrowing paths.
#[test]
fn typed_function_facade_rejects_wrong_raw_param_type() -> Result<(), IrError> {
    Module::with_new("wrong_param", |m| {
        let i32_ty = m.i32_type();
        let f64_ty = m.f64_type();
        let fn_ty = m.fn_type(i32_ty, [f64_ty.as_type()], false);
        let raw = m.add_function::<i32, _>("double_param", fn_ty, Linkage::External)?;

        let err = TypedFunctionValue::<i32, (i32,), _>::try_from_function(raw)
            .expect_err("wrong parameter kind must be rejected");
        assert_eq!(
            err,
            IrError::TypeMismatch {
                expected: TypeKindLabel::Integer,
                got: TypeKindLabel::Double,
            }
        );
        Ok(())
    })
}

type AddSig = fn(i32, i32) -> i32;
type WinApiSig = unsafe extern "system" fn(Ptr, i32, f32) -> Ptr;

/// llvmkit-specific Rust-signature facade over LLVM function types; closest
/// upstream coverage is `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`.
#[test]
fn function_pointer_alias_builds_typed_function_and_params() -> Result<(), IrError> {
    Module::with_new("alias", |m| {
        let fn_ty = m.typed_function_type_of::<AddSig>(false)?;
        assert_eq!(format!("{fn_ty}"), "i32 (i32, i32)");
        let f = m.add_typed_function_of::<AddSig, _>("add", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let (lhs, rhs) = f.params();
        let _: IntValue<'_, i32, _> = lhs;
        let _: IntValue<'_, i32, _> = rhs;
        let b = f.builder(&m).position_at_end(entry);
        let sum = b.build_int_add::<i32, _, _, _>(lhs, rhs, "sum")?;
        b.build_ret(sum)?;
        let text = format!("{m}");
        assert!(
            text.contains("define i32 @add(i32 %0, i32 %1)"),
            "got:\n{text}"
        );
        assert!(text.contains("ret i32 %sum\n"), "got:\n{text}");
        Ok(())
    })
}

/// llvmkit-specific Rust-signature facade for platform ABI-shaped function
/// pointer aliases; closest upstream coverage is LLVM function type printing in
/// `unittests/IR/AsmWriterTest.cpp`.
#[test]
fn extern_system_signature_alias_builds_pointer_return_function() -> Result<(), IrError> {
    Module::with_new("winapi", |m| {
        let f = m.add_typed_function_of::<WinApiSig, _>("call_window_proc", Linkage::External)?;
        let (hwnd, code, scale) = f.params();
        let _: PointerValue<'_, _> = hwnd;
        let _: IntValue<'_, i32, _> = code;
        let _: FloatValue<'_, f32, _> = scale;
        assert_eq!(
            format!("{}", f.as_function().signature()),
            "ptr (ptr, i32, float)"
        );
        Ok(())
    })
}

/// llvmkit-specific raw-wrapper validation; closest upstream coverage is
/// `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)` for
/// raw argument counts and order.
#[test]
fn raw_function_can_be_wrapped_with_function_pointer_signature() -> Result<(), IrError> {
    Module::with_new("raw", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let raw = m.add_function::<i32, _>("add", fn_ty, Linkage::External)?;
        let typed = raw.with_typed_signature::<AddSig>()?;
        let (lhs, rhs) = typed.params();
        let _: IntValue<'_, i32, _> = lhs;
        let _: IntValue<'_, i32, _> = rhs;
        Ok(())
    })
}

/// llvmkit-specific typed-builder return schema; closest upstream coverage is
/// `unittests/IR/AsmWriterTest.cpp` for return instruction printing.
#[test]
fn builder_can_be_created_from_function_pointer_return_schema() -> Result<(), IrError> {
    Module::with_new("builder", |m| {
        let f = m.add_typed_function_of::<AddSig, _>("zero", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for_return::<AddSig>(&m).position_at_end(entry);
        b.build_ret(0_i32)?;
        let text = format!("{m}");
        assert!(text.contains("ret i32 0\n"), "got:\n{text}");
        Ok(())
    })
}
