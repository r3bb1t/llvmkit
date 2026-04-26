//! Default IR-builder folder. Mirrors
//! `llvm/include/llvm/IR/ConstantFolder.h`.
//!
//! Phase G ships only the integer-add fold path the slice exercises:
//! when both operands are integer constants of the same width, return
//! the wrapping sum. Same shape applies to `sub` / `mul`. Other folds
//! (cmp, cast, GEP, FP) land as their `build_*` counterparts arrive.

use crate::constant::Constant;
use crate::ir_builder::folder::{IRBuilderFolder, as_int_const};
use crate::value::Value;

/// Default fold strategy: fold straightforward constant-on-constant
/// arithmetic; decline anything that needs more sophistication.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConstantFolder;

impl<'ctx> IRBuilderFolder<'ctx> for ConstantFolder {
    fn fold_int_add(&self, lhs: Value<'ctx>, rhs: Value<'ctx>) -> Option<Constant<'ctx>> {
        let a = as_int_const(lhs)?;
        let b = as_int_const(rhs)?;
        if a.bit_width() != b.bit_width() {
            return None;
        }
        let bits = a.bit_width();
        let av = a.value_zext_u128()?;
        let bv = b.value_zext_u128()?;
        let mask = mask_for_width(bits);
        let sum = av.wrapping_add(bv) & mask;
        let words = u128_to_words(sum);
        let id = a
            .as_value()
            .module()
            .context()
            .intern_constant_int(a.ty().as_type().id(), words);
        let module = a.as_value().module();
        let ty = a.ty().as_type().id();
        Some(Constant::from_parts(Value::from_parts(id, module, ty)))
    }

    fn fold_int_sub(&self, lhs: Value<'ctx>, rhs: Value<'ctx>) -> Option<Constant<'ctx>> {
        let a = as_int_const(lhs)?;
        let b = as_int_const(rhs)?;
        if a.bit_width() != b.bit_width() {
            return None;
        }
        let bits = a.bit_width();
        let av = a.value_zext_u128()?;
        let bv = b.value_zext_u128()?;
        let mask = mask_for_width(bits);
        let diff = av.wrapping_sub(bv) & mask;
        let words = u128_to_words(diff);
        let id = a
            .as_value()
            .module()
            .context()
            .intern_constant_int(a.ty().as_type().id(), words);
        let module = a.as_value().module();
        let ty = a.ty().as_type().id();
        Some(Constant::from_parts(Value::from_parts(id, module, ty)))
    }

    fn fold_int_mul(&self, lhs: Value<'ctx>, rhs: Value<'ctx>) -> Option<Constant<'ctx>> {
        let a = as_int_const(lhs)?;
        let b = as_int_const(rhs)?;
        if a.bit_width() != b.bit_width() {
            return None;
        }
        let bits = a.bit_width();
        let av = a.value_zext_u128()?;
        let bv = b.value_zext_u128()?;
        let mask = mask_for_width(bits);
        let product = av.wrapping_mul(bv) & mask;
        let words = u128_to_words(product);
        let id = a
            .as_value()
            .module()
            .context()
            .intern_constant_int(a.ty().as_type().id(), words);
        let module = a.as_value().module();
        let ty = a.ty().as_type().id();
        Some(Constant::from_parts(Value::from_parts(id, module, ty)))
    }
}

#[inline]
fn mask_for_width(bits: u32) -> u128 {
    if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    }
}

fn u128_to_words(v: u128) -> Box<[u64]> {
    let lo = u64::try_from(v & 0xffff_ffff_ffff_ffff)
        .unwrap_or_else(|_| unreachable!("u128 low 64 bits fits in u64"));
    let hi =
        u64::try_from(v >> 64).unwrap_or_else(|_| unreachable!("u128 high 64 bits fits in u64"));
    if hi == 0 {
        if lo == 0 {
            Box::new([])
        } else {
            Box::new([lo])
        }
    } else {
        Box::new([lo, hi])
    }
}
