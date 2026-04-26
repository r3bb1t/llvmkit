//! End-to-end Phase A3 + Phase D-lite + Phase C `build_trunc` demo.
//!
//! Builds the equivalent of:
//!
//! ```llvm
//! ; ModuleID = 'cpu_state_add'
//! define i32 @add(i64 %rax, i64 %rbx, i64 %rcx, i64 %rdx) local_unnamed_addr {
//! entry:
//!   %0 = trunc i64 %rax to i32
//!   %1 = trunc i64 %rbx to i32
//!   %2 = trunc i64 %rcx to i32
//!   %add1 = add i32 %0, %1
//!   %add2 = add i32 %add1, %2
//!   ret i32 %add2
//! }
//!
//! define noundef i32 @main() local_unnamed_addr {
//! entry:
//!   ret i32 1
//! }
//! ```
//!
//! Exercises:
//! - Multi-arg function with named parameters.
//! - `local_unnamed_addr` on both functions.
//! - `noundef` return-attribute on `main`.
//! - `trunc` casts across `i64 -> i32`.
//! - Chained `add` instructions.
//! - Multiple functions in one module.
//!
//! Run:
//!
//! ```text
//! cargo run -p llvmkit-ir --example cpu_state_add
//! ```

use llvmkit_ir::{
    AttrKind, B32, B64, IRBuilder, IntValue, IrError, Linkage, Module, RInt, UnnamedAddr,
};

pub fn build(m: &Module<'_>) -> Result<(), IrError> {
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();

    // ---- `add`: 4 i64 inputs (rax/rbx/rcx/rdx), returns i32. ----
    let add_sig = m.fn_type(
        i32_ty,
        [
            i64_ty.as_type(),
            i64_ty.as_type(),
            i64_ty.as_type(),
            i64_ty.as_type(),
        ],
        false,
    );
    let add_fn = m
        .function_builder::<RInt<B32>>("add", add_sig)
        .linkage(Linkage::External)
        .unnamed_addr(UnnamedAddr::Local)
        .param_name(0, "rax")
        .param_name(1, "rbx")
        .param_name(2, "rcx")
        .param_name(3, "rdx")
        .build()?;

    let entry = add_fn.append_basic_block("entry");
    let b = IRBuilder::new_for::<RInt<B32>>(m).position_at_end(entry);

    let rax: IntValue<B64> = add_fn.param(0)?.try_into()?;
    let rbx: IntValue<B64> = add_fn.param(1)?.try_into()?;
    let rcx: IntValue<B64> = add_fn.param(2)?.try_into()?;
    // `rdx` is part of the signature but unused in the priorities-section
    // body; touch the slot so the lifetime brand keeps it live.
    let _rdx: IntValue<B64> = add_fn.param(3)?.try_into()?;

    let t0 = b.build_trunc(rax, i32_ty, "")?;
    let t1 = b.build_trunc(rbx, i32_ty, "")?;
    let t2 = b.build_trunc(rcx, i32_ty, "")?;
    let s1 = b.build_int_add(t0, t1, "add1")?;
    let s2 = b.build_int_add(s1, t2, "add2")?;
    b.build_ret(s2)?;

    // ---- `main`: no params, returns i32, ret-attr `noundef`. ----
    let main_sig = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
    let main_fn = m
        .function_builder::<RInt<B32>>("main", main_sig)
        .linkage(Linkage::External)
        .unnamed_addr(UnnamedAddr::Local)
        .return_attribute(AttrKind::NoUndef)
        .build()?;
    let entry = main_fn.append_basic_block("entry");
    let b = IRBuilder::new_for::<RInt<B32>>(m).position_at_end(entry);
    let one = i32_ty.const_int(1_i32);
    let one_v = IntValue::<B32>::try_from(one.as_value())?;
    b.build_ret(one_v)?;

    Ok(())
}

fn main() {
    let m = Module::new("cpu_state_add");
    if let Err(e) = build(&m) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    print!("{m}");
}
