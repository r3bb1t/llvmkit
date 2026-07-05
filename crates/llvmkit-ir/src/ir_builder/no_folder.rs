//! No-op IR-builder folder. Mirrors `llvm/include/llvm/IR/NoFolder.h`.
//!
//! Every method declines to fold; the builder always emits a real
//! instruction.

use super::ModuleBrand;
use super::folder::IRBuilderFolder;

/// Folder that never folds: every hook keeps its default
/// "decline to fold" body, so the builder materializes a real
/// instruction for every operation. Mirrors `llvm/include/llvm/IR/NoFolder.h`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoFolder;

impl<'ctx, B: ModuleBrand + 'ctx> IRBuilderFolder<'ctx, B> for NoFolder {}
