//! Constant-expression, blockaddress, and token-none tests.

use llvmkit_ir::{ConstantExprFlags, ConstantExprOpcode, IRBuilder, IrError, Linkage, Module};

fn module_text(m: &Module<'_>) -> String {
    format!("{m}")
}

/// Mirrors `test/Assembler/ConstantExprNoFold.ll` and
/// `llvm/lib/IR/AsmWriter.cpp::writeConstantInternal`: constant expressions
/// print as `opcode (<typed operands> to <dest-ty>)` without folding.
#[test]
fn constant_expr_bitcast_round_trips() -> Result<(), IrError> {
    let m = Module::new("constexpr_bitcast");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    let g = m.add_global("g", i32_ty.as_type(), zero)?;
    let ptr_ty = m.ptr_type(0).as_type();
    let bitcast = m.constant_expr(
        ptr_ty,
        ConstantExprOpcode::BitCast,
        [g.as_global_constant_ptr().as_value()],
        [],
        [],
        ConstantExprFlags::default(),
    )?;
    m.add_global("p", ptr_ty, bitcast)?;

    let text = module_text(&m);
    assert!(
        text.contains("@p = global ptr bitcast (ptr @g to ptr)"),
        "got:\n{text}"
    );
    m.verify_borrowed()?;
    Ok(())
}

/// Mirrors `test/Assembler/ConstantExprNoFold.ll`: `ptrtoaddr` is distinct
/// from `ptrtoint` in constant-expression storage and printing.
#[test]
fn constant_expr_ptrtoaddr_round_trips() -> Result<(), IrError> {
    let m = Module::new("constexpr_ptrtoaddr");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    let g = m.add_global("g", i32_ty.as_type(), zero)?;
    let expr = m.constant_expr(
        m.i64_type().as_type(),
        ConstantExprOpcode::PtrToAddr,
        [g.as_global_constant_ptr().as_value()],
        [],
        [],
        ConstantExprFlags::default(),
    )?;
    m.add_global("addr", m.i64_type().as_type(), expr)?;

    let text = module_text(&m);
    assert!(
        text.contains("@addr = global i64 ptrtoaddr (ptr @g to i64)"),
        "got:\n{text}"
    );
    Ok(())
}

/// Mirrors `test/Assembler/blockaddress.ll`: a block address constant names the
/// referenced function and basic block as `blockaddress(@f, %entry)`.
#[test]
fn blockaddress_constant_round_trips() -> Result<(), IrError> {
    let m = Module::new("blockaddress_const");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let terminator = b.build_ret_void().1;
    assert!(terminator.is_terminator());
    let addr = m.block_address(f, entry)?;
    m.add_global("addr", m.ptr_type(0).as_type(), addr)?;

    let text = module_text(&m);
    assert!(
        text.contains("@addr = global ptr blockaddress(@f, %entry)"),
        "got:\n{text}"
    );
    m.verify_borrowed()?;
    Ok(())
}

/// Mirrors `llvm/lib/AsmParser/LLParser.cpp::parseValID` `kw_none` and
/// `llvm/lib/IR/AsmWriter.cpp::writeConstantInternal`: token constants print
/// as `none`.
#[test]
fn token_none_round_trips() -> Result<(), IrError> {
    let m = Module::new("token_none");
    let none = m.token_none();
    let text = format!("{}", none.as_value());
    assert_eq!(text, "token none");
    Ok(())
}

/// Mirrors `test/Assembler/ConstantExprNoFold.ll`: supported constant
/// expression opcode storage is the LLVM 22 parser-needed subset.
#[test]
fn constant_expr_supported_opcode_set_is_exact() {
    fn keyword(opcode: ConstantExprOpcode) -> &'static str {
        match opcode {
            ConstantExprOpcode::GetElementPtr => "getelementptr",
            ConstantExprOpcode::InBoundsGetElementPtr => "getelementptr",
            ConstantExprOpcode::Trunc => "trunc",
            ConstantExprOpcode::PtrToAddr => "ptrtoaddr",
            ConstantExprOpcode::PtrToInt => "ptrtoint",
            ConstantExprOpcode::IntToPtr => "inttoptr",
            ConstantExprOpcode::BitCast => "bitcast",
            ConstantExprOpcode::AddrSpaceCast => "addrspacecast",
            ConstantExprOpcode::ExtractElement => "extractelement",
            ConstantExprOpcode::InsertElement => "insertelement",
            ConstantExprOpcode::ShuffleVector => "shufflevector",
            ConstantExprOpcode::Add => "add",
            ConstantExprOpcode::Sub => "sub",
            ConstantExprOpcode::Xor => "xor",
        }
    }
    let keywords = [
        ConstantExprOpcode::GetElementPtr,
        ConstantExprOpcode::InBoundsGetElementPtr,
        ConstantExprOpcode::Trunc,
        ConstantExprOpcode::PtrToAddr,
        ConstantExprOpcode::PtrToInt,
        ConstantExprOpcode::IntToPtr,
        ConstantExprOpcode::BitCast,
        ConstantExprOpcode::AddrSpaceCast,
        ConstantExprOpcode::ExtractElement,
        ConstantExprOpcode::InsertElement,
        ConstantExprOpcode::ShuffleVector,
        ConstantExprOpcode::Add,
        ConstantExprOpcode::Sub,
        ConstantExprOpcode::Xor,
    ]
    .map(keyword);
    assert_eq!(
        keywords.as_slice(),
        [
            "getelementptr",
            "getelementptr",
            "trunc",
            "ptrtoaddr",
            "ptrtoint",
            "inttoptr",
            "bitcast",
            "addrspacecast",
            "extractelement",
            "insertelement",
            "shufflevector",
            "add",
            "sub",
            "xor",
        ]
    );
}

