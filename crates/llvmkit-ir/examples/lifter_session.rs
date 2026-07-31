//! **The consumer proof for 0.0.4.** A binary lifter as a plain,
//! re-entrant, movable value.
//!
//! This example is the shape the pre-2.0 API could not express at all, and
//! every cycle of the redesign shows up in it:
//!
//! - **Cycle A/B (ids).** Every IR reference the session stores between steps
//!   is a `Copy`, module-tagged id: the `HashMap<u64, SsaBlock<..>>` mapping
//!   guest addresses to blocks, the register file of
//!   [`IntVariable`](llvmkit_ir::IntVariable)s, the cursor. Nothing in the
//!   struct borrows anything.
//! - **Cycle C (owned modules).** The struct *owns* its
//!   [`Module`](llvmkit_ir::Module). Under `Module::with_new` the module was a
//!   local of a closure's frame, so a lifter could only ever be a closure body
//!   — never a value a host calls back into.
//! - **Cycle D (the cursor model).** [`SsaState`] holds the Braun bookkeeping
//!   and lives in a struct field; the working
//!   [`SsaBuilder`](llvmkit_ir::SsaBuilder) is minted inside `step`, used, and
//!   dropped before `step` returns. Because the builder no longer changes type
//!   on `switch_to_block`, `step` can be an ordinary `&mut self` method: there
//!   is no half-built value to park in an `Option` and take back out.
//!
//! The consequence is what the example actually demonstrates: `step()` lifts
//! **one** pseudo-instruction and returns to its caller, who inspects the
//! result and decides whether to continue — the suspend/resume shape a C-ABI
//! plugin embedding needs. And since the whole session is `Send` and holds no
//! borrows, it is handed to a worker thread *mid-function* here and finished
//! there.
//!
//! The guest program is a two-register toy ISA; the loop it encodes is
//! `sum = 0; i = 5; while i != 0 { sum += i; i -= 1 }; return sum`. Note that
//! **no phi is ever written by the lifter**: it `def`s and `use`s registers as
//! if they were mutable locals, seals each block once its guest predecessor
//! count is reached, and Braun's on-the-fly construction places exactly the
//! two loop-header phis the result needs.
//!
//! Run:
//!
//! ```text
//! cargo run -p llvmkit-ir --example lifter_session
//! ```

use std::collections::{HashMap, HashSet};
use std::thread;

use llvmkit_ir::{
    ConstantFolder, IntPredicate, IntVariable, IrError, Linkage, Module, ModuleBrand, SsaBlock,
    SsaBuilder, SsaState, Unverified, Verified,
};

// --------------------------------------------------------------------------
// The guest ISA
// --------------------------------------------------------------------------

/// One decoded pseudo-instruction of the toy guest ISA. `Copy`, so the driver
/// can look one up without borrowing the session's program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pseudo {
    /// `rD <- imm`
    MovI { dst: usize, imm: i32 },
    /// `rD <- rA + rB`
    Add { dst: usize, lhs: usize, rhs: usize },
    /// `rD <- rA - imm`
    SubI { dst: usize, src: usize, imm: i32 },
    /// `if rS == 0 goto target` (falls through otherwise)
    BrZero { src: usize, target: u64 },
    /// `goto target`
    Jmp { target: u64 },
    /// `return rS`
    Ret { src: usize },
}

impl Pseudo {
    /// Short label for the driver's log.
    fn mnemonic(self) -> &'static str {
        match self {
            Pseudo::MovI { .. } => "movi",
            Pseudo::Add { .. } => "add",
            Pseudo::SubI { .. } => "subi",
            Pseudo::BrZero { .. } => "brz",
            Pseudo::Jmp { .. } => "jmp",
            Pseudo::Ret { .. } => "ret",
        }
    }

    /// `true` if this instruction ends its basic block.
    fn is_terminator(self) -> bool {
        matches!(
            self,
            Pseudo::BrZero { .. } | Pseudo::Jmp { .. } | Pseudo::Ret { .. }
        )
    }

    /// The guest address this instruction can branch to, if any.
    fn branch_target(self) -> Option<u64> {
        match self {
            Pseudo::BrZero { target, .. } | Pseudo::Jmp { target } => Some(target),
            _ => None,
        }
    }
}

/// `sum = 0; i = 5; while i != 0 { sum += i; i -= 1 }; return sum`, at
/// plausible guest addresses.
pub const PROGRAM: &[(u64, Pseudo)] = &[
    (0x1000, Pseudo::MovI { dst: 0, imm: 0 }),
    (0x1004, Pseudo::MovI { dst: 1, imm: 5 }),
    (
        0x1008,
        Pseudo::BrZero {
            src: 1,
            target: 0x1020,
        },
    ),
    (
        0x100c,
        Pseudo::Add {
            dst: 0,
            lhs: 0,
            rhs: 1,
        },
    ),
    (
        0x1010,
        Pseudo::SubI {
            dst: 1,
            src: 1,
            imm: 1,
        },
    ),
    (0x1014, Pseudo::Jmp { target: 0x1008 }),
    (0x1020, Pseudo::Ret { src: 0 }),
];

