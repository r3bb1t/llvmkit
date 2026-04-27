# Inkwell → llvmkit migration guide

`llvmkit` is a from-scratch Rust IR data model. It is **not** a wrapper
around `libLLVM`. Migration from
[`inkwell`](https://github.com/TheDan64/inkwell) is mostly straight
renames of crate path + a few intentional API tightenings; this page
lists every difference so the diff stays mechanical.

The migration is currently feasible for code that builds on the **type
system surface** (Phase A) and **attributes** (Phase B). Function,
global, instruction, and full IRBuilder surfaces are scheduled per the
plan as separate sessions; check `local://IR_FOUNDATION_PLAN.md` for
the current state.

## Crate path

```diff
- use inkwell::context::Context;
- use inkwell::module::Module;
- use inkwell::types::IntType;
+ use llvmkit_ir::{Module, IntType};
```

`llvmkit` re-exports through the umbrella crate, but the IR-only entry
is `llvmkit_ir` directly.

## Context vs Module

Inkwell:

```rust
let ctx = Context::create();
let module = ctx.create_module("foo");
let i32 = ctx.i32_type();
```

llvmkit:

```rust
let module = Module::new("foo");
let i32 = module.i32_type();
```

`Module<'ctx>` carries its own interning state. There is no separate
`Context` value to construct first. If you really need a shared
"context" across modules, that's tracked as future work
(`TypePool<'ctx>`, Tier-3) and not currently shipped.

## Type identity

Inkwell hands out typed wrappers around `LLVMTypeRef` — a `*mut LLVMType`.
Equality is pointer-identity at the FFI boundary.

llvmkit handles are `(TypeId, ModuleRef<'ctx>)` records, where
`ModuleRef<'ctx>` carries a process-global `ModuleId`. Identity is
derived from these integer fields — no pointers, no `as` casts. Two
modules' handles compare unequal even if their numeric `TypeId` happens
to match.

## LLVM 22 semantic deltas

These come from upstream LLVM, not from llvmkit's design choices:

- **Opaque pointers are mandatory**. `PointerType::get_element_type()`
  is gone (already so in inkwell-era LLVM 17+). `ptr` carries no
  pointee; `getelementptr` / `load` / `store` carry the element type
  explicitly.
- **`ptrtoaddr` instruction** is new in LLVM 22 alongside `ptrtoint`.
  Phase E adds it to the instruction model.
- **Switch case values** are no longer instruction operands.
- **`@llvm.masked.{load,store,gather,scatter}`** lost their alignment
  arg.

## Type-state additions vs inkwell

llvmkit surfaces invariants in the type system that inkwell can only
check at runtime:

|Invariant|Inkwell|llvmkit|
|---|---|---|
|"this is a sized type"|runtime `is_sized()`|`SizedType<'ctx>` refinement; `build_alloca` will take it directly (Phase G)|
|"this is first-class"|n/a|`BasicTypeEnum<'ctx>` excludes function / label / metadata / token / void / opaque-struct|
|"this is an aggregate"|n/a|`AggregateType<'ctx>` (array or struct only — vector is *first-class but not aggregate* per LangRef)|
|"this is basic-or-metadata" (variadic intrinsic)|n/a|`BasicMetadataTypeEnum<'ctx>`|
|"this is any IR type"|n/a|sealed `IrType<'ctx>` trait — closed extension point|
|"int predicate vs FP predicate"|inkwell uses two distinct enums (good)|`IntPredicate` + `FloatPredicate` are distinct types|
|"integer width is valid"|panic on bad width|`Module::custom_width_int_type` returns `IrResult`|
||"the builder has an insertion point"|runtime `BuilderError::NoInsertionPoint`|`IRBuilder<'ctx, F, S, R>` typestate: `S = Unpositioned` has no `build_*` methods at all; `position_at_end` consumes `self` and returns `IRBuilder<'ctx, F, Positioned, R>`. Calling `build_int_add` on an unpositioned builder is a compile-time error.|
||"this value is an integer"|runtime `is_int_value()` / `as_int_value()`|`IntValue<'ctx>` per-kind handle. `build_int_add(lhs: IntValue, rhs: IntValue, name)` rejects non-int arguments at the type level. Same for `FloatValue`, `PointerValue`, etc.|
||"add operands have the same width"|runtime `assert_eq!(lhs.ty(), rhs.ty())` inside LLVM|`build_int_add<W: IntWidth, ...>(IntValue<'ctx, W>, IntValue<'ctx, W>, name)` enforces equal widths at compile time via the `W` marker. Mixing `IntValue<i32>` with `IntValue<i64>` is a compile error — no runtime check.|
||"`build_ret` value matches function return type"|runtime `BuilderError::TypeMismatch`|`FunctionValue<'ctx, R>` carries a `ReturnMarker`. The IRBuilder's `build_ret` is dispatched per `R`: `RInt<W>` requires an `IntValue<'ctx, W>`, `RFloat<K>` requires a `FloatValue<'ctx, K>`, `RPtr` requires a `PointerValue<'ctx>`, `RVoid` exposes only `build_ret_void()`. The runtime type-equality check survives only on `RDyn`-marked builders.|

Width markers are **Rust scalar types**: `bool`, `i8`, `i16`, `i32`,
`i64`, `i128` for static widths, `Dyn` for parsed-IR / runtime widths.
Float kinds follow the same shape: `f32`, `f64` for the binary32 /
binary64 IEEE kinds; `Half`, `BFloat`, `Fp128`, `X86Fp80`, `PpcFp128`
for kinds without a Rust scalar counterpart; `Dyn` for the runtime-
checked path. The same `Dyn` is shared between `IntType<Dyn>` and
`FloatType<Dyn>` — the surrounding container disambiguates.

## Method-name deltas

|Inkwell|llvmkit|Notes|
|---|---|---|
|`Context::create()`|`Module::new(name)`|no separate context value|
|`context.create_module(n)`|`Module::new(n)`|same|
|`context.i32_type()`|`module.i32_type()`|on `Module`, not on a context|
|`context.custom_width_int_type(n)`|`module.custom_width_int_type(n)?`|fallible (returns `IrResult`)|
|`context.struct_type(&fields, packed)`|`module.struct_type(fields, packed)`|takes any `IntoIterator<Item = impl Into<Type<'ctx>>>`|
|`context.opaque_struct_type(n)`|`module.named_struct(n)`|name preserved|
|`StructType::set_body(...)`|`module.set_struct_body(st, fields, packed)?`|on `Module`; fallible (returns `Err` on second-set or non-named struct)|
|`fn_type(&params, var_args)`|`module.fn_type(ret, params, var_arg)`|return type explicit|
||`array_type.const_array(...)`|`array_type.const_array(elements)?`|takes any `IntoIterator<Item: IsConstant<'ctx>>`; validates element type + length|
||`int_type.const_int(v, sign_extend)`|`int_type.const_int(v_rust)` infallibly when the Rust input fits losslessly; or `const_int_checked` / `const_int_raw(v: u64, sign_extend: bool)` for the raw / fallible paths. Sign-vs-zero extend is driven by the Rust input type's signedness via `IntoConstantInt<'ctx, W>`.|
||`float_type.const_float(d)` (f64)|`f64_ty.const_double(value)` / `f32_ty.const_float(value)` infallibly; `const_from_bits(u128)` for the half / bfloat / fp128 / x86_fp80 / ppc_fp128 widths.|
||`pointer_type.const_null()`|`pointer_type.const_null()`|same; also `const_zero()`|
||`type.get_undef()` / `get_poison()`|`ty.get_undef()` / `get_poison()`|same|
||`module.add_function(name, fn_ty, linkage)`|`module.add_function(name, fn_ty, linkage)?`|fallible (`Err(DuplicateFunctionName)`)|
||`module.get_function(name)`|`module.function_by_name(name)`|`Option<FunctionValue>`|
||`module.get_functions()`|`module.iter_functions()`|`ExactSizeIterator`|
||—|`module.function_builder(name, fn_ty)`|new — chainable `.linkage()` / `.calling_conv()` / `.attribute()` / `.build()?`|
||`function.get_nth_param(n)`|`f.param(n)?`|fallible (`Err(ArgumentIndexOutOfRange)`); returns `Argument<'ctx>`|
||`function.get_param_iter()`|`f.params()`|`ExactSizeIterator<Item = Argument>`|
||`function.get_first_basic_block()`|`f.entry_block()`|`Option<BasicBlock>`|
||`function.get_basic_blocks()`|`f.basic_blocks()`|`ExactSizeIterator<Item = BasicBlock>`|
||`function.append_basic_block("l")`|`f.append_basic_block("l")`|same; mutates via interior mutability|
||`Builder::build_int_add(a, b, name)`|`b.build_int_add::<W, _, _>(lhs: IntValue<'ctx, W>, rhs: IntValue<'ctx, W>, name)?` "" `W` is inferred at the call site, mismatched widths reject at compile time.|
||`Builder::build_int_sub` / `_mul`|`b.build_int_sub(...)` / `b.build_int_mul(...)`|same shape as `add`|
||`Builder::build_return(Some(v))`|`b.build_ret(value)?`|`value: impl IsValue<'ctx>`; type must match the function's return type|
||`Builder::build_return(None)`|`b.build_ret_void()?`|errors if return type isn't `void`|
||`Builder::position_at_end(bb)`|`IRBuilder::new(&m).position_at_end(bb)`|consumes `self` and transitions `Unpositioned` \u2192 `Positioned` typestate; `build_*` methods are only reachable in `Positioned`|
||—|`IRBuilder::new_for::<R>(&m)`|new — produces an [`RInt<W>`](crate::return_marker::RInt) / [`RFloat<K>`](crate::return_marker::RFloat) / [`RPtr`](crate::return_marker::RPtr) / [`RVoid`](crate::return_marker::RVoid)-tagged builder for compile-time-checked `build_ret`|
||—|`m.add_function::<R>(name, fn_ty, linkage)?`|new — typed-return form; errors with `IrError::ReturnTypeMismatch` if the signature's return type does not match `R`|
||—|`m.function_builder::<R>(name, fn_ty)`|chainable: `.linkage()` / `.calling_conv()` / `.unnamed_addr()` / `.attribute()` / `.return_attribute(kind)` / `.param_attribute(slot, kind)` / `.param_name(slot, name)` / `.build()?`|
||`Builder::build_int_truncate(v, dst, name)`|`b.build_trunc::<Src, Dst>(value, dst_ty, name)?`|widths checked at compile time via `Src: WiderThan<Dst>`; widening fails to compile|
||—|`b.build_trunc_dyn(value, dst_ty, name)?`|runtime-checked fallback for `IntValue<Dyn>` paths; errors with `IrError::OperandWidthMismatch`|
||`Builder::build_int_z_extend(v, dst, name)`|`b.build_zext::<Src, Dst>(value, dst_ty, name)?`|widths checked at compile time via `Dst: WiderThan<Src>`|
||`Builder::build_int_s_extend(v, dst, name)`|`b.build_sext::<Src, Dst>(value, dst_ty, name)?`|widths checked at compile time via `Dst: WiderThan<Src>`|
||—|`b.build_zext_dyn` / `b.build_sext_dyn`|runtime-checked fallbacks for `IntValue<Dyn>` paths|
||`Builder::build_int_compare(p, l, r, name)`|`b.build_int_cmp::<W, _, _>(IntPredicate, lhs, rhs, name)?`|both operands share width `W`; result is `IntValue<'ctx, bool>`|
||`Builder::build_unconditional_branch(bb)`|`b.build_br(target)?`|target's `R` matches the builder's; foreign module rejected with `IrError::ForeignValue`|
||`Builder::build_conditional_branch(c, t, e)`|`b.build_cond_br(cond, then_bb, else_bb)?`|`cond` accepts any `IntoIntValue<'ctx, bool>`|
||`Builder::build_unreachable()`|`b.build_unreachable()`|infallible (no operands)|
||`Builder::build_phi(ty, name)` + `phi.add_incoming(&[...])`|`b.build_int_phi::<W, _, _>(ty, incoming, name)?` + `phi.add_incoming(value, block)?`|empty initial list allowed; mirrors `PHINode::addIncoming` for the loop-edge flow|

## Error model

Inkwell's `BuilderError` becomes llvmkit's crate-level `IrError`
(`thiserror`-flavored). Every fallible path is `Result<T, IrError>`
aliased as `IrResult<T>`. Pure constructors (`module.i32_type()`,
`module.bool_type()`) stay infallible.

There is no `IrError::NoInsertionPoint` — Phase G's typestate-positioned
`IRBuilder<'ctx, S>` will make that bug class unreachable. There is no
`IrError::WrongModule` either — the `'ctx` brand catches it at compile
time for short-lived borrows.

## Lifetime brand

llvmkit handles carry a `'ctx` brand: every `Module::new()` borrow
introduces a fresh lifetime that distinguishes the handles it produces.
Mixing handles from two modules is rejected by the borrow checker for
the common case. Bullet-proof isolation against intentional mixing
requires `for<'brand>` HRTB construction (ghost-cell style); we do not
ship that.

## Things you give up

- **No FFI escape hatch.** There is no `LLVMValueRef`-style raw handle
  to drop down to.
- **No code generation.** llvmkit ends at IR; lowering / linking still
  goes through upstream LLVM.
- **No completed builder body yet.** Phase G is its own session per
  the foundation plan.


## AsmWriter (`format!("{module}")`)

`Module`, `FunctionValue`, `BasicBlock`, `Instruction`, `Value` all
implement `Display` and produce real `.ll`. Mirrors
`llvm/lib/IR/AsmWriter.cpp` for the supported opcode set
(`add`/`sub`/`mul`/`ret` plus every constant kind). Slot numbering for
unnamed values shares a single per-function counter (arguments,
block labels, instruction results), matching the upstream
`SlotTracker`.

## Compile-time invariants

Phase A3 promotes one more LLVM runtime check to the type system. The
current ledger of bugs that compile down to a Rust type error rather
than a runtime [`IrError`]:

- The IRBuilder must be positioned (`Unpositioned` has no `build_*`).
- Integer-arithmetic operands must share a width (`W: IntWidth`).
- `build_trunc`'s source / destination widths are tagged statically;
  the runtime check only fires on the `_dyn`-flavoured fallbacks.
- `build_zext` / `build_sext` reject narrowing the same way: the
  destination width must be statically wider than the source.
- `build_int_cmp`'s result type is statically `IntValue<'ctx, bool>`
  (`i1`); downstream `build_cond_br` accepts it without further
  narrowing.
- `build_cond_br`'s condition slot accepts any `IntoIntValue<'ctx, bool>`.
- Branch targets share the parent function's `R` so the typed-return
  invariant flows transitively across branches.
- Phi incoming widths match the phi's static `W`.
- `build_ret` on a typed-return builder requires a value of the
  function's exact return shape (`RInt<W>` / `RFloat<K>` / `RPtr`).
- `build_ret_void` is *only* reachable on a `void`-returning builder
  (`RVoid`) or on `RDyn` with a runtime check.
- The runtime [`IrError::ReturnTypeMismatch`] survives only on the
  `RDyn` path — every static marker enforces the invariant at
  compile time.