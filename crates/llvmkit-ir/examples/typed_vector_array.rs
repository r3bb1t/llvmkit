//! Const-generic **typed vectors and arrays** — carry the element type and the
//! length in the Rust type system, so `<N x T>` / `[N x T]` length mismatches
//! and wrong-element inserts become *compile* errors instead of `verify()`
//! diagnostics. The vector/array analog of the scalar `IntValue<'ctx, W>`.
//!
//! Builds the equivalent of:
//!
//! ```llvm
//! define i32 @vadd(<4 x i32> %0, <4 x i32> %1) {
//! entry:
//!   %sum = add <4 x i32> %0, %1
//!   %lane0 = extractelement <4 x i32> %sum, i32 0
//!   ret i32 %lane0
//! }
//!
//! define i32 @apack([4 x i32] %0) {
//! entry:
//!   %u = insertvalue [4 x i32] %0, i32 7, 1
//!   %back = extractvalue [4 x i32] %u, 1
//!   ret i32 %back
//! }
//! ```
//!
//! Run with:
//!
//! ```text
//! cargo run -p llvmkit-ir --example typed_vector_array
//! ```

use llvmkit_ir::{
    ArrLen, ArrayValue, IRBuilder, IntValue, IrError, Len, Linkage, Module, VectorValue,
};

fn build() -> Result<(), IrError> {
    Module::with_new("typed_vec_array", |m| {
        let i32_ty = m.i32_type();

        // ---- Typed vectors: `<4 x i32>` ------------------------------------
        // `vector_type_n::<i32, 4>()` is the const-generic constructor; it
        // returns a `VectorType<'_, i32, Len<4>>` — the element marker (`i32`)
        // and lane count (`Len<4>`) both live in the type. `N == 0` would be a
        // compile-time `const {}` error.
        let v4i32 = m.vector_type_n::<i32, 4>();

        let fn_ty = m.fn_type(i32_ty.as_type(), [v4i32.as_type(), v4i32.as_type()], false);
        let vadd = m.add_function::<i32, _>("vadd", fn_ty, Linkage::External)?;
        let entry = vadd.append_basic_block(&m, "entry");
        let b = IRBuilder::at_end(entry);

        // Narrow the erased `<4 x i32>` params into the statically typed handle.
        // `try_into` checks BOTH element (i32) and lane count (4) at run time,
        // then stamps the markers — a `<2 x i32>` or `<4 x i64>` value fails
        // here with `OperandWidthMismatch` / `TypeMismatch`.
        let a: VectorValue<'_, i32, Len<4>> = vadd
            .param(0)
            .expect("param 0")
            .as_value()
            .try_into()
            .expect("narrow param 0 to <4 x i32>");
        let c: VectorValue<'_, i32, Len<4>> = vadd
            .param(1)
            .expect("param 1")
            .as_value()
            .try_into()
            .expect("narrow param 1 to <4 x i32>");

        // Both operands are `VectorValue<i32, Len<4>>`. `build_vec_int_add` pins
        // both to the SAME `E` and `L`, so a length or element mismatch has no
        // matching impl — a compile error, not a verifier diagnostic. E.g.:
        //   let half: VectorValue<'_, i32, Len<2>> = /* ... */;
        //   b.build_vec_int_add(a, half, "bad")?;   // E0308: Len<4> vs Len<2>
        let sum = b.build_vec_int_add(a, c, "sum")?;

        // `build_vec_extract` returns the element as its typed scalar handle;
        // the return type is inferred from the element marker as
        // `IntValue<'_, i32>` — no turbofish needed.
        let lane0: IntValue<'_, i32> =
            b.build_vec_extract(sum, i32_ty.const_int(0_i32), "lane0")?;
        b.build_ret(lane0)?;

        // ---- Typed arrays: `[4 x i32]` -------------------------------------
        let a4i32 = m.array_type_n::<i32, 4>();

        let fn_ty = m.fn_type(i32_ty.as_type(), [a4i32.as_type()], false);
        let apack = m.add_function::<i32, _>("apack", fn_ty, Linkage::External)?;
        let entry = apack.append_basic_block(&m, "entry");
        let b = IRBuilder::at_end(entry);

        let arr: ArrayValue<'_, i32, ArrLen<4>> = apack
            .param(0)
            .expect("param 0")
            .as_value()
            .try_into()
            .expect("narrow param 0 to [4 x i32]");

        // `build_arr_insert` takes an element typed by the array's marker
        // (`IntValue<i32>`); passing an `IntValue<i64>` or a float would not
        // compile. The result keeps the `i32` / `ArrLen<4>` markers, so it
        // feeds straight back into another typed array op.
        let seven: IntValue<'_, i32> = i32_ty
            .const_int(7_i32)
            .as_value()
            .try_into()
            .expect("i32 constant");
        let updated: ArrayValue<'_, i32, ArrLen<4>> = b.build_arr_insert(arr, seven, 1, "u")?;

        // Read index 1 back; element type inferred as `IntValue<i32>`.
        let back: IntValue<'_, i32> = b.build_arr_extract(updated, 1, "back")?;
        b.build_ret(back)?;

        print!("{m}");
        Ok(())
    })
}

pub fn main() {
    if let Err(e) = build() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