/// Number of guest registers the session models.
const REGISTERS: usize = 2;

// --------------------------------------------------------------------------
// The session
// --------------------------------------------------------------------------

/// What one [`LifterSession::step`] did, handed back to the caller so it can
/// inspect and decide before resuming.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// One pseudo-instruction was lifted.
    Lifted {
        /// Guest address it was decoded from.
        addr: u64,
        /// Its mnemonic.
        mnemonic: &'static str,
        /// [`Module::instruction_count`] *after* lifting it — the cheap
        /// module-size probe a fixpoint driver watches for a plateau.
        module_size: usize,
    },
    /// The program is fully lifted; call [`LifterSession::finish`].
    Done,
}

/// A binary lifter, as a plain owned value. No closure, no borrow held between
/// steps, `Send`, and re-entrant.
pub struct LifterSession<B: ModuleBrand> {
    /// The module being authored. Owned — cycle C.
    module: Module<B, Unverified>,
    /// The function the guest routine lifts to.
    function: llvmkit_ir::FunctionId<i32, B>,
    /// Braun's on-the-fly SSA bookkeeping. Owned, `Send`, `Clone` — cycle D.
    /// A speculative lifter would `clone()` this before an uncertain arm and
    /// restore it if the decode turns out wrong.
    state: SsaState<B>,
    /// Guest address -> IR block. This is the map the C++ system keys by
    /// `BasicBlock *` and has to hand-migrate whenever a block is replaced;
    /// here the value is a `Copy` id ([`SsaBlock::id`] is the plain
    /// [`BlockId`](llvmkit_ir::BlockId) inside it) that stays valid.
    blocks: HashMap<u64, SsaBlock<i32, B>>,
    /// The guest register file, as typed SSA variables. Lifetime-free ids
    /// since cycle D, so they can sit in the struct next to everything else.
    registers: [IntVariable<i32, B>; REGISTERS],
    /// **The cursor, as data.** Held by the *session*, not by a builder: the
    /// builder that owns an insertion point exists only for the duration of a
    /// single `step`.
    cursor: Option<SsaBlock<i32, B>>,
    /// Guest addresses that begin a basic block, from the pre-pass.
    leaders: HashSet<u64>,
    /// How many CFG edges the pre-pass says reach each leader, and how many
    /// have been emitted so far. When they meet, the block's predecessor set
    /// is final and it is Braun-sealed — which is what keeps the emitted IR
    /// free of redundant phis.
    pred_count: HashMap<u64, usize>,
    seen_preds: HashMap<u64, usize>,
    /// Decoded program and the index of the next instruction to lift.
    program: Vec<(u64, Pseudo)>,
    pc: usize,
}

/// Look up (or lazily create) the block for a guest address.
///
/// A free function rather than a method so the borrow checker can see that it
/// touches only the builder and the block map — `step` calls it while holding
/// disjoint borrows of the session's other fields.
fn block_at<'s, 'ctx, B: ModuleBrand + 'ctx>(
    b: &mut SsaBuilder<'s, 'ctx, B, ConstantFolder, i32>,
    blocks: &mut HashMap<u64, SsaBlock<i32, B>>,
    addr: u64,
) -> SsaBlock<i32, B> {
    if let Some(&block) = blocks.get(&addr) {
        return block;
    }
    let block = b.create_block(format!("L{addr:04x}"));
    blocks.insert(addr, block);
    block
}

/// Record that one more CFG edge into `addr` has been emitted, and Braun-seal
/// its block once every predecessor the pre-pass counted has arrived.
///
/// Sealing eagerly is the difference between clean IR and IR full of
/// single-operand phis the trivial-phi rule then has to unpick: a read in an
/// *unsealed* block must place a placeholder phi, because the block might
/// still gain predecessors. Note the ordering — `seal` runs strictly after the
/// terminator that recorded the edge, since branching *to* an already-sealed
/// block is [`llvmkit_ir::IrError::SsaBranchToSealedBlock`].
fn note_edge<'s, 'ctx, B: ModuleBrand + 'ctx>(
    b: &mut SsaBuilder<'s, 'ctx, B, ConstantFolder, i32>,
    blocks: &HashMap<u64, SsaBlock<i32, B>>,
    pred_count: &HashMap<u64, usize>,
    seen_preds: &mut HashMap<u64, usize>,
    addr: u64,
) -> Result<(), IrError> {
    let seen = seen_preds.entry(addr).or_insert(0);
    *seen += 1;
    if *seen == pred_count.get(&addr).copied().unwrap_or(0) {
        b.seal_block(blocks[&addr])?;
    }
    Ok(())
}

