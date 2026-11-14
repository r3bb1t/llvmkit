//! llvmkit-specific compile-fail (D2, D10): a builder a pass's context lent
//! borrows the context `&mut`, so no context edit runs while it lives.
//!
//! `FnPatch::builder_before` takes `&mut self` and its builder carries that
//! borrow. Erasing the builder's anchor through `patch.erase` while the
//! builder is alive would leave it inserting before an instruction no longer
//! in its block; here it is `E0502` (`erase` borrows the context shared). The builds of a positioned builder are
//! infallible because of this borrow (`append_instruction` refuses nothing),
//! so this fixture is what fails if a context's builder entry ever stops
//! borrowing it. The top-level twin is `erase_anchor_while_builder_lives.rs`.
//! No upstream counterpart: upstream's `IRBuilder` keeps a raw iterator, and
//! erasing its instruction leaves it dangling.

use llvmkit_ir::{
    AtomicOrdering, FnCx, FnReport, FunctionPass, IrResult, ModuleBrand, PatchBody, SyncScope,
};

struct EraseUnderBuilder;

impl<B: ModuleBrand> FunctionPass<B> for EraseUnderBuilder {
    type Access = PatchBody;
    type Requires = ();
    const NAME: &'static str = "erase-under-builder";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, PatchBody, ()>) -> IrResult<FnReport<B>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let mut patch = cx.mutate();
        let entry = patch.function().entry_block().expect("definition");
        let anchor = entry.placed_instructions().next().expect("instruction");
        let doomed = entry
            .instructions()
            .next()
            .and_then(|instruction| instruction.as_non_terminator())
            .expect("a non-terminator");
        let builder = patch.builder_before(anchor)?;
        patch.erase(&doomed);
        let _ = builder.fence(AtomicOrdering::SequentiallyConsistent, SyncScope::System, "");
        Ok(patch.done())
    }
}

fn main() {}
