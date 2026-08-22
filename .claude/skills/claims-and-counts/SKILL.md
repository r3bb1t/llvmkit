---
name: claims-and-counts
description: Use when about to write a number, a count, or an "all / every / none / every call site / no test does X / the only one" sentence into CHANGELOG.md, UPSTREAM.md, docs/, a commit message, a test doc comment, or a report returned to a caller. Also use when re-deriving, repairing, or repeating a count that already exists in a file, and when a count or a completeness claim is itself the requested deliverable.
---

# Claims and counts

Nothing in CI checks a number written into prose. Every count in this repo's
docs rots silently, and this failure has recurred more than seventeen times.

Four rounds tried to fix it by writing *better* claims. Each round seeded a new
one. It stopped only when claims were **deleted** instead.

## Ask in this order

**1. Can the sentence do its job without the number or the quantifier?**
Then delete it. This is the default, not the fallback.

> "The guard was written inline **three times** — on the `parseMetadata`
> fall-through, in `parse_md_tuple_operand`, and in `parse_di_arg_list`."

became

> "The guard was written inline on more than one path; it now exists once."

The paragraph lost nothing a reader needed, and lost a number that was wrong
(it was twice, plus a third caller that omitted the guard).

**2. Can it be a blanket negative with no list to rot?**
"Nothing else is honoured, in any category" stays true as upstream grows.
"These six directives are unimplemented" does not — and was wrong on arrival,
because one of the six was not a distinct enum member.

**3. Only if neither: the claim carries its command and its commit, inline.**
Next to the claim, in the same sentence or the line under it. Not in a report
section, not in chat, not in a closing note — those get separated from the claim
within one edit.
A list of `file:line` anchors is not a command. If you paste anchors, the number
**is** the length of the pasted list — count them. A claim of ten beside nine
anchors has already failed, and an anchor that points at a doc comment supports
nothing.

**4. If your derivation command's pattern would match the text you are writing,
exclude the file or restate the measurement without the matchable literal.**
A command written into the file it measures counted its own quotation and
returned 92 where the truth was 91. `docs/divergences.md`'s header does this
deliberately and explains why — copy that shape.

## Tool hazards

Facts, all observed here. None is derivable by reasoning.

| Trap | Symptom | Guard |
|---|---|---|
| The harness `Grep` tool under-reports | 20 files where `rg` and POSIX `grep` both give 41 | Counts come from `rg` or POSIX `grep`. `Grep` corroborates; it is never evidence |
| `orig_cpp/` is gitignored | a bare `rg` can silently return 0 | `--no-ignore --hidden` |
| Fixtures containing a NUL byte | GNU grep prints "Binary file … matches"; 112 where the truth is 114 | `-a`, on both `rg` and `grep` |
| `rg -c` counts *lines* | undercount when one line holds two matches | `--count-matches`, or `-o \| wc -l` |
| Regex metacharacters in a literal | `(` silently changes the pattern | `-F` |
| A trailing `grep -c` in a gate chain | exits 1 on no match, so a green gate reports failure | `\|\| true`, or never end a chain on `grep` |
| `until ! ps -W \| grep -qi "cargo\|rustc"` | never terminates — permanent processes live under `.cargo\bin\` | Do not poll. Background the gate and read its completion notification |
| No git anchor for `orig_cpp/` | `git ls-files orig_cpp` is empty; a repo commit does not pin it | Anchor to the vendored tag `llvmorg-22.1.4`, and say the repo commit does not pin the tree |

## Derive after, not before

A number derived before your edit lands describes a tree that no longer exists.
Two commits have shipped a figure their own edits had already moved. Derive at
HEAD **after** the commit exists, or delete the number.

## When a count is the deliverable

Hand back a paste-ready string, not bare digits: the value, the command that
produced it, and the anchor it was measured at.

> 41 files / 57 occurrences — `rg -uuu -F --count-matches 'metadata !DIArgList(' orig_cpp/…/test/` at `fe03b01`

## Red flags

- You are repairing an existing claim. Every observed backfire was a repair.
- You are copying a number out of `CLAUDE.md`, `AGENTS.md` or `UPSTREAM.md`.
- Your sentence contains "all", "every", "none", "only", or "now say so" —
  **including when you reasoned to it from a mechanism rather than counting it.**
  Reasoning does not exempt a quantifier; it hides it. Name the one input that
  would break the sentence and go check that one.
- Your evidence is that a search returned **nothing**. That proves the term you
  typed is absent, not that the fact is. Search the concept's other spelling
  before writing "no entry exists".
- You are asserting an absence **without having run any search at all** —
  "this is unrecorded", "nothing pins this", "there is no entry to delete",
  "no caller does X". A negative existence claim is the cheapest thing in this
  file to check and the easiest to be wrong about. It has shipped twice: a task
  brief declared a defect unrecorded when `### 108` had named it for a day, and
  a routine was called uncalled when its one caller spelled the name differently.
  **Run the grep, paste it beside the claim, or do not make the claim.**
- The thing you found **exists but may not cover what you think**. A ledger row,
  a test, or a doc section is evidence of *someone's* coverage, not of yours.
  Entry 108 named this exact defect and covered two of its eight parts; a
  regression test pinned the bug with a *named* value when only an unnamed one
  reproduced it. Read what it actually asserts before counting it as covered,
  and if it is narrower than the thing you are describing, say so.
- You deleted the hedge without adding the check. An unhedged wrong claim reads
  as verified; that is worse than the hedge you removed.
- You reached for the `Grep` tool to get a count.
- You are about to write a longer, more carefully hedged claim than the one you
  are replacing. That is the failure, not the fix — go back to step 1.

Full rule: `AGENTS.md` § *Testing & QA*.