/// Mirrors `Verifier.cpp::Verifier::visitConstantExpr`: pointer bitcasts may
/// only bitcast to pointer types with matching address spaces.
#[test]
fn invalid_bitcast_constant_expr_is_rejected() -> Result<(), IrError> {
    let m = Module::new("constexpr_invalid_bitcast");
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    let g = m.add_global("g", i32_ty.as_type(), zero)?;
    let ptr = g.as_global_constant_ptr().as_value();

    let err = m
        .constant_expr(
            m.i64_type().as_type(),
            ConstantExprOpcode::BitCast,
            [ptr],
            [],
            [],
            ConstantExprFlags::default(),
        )
        .expect_err("pointer-to-integer bitcast constexpr is rejected");
    assert!(matches!(err, IrError::InvalidOperation { .. }));

    m.constant_expr(
        m.ptr_type(0).as_type(),
        ConstantExprOpcode::BitCast,
        [ptr],
        [],
        [],
        ConstantExprFlags::default(),
    )?;
    Ok(())
}

/// Mirrors `Verifier.cpp::Verifier::visitConstantExpr`: getelementptr
/// constant expressions validate aggregate index walks.
#[test]
fn invalid_gep_constant_expr_indices_are_rejected() -> Result<(), IrError> {
    let m = Module::new("constexpr_invalid_gep");
    let i32_ty = m.i32_type();
    let struct_ty = m.struct_type([i32_ty.as_type()], false);
    let init = struct_ty.const_struct([i32_ty.const_zero().as_constant()])?;
    let g = m.add_global("g", struct_ty.as_type(), init)?;
    let zero = i32_ty.const_zero();
    let one = i32_ty.const_int(1i32);

    let err = m
        .constant_expr_with_options(
            m.ptr_type(0).as_type(),
            ConstantExprOpcode::GetElementPtr,
            [
                g.as_global_constant_ptr().as_value(),
                zero.as_value(),
                one.as_value(),
            ],
            [],
            [],
            llvmkit_ir::ConstantExprOptions::new().source_ty(struct_ty.as_type()),
        )
        .expect_err("out-of-range struct GEP index is rejected");
    match err {
        IrError::InvalidOperation { message } => {
            assert!(message.contains("invalid getelementptr indices"))
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
    Ok(())
}

/// Mirrors `Constants.cpp::ConstantPtrAuth::get`: the constructor validates
/// all five operands and writer elision only removes trailing defaults.
#[test]
fn ptrauth_constructor_requires_five_operand_shape() -> Result<(), IrError> {
    let m = Module::new("ptrauth_constructor");
    let i8_ty = m.i8_type();
    let g = m.add_global("g", i8_ty.as_type(), i8_ty.const_zero())?;
    let ptr = g.as_global_constant_ptr();
    let key = m.i32_type().const_zero();
    let disc = m.i64_type().const_int(1i64);
    let addr_disc = m.ptr_type(0).const_null();
    let signed = m.ptr_auth(ptr, key, disc, addr_disc, ptr)?;
    m.add_global("signed", m.ptr_type(0).as_type(), signed)?;

    let text = module_text(&m);
    assert!(text.contains("@signed = global ptr ptrauth (ptr @g, i32 0, i64 1, ptr null, ptr @g)"));

    let bad = m.ptr_auth(ptr, key, disc, addr_disc, key);
    assert!(matches!(bad, Err(IrError::TypeMismatch { .. })));

    let defaulted = m.ptr_auth(ptr, key, m.i64_type().const_zero(), addr_disc, addr_disc)?;
    m.add_global("defaulted", m.ptr_type(0).as_type(), defaulted)?;
    let text = module_text(&m);
    assert!(text.contains("@defaulted = global ptr ptrauth (ptr @g, i32 0)"));
    Ok(())
}
