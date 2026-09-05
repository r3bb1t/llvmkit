---
name: briefs-and-premises
description: Use when writing something another agent or a later session will act on without re-deriving it — a plan, a task spec, a handoff, a review finding, a design amendment, a bug report, or a commit message that explains why. Also use when about to state where code lives, where a value came from, what a routine is reached from, how many edits a fix needs, or what an existing doc, test or ledger entry covers. This triggers on writing a premise into any document someone will execute, including — especially — when you just read the code and feel certain.
---

# Briefs and premises

A premise you assert in a brief is a check the executor will not perform. They
will read "`FileLoc` lives in `llvmkit-support`" and go there. If it is wrong,
they burn the task discovering it, and the wrong sentence is still in the file.

This is the sibling of `claims-and-counts`, not a copy of it. That skill governs
**numbers and quantifiers**, and its remedy is to delete them. Most premises
cannot be deleted — the brief needs them to give an instruction — so the move
here is different.

## The failure is fluency, not ignorance

Every observed instance came from an agent who had **already read the refuting
code**. Not one came from not looking.

Reading a routine builds a fluent summary of it, and a sentence generated from
that summary feels like recall. It is generation. The summary is lossy in
exactly the places a brief cares about: which guard ran first, which crate a
type sits in, whether the caller had the typed value in hand.

The tell is the absence of friction. A premise that took no effort to write is
one you did not check — and it will read, to the executor, exactly like one you
did.

## The move: assert, or instruct — never assert what you could instruct

If you can verify it now, verify it and paste the evidence beside the claim.

If you cannot — or if it is the kind of thing that rots — **do not soften the
sentence. Convert it into the executor's first step.** A hedged premise is still
a premise; a step is a check that actually happens.

Asserted, and wrong:

> `llvmkit-support` has a `FileLoc`. If it holds line and column, `line_col`
> should return that rather than a new struct.

Instructed, and correct whatever the answer turns out to be:

> - [ ] **Step 1: Find where `FileLoc` lives, and check the dependency direction.**
>   ```bash
>   grep -rn "struct FileLoc" crates/ --include=*.rs
>   grep -n "^\[dependencies\]" -A8 crates/llvmkit-support/Cargo.toml
>   ```
>   Support depends on nothing, so if `FileLoc` is in another crate it cannot be
>   reused — define the pair type in support and check whether `FileLoc` should
>   be built on it.

The second version is shorter than the hedged version would have been, survives
the type moving, and hands the executor a real decision instead of a guess to
inherit.

## Run every command you write, before the brief ships

A brief dense with commands is a brief dense with untested code. Paste each one
into a shell once. This is the cheapest check in this file and it has the
highest yield:

- `sed -n '6224p' …` printed **blank** — the line was 6223, and the anchor had
  been derived by arithmetic on an earlier `sed` window rather than by a search.
- A `grep` for `FileLoc` in the wrong crate returned nothing, which is what
  exposed the location claim above.

Prefer a search over a line number in the first place — line anchors rot within
one commit, and this repo forbids them in tracked prose for that reason. If a
command's output is load-bearing, paste the output too, not just the command.

## The premise types that fail here

Each row is a real incident, and none of them contains a number or a quantifier,
so none would have tripped `claims-and-counts`.

| Type | The claim | What was true |
|---|---|---|
| **Provenance** — where a value came from | "the field name comes from `self.peek()`, so it is user text" | it is reached only past `accepts_field`, so it is in the closed set; upstream renders a macro literal there |
| **Location** — which crate or file owns a thing | "`FileLoc` is in `llvmkit-support`" | it is in `llvmkit-asmparser`, and the edge runs the other way |
| **Reachability** — what a routine is reached from | a parser arm called live | dead by construction; three separate bounds already rejected the input |
| **Scope** — how big the fix is | "24 call sites need the fallback replaced" | upstream asserts once; one guard in the recursive core |
| **Coverage** — what an existing entry covers | a ledger row "names `compute_known_bits_inner`'s global guard" | it names the *depth* guard; the type guard was unrecorded |
| **Your own earlier finding** | a survey line describing one narrow defect | the defect had three tiers and eight variants |

**Coverage is the subtle one.** Finding a hit is not the end of the check. Read
what the entry actually asserts and compare it to what you are about to claim it
covers. Recorded is not covered, and a row that names your file may be about
something else entirely.

## When the premise is your own prior conclusion

A finding you produced earlier in the same session is not verified merely
because you produced it. It was a premise then too. Before it hardens into a
task spec, re-derive it — this is where under-scoping shows up, because the
first look finds the instance and the second look finds the class.

If you do revise it, say so in the brief and say what was wrong. An executor who
knows a premise was corrected once will check it again; one who inherits a
silently-fixed sentence will not.

## Red flags

- The sentence explains **why** a thing is the way it is, and you did not look.
  Causal and provenance claims are the most confident and the least checked.
- You are about to write "which is why", "because it comes from", "this is
  reached only when", or "so it must be" — each asserts a mechanism.
- You are writing a premise that makes work **smaller** ("it's just one call
  site", "nothing depends on this"). Convenient premises get less scrutiny.
- Your brief tells someone to reuse, extend, or follow an existing thing, and
  you have not opened that thing this session.
- You hedged instead of checking. A hedge transfers your uncertainty to someone
  with less context than you, which is a worse place for it.
- You are describing what a doc, test or ledger entry covers, having matched it
  by name rather than read it.
- The brief contains a command you have not run.
- You corrected a premise and left the surrounding reasoning intact. The
  reasoning was built on the wrong premise; re-read it.

Related: `claims-and-counts` for numbers and quantifiers,
`porting-from-orig-cpp` when the premise is about upstream's behaviour.
