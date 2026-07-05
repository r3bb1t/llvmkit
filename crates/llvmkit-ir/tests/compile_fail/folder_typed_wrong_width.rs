//! A custom folder cannot return a wrong-width typed fold result:
//! the typed hook's signature pins the width. Example-lock (C++ has
//! no static analog; nearest family: IRBuilderFolder.h contract).
use llvmkit_ir::{BinaryOpcode, IRBuilderFolder, IntValue, IrResult, ModuleBrand};

struct BadFolder;

impl<'ctx, B: ModuleBrand + 'ctx> IRBuilderFolder<'ctx, B> for BadFolder {
    fn fold_int_bin_op<W: llvmkit_ir::IntWidth>(
        &self,
        _opcode: BinaryOpcode,
        lhs: IntValue<'ctx, W, B>,
        _rhs: IntValue<'ctx, W, B>,
    ) -> IrResult<Option<IntValue<'ctx, W, B>>> {
        let wide: IntValue<'ctx, i64, B> = todo!();
        Ok(Some(wide)) //~ ERROR mismatched types
    }
}

fn main() {}
