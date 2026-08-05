//! **Module-per-batch**: the JIT shape an owned module makes expressible.
//!
//! A batch compiler mints a module, fills it, verifies it, and then **hands it
//! away by value** to whatever consumes it — a JIT, an object writer, a
//! serializer — before starting the next one. The consumer takes ownership, so
//! the producer keeps nothing alive; the loop's next iteration starts from a
//! genuinely clean slate.
//!
//! Under `Module::with_new` this could not be written. The module was a local
//! of a closure's frame, so "return it" and "hand it to someone else" were
//! both impossible: everything a batch produced had to be consumed *inside*
//! the closure, and the closure could not be re-entered.
//!
//! The second thing this shows is the brand registry doing its job over a
//! sequence. The batches all run under **one named brand**, `Jit`. The
//! registry allows one live module per non-`Dyn` brand — and the claim is
//! released when the module drops, which here is when the consumer that took
//! it by value returns. So each round can claim `Jit` again, and no two
//! batches' ids are ever confusable, either at compile time (they are all
//! `Jit`-branded) or at run time (each module carries its own process-unique
//! tag, so a leftover id from batch 0 is *refused* by batch 1 rather than
//! silently resolving).
//!
//! Run:
//!
//! ```text
//! cargo run -p llvmkit-ir --example module_per_batch
//! ```

use llvmkit_ir::{IrBuilder, IrError, Linkage, Module, ModuleBrand, Unverified, ValueId, Verified};

/// The brand every batch runs under. Named, so `Module<Jit, _>` is spellable
/// in a signature — and registry-checked, so two live batches cannot overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Jit;
impl ModuleBrand for Jit {}

/// Fill `module` with `i32 @batch(i32 %x)` returning `x + n`, and verify it.
///
/// Takes the module **by value** and returns it by value: the whole batch is
/// one ownership hop, with no borrow escaping.
pub fn compile_batch<B: ModuleBrand>(
    module: Module<B, Unverified>,
    n: i32,
) -> Result<Module<B, Verified>, IrError> {
    let i32_ty = module.i32_type();
    let fn_ty = module.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = module.add_function_dyn("batch", fn_ty, Linkage::External)?;
    let entry = module.view(f).append_basic_block(&module, "entry");

    let b = IrBuilder::new_for::<llvmkit_ir::Dyn>(&module).position_at_end(entry);
    let x: llvmkit_ir::IntValue<'_, i32, _> = module.view(f).param(0)?.try_into()?;
    let sum = b.int_add(x, n, "sum")?;
    b.ret(module.view(sum))?;

    module.verify()
}

/// The consumer. Takes the verified module **by value** — this is where a real
/// pipeline would hand it to a JIT — and returns only its text. Ownership ends
/// here: the module drops when this function returns, which releases the
/// brand claim for the next batch.
pub fn submit<B: ModuleBrand>(module: Module<B, Verified>) -> String {
    format!("{module}")
}

/// Run `count` batches back to back under the same named brand.
pub fn run_batches(count: i32) -> Result<Vec<String>, IrError> {
    let mut submitted = Vec::new();
    for n in 0..count {
        // Claiming `Jit` succeeds every round *because* the previous round's
        // module was consumed by value and dropped.
        let module = Module::branded::<Jit, _>(format!("batch{n}"))?;
        let verified = compile_batch(module, n)?;
        submitted.push(submit(verified));
    }
    Ok(submitted)
}

/// A stale id from batch `n` is refused by batch `n + 1`, even though both are
/// `Jit`-branded and so indistinguishable to the type system. `true` if the
/// successor refused it, which is the whole point.
pub fn stale_id_is_refused_by_the_next_batch() -> Result<bool, IrError> {
    // `first` drops at the end of this block: the brand claim is released and
    // the storage is freed. The id outlives it — ids are `'static`.
    let first = compile_batch(Module::branded::<Jit, _>("batch0")?, 0)?;
    let batch_fn = first
        .function_by_name_dyn("batch")
        .expect("declared just above");
    let stale: ValueId<Jit> = first
        .view(batch_fn)
        .basic_blocks()
        .next()
        .expect("entry")
        .instructions()
        .next()
        .expect("the add")
        .to_erased()
        .id();
    drop(first);

    let second = compile_batch(Module::branded::<Jit, _>("batch1")?, 1)?;
    // Same brand, same *type*; different generation, so the runtime tag
    // refuses it rather than resolving against whatever now occupies the slot.
    Ok(second.try_view(stale).is_none())
}

fn emit() -> Result<(), IrError> {
    println!("-- three batches, one named brand, each handed off by value --");
    for (n, text) in run_batches(3)?.into_iter().enumerate() {
        println!("[batch {n}]");
        print!("{text}");
    }

    println!("-- a stale id from a dead batch --");
    println!(
        "  refused by its successor: {}",
        stale_id_is_refused_by_the_next_batch()?
    );
    Ok(())
}

pub fn main() {
    if let Err(e) = emit() {
        eprintln!("error: {e:?}");
        std::process::exit(1);
    }
}
