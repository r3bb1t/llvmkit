# Two worked examples

Both are real, both happened in this repo, and both were caught only after the
change had been reviewed and called correct.

---

## 1. The "equivalent" rewrite that collapsed five messages

**The change.** `parseValueAsMetadata` had been fused into a larger routine. The
un-fusing was correct. To make one routine with one policy, the implementer
applied `parseValueAsMetadata`'s `TypeMsg` unconditionally to the type parse:

```rust
let ty = self.parse_type(false).map_err(|_| self.expected(type_msg))?;
```

Reasonable-looking: one routine, one policy, and the probe the implementer ran
(`metadata` followed by a non-type) still produced upstream's text.

**What the arm table would have shown.** Upstream's
`parseType(Type *&Result, const Twine &Msg, bool AllowVoid)` uses `Msg` in
**one** arm — `default:`. Every other arm produces its own message, and some
delegate to a *different overload* whose `Msg` defaults to `"expected type"`.

Tracing the `lbrace` arm to its terminal:

```
parseType(Ty, TypeMsg, Loc)          // 3-arg, Msg = "expected metadata operand"
  └─ case lltok::lbrace → parseAnonStructType
       └─ parseStructBody
            └─ parseType(Ty)         // 2-arg overload, Msg defaults to "expected type"
                 └─ default: → tokError("expected type")   ← the message the user sees
```

`TypeMsg` never reaches the output for that input.

**The damage.** Five upstream-faithful diagnostics collapsed into one, and one
lost its caret:

| input | upstream (= llvmkit before) | llvmkit after the "equivalent" rewrite |
|---|---|---|
| `{ i32, }` | `expected type` | `expected metadata operand` |
| `void` | `void type only allowed for function results`, caret on `void` | caret moved to the next token |
| `ptr*` | `ptr* is invalid - use ptr instead` | `expected metadata operand` |
| `label*` | `basic block pointers are invalid` | `expected metadata operand` |
| `[4 x undef]` | `expected type` | `expected metadata operand` |

llvmkit had been **byte-identical to upstream** on all five before the change.

**What made it look acceptable.** It was disclosed — written up as a divergence
entry with a probe. Disclosure felt like diligence. It is the third row of the
skill's rationalization table: a comment explaining a mismatch is not fidelity.

**The fix, and why it is small.** `parseType`'s `default:` arm is a switch on the
**first token**. So the faithful policy needs no `Msg` parameter and no
parser-wide refactor — only a lookahead that renders the switch's case labels:

```rust
let type_loc = self.loc();
if !self.peek_begins_a_type() {      // exactly parseType's `default:` condition
    return Err(self.expected(type_msg));
}
let ty = self.parse_type(false)?;    // nested messages pass through, anchors intact
```

Two things fell out of doing it properly: the divergence entry was **deleted**
rather than amended, and the shared predicate closed an unrelated rejects-valid
(the hand-spelled token set it replaced had omitted `kw_target`, so
`!{ target("spirv.Image") poison }` had been rejected where upstream accepts).

**Lesson.** Fixing the routine closed a divergence. Recording it would have kept
one, plus the four undiscovered messages.

---

## 2. The arm nobody ported

**The symptom.** `metadata !DIArgList(i32 %a, i32 %b)` answered
`expected metadata type`. Upstream parses it.

**Why four reviews missed it.** Each review saw only its own diff. The gap was
not *in* any diff — it was a branch that had never existed, in a routine
everyone had read.

**What the arm table shows.** Upstream `parseMetadata` opens:

```cpp
if (Lex.getKind() == lltok::MetadataVar) {
  // DIArgLists are a special case, as they are a list of ValueAsMetadata and
  // so parsing this requires a Function State.
  if (Lex.getStrVal() == "DIArgList") { … parseDIArgList(AL, PFS) … }
  MDNode *N;
  if (parseSpecializedMDNode(N)) …
}
```

Two arms inside the `MetadataVar` branch. llvmkit had the second and not the
first — it went straight to the specialized-node path.

**Verifying the port, not the outcome.** Getting `!DIArgList(...)` to parse is
not enough. The checks that made it a port:

- **Position** — the new branch sits inside the `MetadataVar` branch, *ahead of*
  the specialized-node dispatch, as upstream's does.
- **Key** — upstream compares `Lex.getStrVal() == "DIArgList"`, a byte compare
  on the metadata-var *name*. llvmkit's must have the same accept/reject set,
  including escaped spellings, which holds because the lexer mirrors
  `LexExclaim`'s `UnEscapeLexed`.
- **The `None` path** — upstream's `parseDIArgList` needs a function state, and
  at module scope upstream **asserts**. llvmkit must not port a crash: it
  returns a clean rejection carrying upstream's own literal from
  `parseNamedMetadata`. That is hardening, and it is recorded as such.

**Lesson.** Per-change review cannot find an arm that was never written. Only a
whole-routine comparison can, which is what the arm table is.
