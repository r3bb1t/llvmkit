//! No-op IR-builder folder. Mirrors `llvm/include/llvm/IR/NoFolder.h`.
//!
//! Every method declines to fold; the builder always emits a real
//! instruction.

use super::folder::{IrBuilderFolder, TrustedFolder, trusted_folder_sealed};
use super::{ModuleBrand, Value};
use crate::CrateOnly;

/// Folder that never folds: every hook keeps its default
/// "decline to fold" body, so the builder materializes a real
/// instruction for every operation. Mirrors `llvm/include/llvm/IR/NoFolder.h`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoFolder;

impl<'ctx, B: ModuleBrand + 'ctx> IrBuilderFolder<'ctx, B> for NoFolder {}

impl trusted_folder_sealed::Sealed for NoFolder {}

impl<'ctx, B: ModuleBrand + 'ctx> TrustedFolder<'ctx, B> for NoFolder {
    /// Mirrors `NoFolder::FoldExtractValue`, which answers null.
    #[inline]
    fn fold_field(
        &self,
        _aggregate: Value<'ctx, B>,
        _index: u32,
        _token: CrateOnly,
    ) -> Option<Value<'ctx, B>> {
        None
    }
}