impl<B: ModuleBrand> LifterSession<B> {
    /// Open a session over `module` for `program`.
    ///
    /// The pre-pass computes the leader set (block starts): the entry address,
    /// every branch target, and every address that falls through out of a
    /// terminator. Mirrors what any real lifter does before it emits — LLVM's
    /// own `MCDisassembler` clients build the same set.
    pub fn new(module: Module<B, Unverified>, program: &[(u64, Pseudo)]) -> Result<Self, IrError> {
        let i32_ty = module.i32_type();
        let _ = i32_ty;
        let function = module
            .add_typed_function::<i32, (), _>("lifted", Linkage::External)?
            .as_function();

        let mut leaders = HashSet::new();
        if let Some(&(first, _)) = program.first() {
            leaders.insert(first);
        }
        for (i, &(_, op)) in program.iter().enumerate() {
            if let Some(target) = op.branch_target() {
                leaders.insert(target);
            }
            if op.is_terminator()
                && let Some(&(next, _)) = program.get(i + 1)
            {
                leaders.insert(next);
            }
        }

        // Second pre-pass: how many CFG edges reach each leader. A
        // non-terminator contributes a fallthrough edge only when the next
        // address begins a block.
        let mut pred_count: HashMap<u64, usize> = HashMap::new();
        for (i, &(_, op)) in program.iter().enumerate() {
            let next = program.get(i + 1).map(|&(a, _)| a);
            match op {
                Pseudo::BrZero { target, .. } => {
                    *pred_count.entry(target).or_default() += 1;
                    if let Some(next) = next {
                        *pred_count.entry(next).or_default() += 1;
                    }
                }
                Pseudo::Jmp { target } => *pred_count.entry(target).or_default() += 1,
                Pseudo::Ret { .. } => {}
                _ => {
                    if let Some(next) = next
                        && leaders.contains(&next)
                    {
                        *pred_count.entry(next).or_default() += 1;
                    }
                }
            }
        }

        let mut state = SsaState::for_function(&module, module.view(function))?;
        let registers = {
            let mut b = SsaBuilder::for_function(&module, module.view(function), &mut state)?;
            // `from_fn` cannot be used here: `IntVariable` is `Copy` but the
            // closure would have to borrow `b` mutably per element, which is
            // exactly what a plain loop expresses more clearly anyway.
            let r0 = b.declare_int_var::<i32, _>("r0");
            let r1 = b.declare_int_var::<i32, _>("r1");
            [r0, r1]
        };

        Ok(Self {
            module,
            function,
            state,
            blocks: HashMap::new(),
            registers,
            cursor: None,
            leaders,
            pred_count,
            seen_preds: HashMap::new(),
            program: program.to_vec(),
            pc: 0,
        })
    }

