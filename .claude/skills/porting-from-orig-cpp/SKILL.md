---
name: porting-from-orig-cpp
description: Use when reading the vendored C++ tree at orig_cpp/ to decide what llvmkit should do — porting an LLVM routine or one of its arms, changing a diagnostic message or its caret anchor, reviewing a diff that claims parity, explaining why llvmkit and LLVM answer differently on some input, or judging a fix by whether probe output, bytes, or a test now match. Also use on finding any behavioural difference from the vendored tree, including one that looks unobservable.
---

# Porting from orig_cpp

llvmkit's contract is **1:1 logic with upstream**, not 1:1 output. Port the
routine as a routine: same control flow, same branch order, same early returns,
same guard conditions, and each diagnostic raised at the same point and anchored
at the same token.

Rust that produces the same answer on the inputs you probed is **not** a port.

## The trade you are about to make

Every observed failure of this rule arrived the same way: the agent wrote down
the correct standard, weighed it, and traded it for a cheaper one — byte
equality on the inputs it happened to test. It is a rationalization, not
laziness, so it comes with a justifying sentence. These are the sentences.

| What you are about to say | Why it is wrong here |
|---|---|
| "Both produce identical bytes and identical carets." | Byte equality on the inputs you probed is the thing being tested, not the test. |
| "That difference is not observable." | Unobservable-today is a divergence with a latent trigger. Fix it, or record it. |
| "I explained the mismatch in a comment / the commit body." | A comment explaining a mismatch is disclosure, not fidelity. A regression nearly shipped here as a "documented divergence". |
| "It is a hand-rolled copy of X, not a caller of X." | If upstream calls, llvmkit calls. A second copy of a routine **is** the defect. |
| "llvmkit rejects it, so upstream must reject it too." | Trace upstream to its terminal before asserting. It frequently accepts. |
| "`foo_a(x, label)` is llvmkit's spelling of `barA(T, Msg)`." | Check whether a closer spelling already exists in the same file first. |
| "The tests pass, so the port is faithful." | The tests encode what someone already believed. They cannot detect an arm nobody ported. |

**Violating the letter of the rule is violating the spirit of it.** If you find
yourself arguing that the outcome is what matters, stop — that is this failure,
in progress.

## The arm table

Before changing a ported routine, build a table for **the routine under change**
— one row per upstream arm, in upstream's order:

| upstream construct | llvmkit spelling | verdict |
|---|---|---|

Rules, each from a real miss:

- **Start at arm one.** The arm that gets skipped is the one nobody thought was
  interesting. An unported `DIArgList` arm survived four reviews that way.
- **Compare guard conditions as their own row**, not just the outcomes they
  produce. A `&&` whose operands are reordered is a divergence even when the
  accept/reject set is identical, because it changes which diagnostic fires.
- **For a catch-all (`default:` / `_`) arm, enumerate the inputs that reach it**
  and record upstream's actual answer per input. Do not infer upstream's answer
  from llvmkit's.
- **Label a conclusion you derived rather than ran as `derived` inside the row.**
  If the label only fits in a paragraph after the table, the row is not finished.

The table is capped at the routine you are changing. It is not a licence to
audit the file.

## Spelling may change; logic may not

Rust forces some substitutions. They are allowed, and each gets a comment naming
the upstream symbol it stands for. The catalogue of accepted spellings —
sentinel → enum, out-parameter → `Result`, union → algebraic type, `assert` →
typestate or test — is `AGENTS.md` § *Rust Idioms & Translation Patterns*. Read
that section; do not re-derive it and do not copy it here.

What may **not** change: which conditions are tested, in what order, and where
each diagnostic is raised.

## When you find a difference

A parity divergence is a **defect**. There are exactly two endings, and you must
say which one you chose:

1. **Fix it.** Default. Prefer this even when the difference looks cosmetic —
   the diagnostic that was "merely wrong text" turned out to be five collapsed
   messages and a moved caret.
2. **Record it**, with evidence, in `docs/divergences.md`. Only when it genuinely
   cannot be fixed now, and the entry says why.

Silence is not an option, and neither is a commit body. Before writing a new
record, grep `docs/divergences.md`, `docs/future-work.md` and
`docs/fixture-coverage.md` for the symbol **your entry would cover — not the
routine you happened to be reading.** An entry spanning ten call sites duplicates
any existing entry on any one of them. Proposed entries have duplicated existing
ones three times; the third had run this grep, for the wrong word.

If upstream's behaviour is an `assert` or `llvm_unreachable`, do not port a
crash. Record what llvmkit does instead and why that is hardening, not
divergence.

## Worked examples

`references/worked-example.md` — two real cases from this repo: a diagnostic
whose "equivalent" rewrite collapsed five upstream messages, and an unported arm
that only a whole-routine comparison could find.

## Red flags

- You are about to write "behaviourally equivalent".
- Your evidence is a probe, and the probe inputs are ones you chose.
- You are describing a difference in a comment rather than a ledger entry.
- Your arm table has no row for the `default:` arm.
- Your report has an evidence or caveat section after the table. Move each
  caveat into the row it qualifies and delete the section.
- You are about to say a routine is "the same shape" without having listed its arms.
