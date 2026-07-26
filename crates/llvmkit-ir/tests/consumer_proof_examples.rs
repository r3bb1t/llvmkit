//! The consumer proof for llvmkit 2.0, locked.
//!
//! `examples/lifter_session.rs` and `examples/module_per_batch.rs` are the two
//! shapes the pre-2.0 API could not express. This file pins what they actually
//! produce, so "the shape compiles" can never quietly degrade into "the shape
//! compiles and emits nonsense":
//!
//! - the lifter's `.ll` output, byte for byte, including the *exact* two phis
//!   Braun's algorithm is supposed to place and no others;
//! - that `step()` is genuinely re-entrant — the session is suspended, moved
//!   into a container and back out, and resumed;
//! - that a module really is handed off by value per batch, and that the named
//!   brand it runs under is re-claimable each round.
//!
//! ## Upstream provenance
//!
//! example: locks output of `examples/lifter_session.rs` and
//! `examples/module_per_batch.rs`. llvmkit-specific — LLVM has no on-the-fly
//! SSA layer and no ownership story to lock; the closest functional relatives
//! are `llvm/lib/Transforms/Utils/SSAUpdater.cpp` (incremental phi placement)
//! and `std::unique_ptr<Module>` handoff in `llvm/tools/lli`.
//!
//! Mirrors the `factorial_auto_ssa_example.rs` pattern: `#[path]`-import the
//! example body, drive it, assert on the result. Both examples take their
//! module as a parameter (or are brand-generic), so the tests here use
//! `Module::dynamic` where they can — the brand registry is process-global and
//! the harness runs tests in parallel.

#[path = "../examples/lifter_session.rs"]
mod lifter_session_example;

#[path = "../examples/module_per_batch.rs"]
mod module_per_batch_example;

use llvmkit_ir::{IrError, Module};

use lifter_session_example::{LifterSession, PROGRAM, Step};

/// The lifter emits this, exactly. Two phis at the loop header (`%r1` for the
/// counter, `%r0` for the accumulator) and none anywhere else -- the lifter
/// never wrote one, and the three placeholder phis a block-at-a-time walk
/// would otherwise strand are avoided by sealing each block the moment its
/// guest predecessor count is reached.
const EXPECTED: &str = "; ModuleID = 'lifted'\n\
    define i32 @lifted() {\n\
    L1000:\n  br label %L1008\n\n\
    L1008:\n  %r1 = phi i32 [ 5, %L1000 ], [ %next_i, %L100c ]\n  %r0 = phi i32 [ 0, %L1000 ], [ %sum, %L100c ]\n  %is_zero = icmp eq i32 %r1, 0\n  br i1 %is_zero, label %L1020, label %L100c\n\n\
    L1020:\n  ret i32 %r0\n\n\
    L100c:\n  %sum = add i32 %r0, %r1\n  %next_i = sub i32 %r1, 1\n  br label %L1008\n\
    }\n";

/// example: locks `examples/lifter_session.rs` output byte-for-byte.
#[test]
fn lifter_session_example_emits_locked_ir() -> Result<(), IrError> {
    // Function-pointer coercion: marks `main` as used without running it.
    let _: fn() = lifter_session_example::main;

    let mut session = LifterSession::new(Module::dynamic("lifted"), PROGRAM)?;
    let lifted = lifter_session_example::drive(&mut session, false)?;
    assert_eq!(lifted, PROGRAM.len());

    let verified = session.finish()?;
    let actual = format!("{verified}");
    assert_eq!(actual, EXPECTED, "got:\n{actual}");
    Ok(())
}

/// The property the example exists to demonstrate: `step()` **suspends**. The
/// session is driven one pseudo-instruction at a time, and between two steps it
/// is moved through a container -- which is only possible because it holds no
/// borrow and no half-built typestate value. The resumed session finishes to
/// byte-identical IR.
#[test]
fn a_lifter_session_survives_being_moved_between_steps() -> Result<(), IrError> {
    let session = LifterSession::new(Module::dynamic("lifted"), PROGRAM)?;

    let mut addrs = Vec::new();
    let mut sizes = Vec::new();
    // The session lives in the container BETWEEN steps.
    let mut parked: Vec<LifterSession<llvmkit_ir::DynBrand>> = vec![session];

    loop {
        // Take it out, advance it by exactly one instruction, put it back.
        let mut current = parked.pop().expect("exactly one session is parked");
        let step = current.step()?;
        parked.push(current);
        match step {
            Step::Lifted {
                addr, module_size, ..
            } => {
                addrs.push(addr);
                sizes.push(module_size);
            }
            Step::Done => break,
        }
    }
    let session = parked.pop().expect("exactly one session is parked");

    // One step per pseudo-instruction, in program order.
    assert_eq!(
        addrs,
        PROGRAM.iter().map(|&(a, _)| a).collect::<Vec<_>>(),
        "step() must advance exactly one instruction at a time"
    );
    // `Module::instruction_count` -- the module-size probe a fixpoint driver
    // watches -- is monotonically non-decreasing across the lift, and the two
    // phis land *during* it (eager sealing), not in `finish`: the count after
    // the last step already equals the count in the finished module.
    assert!(
        sizes.windows(2).all(|w| w[0] <= w[1]),
        "module size must not shrink during lifting: {sizes:?}"
    );
    assert_eq!(sizes.last().copied(), Some(9), "{sizes:?}");

    let verified = session.finish()?;
    assert_eq!(format!("{verified}"), EXPECTED);
    assert_eq!(verified.instruction_count(), 9);
    Ok(())
}

/// example: `examples/module_per_batch.rs` really does hand each module away
/// by value, and the named brand it runs under is re-claimable every round --
/// which is only true because the consumer took ownership and dropped it.
#[test]
fn module_per_batch_example_hands_each_module_off_by_value() -> Result<(), IrError> {
    let _: fn() = module_per_batch_example::main;

    let submitted = module_per_batch_example::run_batches(3)?;
    assert_eq!(submitted.len(), 3);
    for (n, text) in submitted.iter().enumerate() {
        assert!(text.contains(&format!("ModuleID = 'batch{n}'")), "{text}");
        assert!(text.contains(&format!("%sum = add i32 %0, {n}")), "{text}");
    }

    // ...and the successor refuses a stale id from the batch before it, so
    // "one brand, many generations" stays safe rather than merely legal.
    assert!(module_per_batch_example::stale_id_is_refused_by_the_next_batch()?);
    Ok(())
}
