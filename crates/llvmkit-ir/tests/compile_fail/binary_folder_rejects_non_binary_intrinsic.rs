//! llvmkit-specific compile-fail (Doctrine D3): binary intrinsic folding accepts
//! only the narrowed semantic enum, not arbitrary generated intrinsic IDs.
//!
//! Closest upstream behaviour: `llvm/include/llvm/IR/ConstantFolder.h` exposes
//! `FoldBinaryIntrinsic` for the binary intrinsic family; llvmkit makes non-binary
//! IDs unspellable at the type level.

use llvmkit_ir::{IntrinsicId, Module, constant_fold_binary_intrinsic};

fn main() {
    let _ = Module::with_new("bad", |m| {
        let i32_ty = m.i32_type();
        let one = i32_ty.const_int(1_i32).as_constant();
        let dl = m.data_layout();
        constant_fold_binary_intrinsic(
            IntrinsicId::EXPECT,
            one,
            one,
            i32_ty.as_type(),
            &dl,
        )
    });
}