    /// Lift exactly **one** pseudo-instruction and return.
    ///
    /// Everything that borrows lives inside this method: the working
    /// `SsaBuilder` is minted from `(&self.module, function, &mut self.state)`,
    /// drives one instruction, and is dropped before returning. The session is
    /// an ordinary value again the moment `step` hands control back.
    pub fn step(&mut self) -> Result<Step, IrError> {
        let Some(&(addr, op)) = self.program.get(self.pc) else {
            return Ok(Step::Done);
        };
        // The address the next instruction sits at — a conditional branch
        // falls through to it.
        let fallthrough = self.program.get(self.pc + 1).map(|&(next, _)| next);
        self.pc += 1;

        let mut b = SsaBuilder::for_function(
            &self.module,
            self.module.view(self.function),
            &mut self.state,
        )?;

        // --- restore the cursor the previous step parked ---
        if self.leaders.contains(&addr) {
            let next = block_at(&mut b, &mut self.blocks, addr);
            if let Some(current) = self.cursor {
                // The previous block ran off its end into this one: emit the
                // fallthrough edge the guest ISA leaves implicit.
                b.switch_to_block(current)?;
                b.br(next)?;
                note_edge(
                    &mut b,
                    &self.blocks,
                    &self.pred_count,
                    &mut self.seen_preds,
                    addr,
                )?;
            }
            self.cursor = Some(next);
        }
        let current = self
            .cursor
            .expect("the first program address is always a leader");
        b.switch_to_block(current)?;

        let regs = self.registers;
        match op {
            Pseudo::MovI { dst, imm } => {
                b.def_int_var(regs[dst], imm)?;
            }
            Pseudo::Add { dst, lhs, rhs } => {
                let a = b.use_int_var(regs[lhs])?;
                let c = b.use_int_var(regs[rhs])?;
                let sum = b.ins()?.build_int_add(a, c, "sum")?;
                b.def_int_var(regs[dst], sum)?;
            }
            Pseudo::SubI { dst, src, imm } => {
                let a = b.use_int_var(regs[src])?;
                let next = b.ins()?.build_int_sub(a, imm, "next_i")?;
                b.def_int_var(regs[dst], next)?;
            }
            Pseudo::BrZero { src, target } => {
                let c = b.use_int_var(regs[src])?;
                let is_zero = b.ins()?.build_int_cmp::<i32, _, _, _>(
                    IntPredicate::Eq,
                    c,
                    0_i32,
                    "is_zero",
                )?;
                let not_taken_addr =
                    fallthrough.expect("a conditional branch is never the last instruction");
                let taken = block_at(&mut b, &mut self.blocks, target);
                let not_taken = block_at(&mut b, &mut self.blocks, not_taken_addr);
                b.cond_br(is_zero, taken, not_taken)?;
                for edge in [target, not_taken_addr] {
                    note_edge(
                        &mut b,
                        &self.blocks,
                        &self.pred_count,
                        &mut self.seen_preds,
                        edge,
                    )?;
                }
                self.cursor = None;
            }
            Pseudo::Jmp { target } => {
                let dest = block_at(&mut b, &mut self.blocks, target);
                b.br(dest)?;
                note_edge(
                    &mut b,
                    &self.blocks,
                    &self.pred_count,
                    &mut self.seen_preds,
                    target,
                )?;
                self.cursor = None;
            }
            Pseudo::Ret { src } => {
                let v = b.use_int_var(regs[src])?;
                b.ret(v)?;
                self.cursor = None;
            }
        }

        Ok(Step::Lifted {
            addr,
            mnemonic: op.mnemonic(),
            module_size: self.module.instruction_count(),
        })
    }

    /// Complete SSA construction (this is where Braun seals every remaining
    /// block and completes the incomplete phis) and verify the module.
    ///
    /// Consumes the session and yields the module *by value*: the caller now
    /// owns it and can hand it to a JIT, a printer, or the next stage.
    pub fn finish(mut self) -> Result<Module<B, Verified>, IrError> {
        {
            let b = SsaBuilder::for_function(
                &self.module,
                self.module.view(self.function),
                &mut self.state,
            )?;
            b.finish()?;
        }
        self.module.verify()
    }
}

/// Drive a session to completion, logging every step. Takes the session by
/// `&mut` — proof that nothing in it is borrowed across the step boundary.
pub fn drive<B: ModuleBrand>(session: &mut LifterSession<B>, log: bool) -> Result<usize, IrError> {
    let mut lifted = 0;
    loop {
        match session.step()? {
            Step::Lifted {
                addr,
                mnemonic,
                module_size,
            } => {
                if log {
                    println!(
                        "  {addr:#06x}  {mnemonic:<5}  module now holds {module_size:>2} instructions"
                    );
                }
                lifted += 1;
            }
            Step::Done => return Ok(lifted),
        }
    }
}

// --------------------------------------------------------------------------
// Driver
// --------------------------------------------------------------------------

/// The example's own named brand: the session's type is spellable
/// (`LifterSession<Cpu>`), and the registry guarantees one live module under
/// it at a time.
pub struct Cpu;
impl ModuleBrand for Cpu {}

/// The session holds no borrows, so it is `Send` — which is what lets the
/// driver below finish a half-lifted function on another thread.
const _: () = {
    const fn assert_send<T: Send>() {}
    const fn assertions() {
        assert_send::<LifterSession<Cpu>>();
    }
    assertions()
};

fn emit() -> Result<(), IrError> {
    let mut session = LifterSession::new(Module::branded::<Cpu, _>("lifted")?, PROGRAM)?;

    println!("-- lifting on the main thread --");
    for _ in 0..3 {
        match session.step()? {
            Step::Lifted {
                addr,
                mnemonic,
                module_size,
            } => println!(
                "  {addr:#06x}  {mnemonic:<5}  module now holds {module_size:>2} instructions"
            ),
            Step::Done => unreachable!("the program has more than three instructions"),
        }
    }

    println!("-- the session moves to a worker thread, mid-function --");
    let verified = thread::spawn(move || -> Result<Module<Cpu, Verified>, IrError> {
        drive(&mut session, true)?;
        session.finish()
    })
    .join()
    .expect("worker thread completed")?;

    println!("-- lifted, verified IR --");
    print!("{verified}");
    Ok(())
}

pub fn main() {
    if let Err(e) = emit() {
        eprintln!("error: {e:?}");
        std::process::exit(1);
    }
}
