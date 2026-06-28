# Inkwell → llvmkit migration guide

`llvmkit` is a from-scratch Rust IR data model. It is **not** a wrapper
around `libLLVM`. Migration from
[`inkwell`](https://github.com/TheDan64/inkwell) is mostly straight
renames of crate path + a few intentional API tightenings; this page
lists every difference so the diff stays mechanical.

Migration is feasible for the shipped textual-IR and IR-construction surface:
types, constants, functions, globals, instructions, the modeled IRBuilder,
parser entry points, verifier typestate, and pass / analysis infrastructure.
Built-in optimization transforms, `PassBuilder`-style pipelines, bitcode, and
code generation remain outside the shipped surface.

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
use llvmkit_ir::Module;

Module::with_new("foo", |module| {
    let i32 = module.i32_type();
    // Build or parse IR using `&module` here.
});
```

`Module<'ctx, B, Unverified>` is created inside `Module::with_new`; the closure
carries the fresh brand and the unverified mutation token. There is no separate
`Context` value to construct first, and there is no public raw `ModuleCore`
handle.

## Type identity

Inkwell hands out typed wrappers around `LLVMTypeRef` — a `*mut LLVMType`.
Equality is pointer-identity at the FFI boundary.

llvmkit handles are `(TypeId, ModuleRef<'ctx, B>)` records, where
`ModuleRef<'ctx, B>` carries a process-global `ModuleId`. Identity is
derived from these integer fields — no pointers, no `as` casts. Two
modules' handles compare unequal even if their numeric `TypeId` happens
to match.

## LLVM 22 semantic deltas

These come from upstream LLVM, not from llvmkit's design choices:

- **Opaque pointers are mandatory**. `PointerType::get_element_type()`
  is gone (already so in inkwell-era LLVM 17+). `ptr` carries no
  pointee; `getelementptr` / `load` / `store` carry the element type
  explicitly.
- **`ptrtoaddr` syntax** is new in LLVM 22 alongside `ptrtoint`.
  llvmkit accepts it in parser / constant-expression paths where modeled.
- **Switch case values** are no longer instruction operands.
- **`@llvm.masked.{load,store,gather,scatter}`** lost their alignment
  arg.

## Type-state additions vs inkwell

llvmkit surfaces invariants in the type system that inkwell can only
check at runtime:

|Invariant|Inkwell|llvmkit|
|---|---|---|
|"this is a sized type"|runtime `is_sized()`|`SizedType<'ctx>` refinement; `build_alloca` takes sized element types directly|
|"this is first-class"|n/a|`BasicTypeEnum<'ctx>` excludes function / label / metadata / token / void / opaque-struct|
|"this is an aggregate"|n/a|`AggregateType<'ctx>` (array or struct only — vector is *first-class but not aggregate* per LangRef)|
|"this is basic-or-metadata" (variadic intrinsic)|n/a|`BasicMetadataTypeEnum<'ctx>`|
|"this is any IR type"|n/a|sealed `IrType<'ctx>` trait — closed extension point|
|"int predicate vs FP predicate"|inkwell uses two distinct enums (good)|`IntPredicate` + `FloatPredicate` are distinct types|
|"integer width is valid"|panic on bad width|`Module::custom_width_int_type` returns `IrResult`|
|"the builder has an insertion point"|runtime `BuilderError::NoInsertionPoint`|`IRBuilder<'_, 'ctx, B, F, S, R>` typestate: `S = Unpositioned` has no `build_*` methods at all; `position_at_end` consumes `self` and returns `IRBuilder<..., Positioned, R>`. Calling `build_int_add` on an unpositioned builder is a compile-time error.|
|"an instruction lifecycle handle cannot be reminted from a copyable view"|raw `InstructionValue`/`LLVMValueRef` handles can be copied and reused for mutation|`Instruction<Attached>` is linear; copyable discovery returns `InstructionView`, while mutation uses builder results, `BlockCursor`, or detached reinsertion.|
|"this value is an integer"|runtime `is_int_value()` / `as_int_value()`|`IntValue<'ctx>` per-kind handle. `build_int_add(lhs: IntValue, rhs: IntValue, name)` rejects non-int arguments at the type level. Same for `FloatValue`, `PointerValue`, etc.|
|"add operands have the same width"|runtime `assert_eq!(lhs.ty(), rhs.ty())` inside LLVM|`build_int_add<W: IntWidth, ...>(IntValue<'ctx, W>, IntValue<'ctx, W>, name)` enforces equal widths at compile time via the `W` marker. Mixing `IntValue<i32>` with `IntValue<i64>` is a compile error — no runtime check.|
|"`build_ret` value matches function return type"|runtime `BuilderError::TypeMismatch`|`FunctionValue<'ctx, R>` carries a `ReturnMarker`. The IRBuilder's `build_ret` is dispatched per `R`: integer Rust marker types require the matching `IntValue`, float Rust marker types require the matching `FloatValue`, `Ptr` requires a `PointerValue`, and `()` exposes only `build_ret_void()`. The runtime type-equality check survives only on `Dyn`-marked builders.|

Width markers are **Rust scalar types**: `bool`, `i8`, `i16`, `i32`,
`i64`, `i128` for static widths, plus `IntDyn` for parsed-IR / runtime
integer widths. Float kinds follow the same shape: `f32`, `f64` for the
binary32 / binary64 IEEE kinds; `Half`, `BFloat`, `Fp128`, `X86Fp80`,
`PpcFp128` for kinds without a Rust scalar counterpart; `FloatDyn` for the
runtime-checked float path. The top-level `Dyn` marks fully-erased return
shapes and is distinct from `IntDyn` / `FloatDyn`.

## Method-name deltas

|Inkwell|llvmkit|Notes|
|---|---|---|
|`Context::create()`|`Module::with_new(name, |m| ...)`|fresh branded module token scoped to the closure|
|`context.create_module(n)`|`Module::with_new(n, |m| ...)`|same|
|`context.i32_type()`|`m.i32_type()`|inside the `with_new` closure, not on a context|
|`context.custom_width_int_type(n)`|`module.custom_width_int_type(n)?`|fallible (returns `IrResult`)|
|`context.struct_type(&fields, packed)`|`module.struct_type(fields, packed)`|takes any `IntoIterator<Item = impl Into<Type<'ctx>>>`|
|`context.opaque_struct_type(n)`|`module.named_struct(n)`|name preserved|
|`StructType::set_body(...)`|`module.set_struct_body(st, fields, packed)?`|on `Module`; fallible (returns `Err` on second-set or non-named struct)|
|`fn_type(&params, var_args)`|`module.fn_type(ret, params, var_arg)`|return type explicit|
||`m.add_typed_function::<Ret, Params, _>(name, linkage)?`|builds the function signature from Rust marker types and returns `TypedFunctionValue<Ret, Params>`|
||`m.add_typed_function_of::<fn(i32) -> i32, _>(name, linkage)?`|builds the same typed facade from a Rust function-pointer alias; `unsafe` / `extern "C"` / `extern "system"` aliases are accepted|
||`#[derive(IrStruct)] struct Point { x: i32, y: i32 }`|derive-backed named struct schemas; generated `PointValue<'ctx, B>` wrappers expose typed field accessors and builders over `extractvalue` / `insertvalue`. See [`docs/ir-struct-derive.md`](docs/ir-struct-derive.md).|
||`WindowPlacementValue::try_from(raw)?`|validates an existing raw `StructValue`, `Value`, `Argument`, `Constant`, or attached `Instruction` against the derived schema before returning the generated wrapper.|
||`StructFields<WindowPlacement>`|typed-function parameter schema that emits one LLVM parameter per top-level field while keeping nested struct fields as generated wrappers.|
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
||`typed.params()`|returns a typed tuple such as `(IntValue<i32>, PointerValue)`|
||`function.get_first_basic_block()`|`f.entry_block()`|`Option<BasicBlock>`|
||`function.get_basic_blocks()`|`f.basic_blocks()`|`ExactSizeIterator<Item = BasicBlock>`|
||`function.append_basic_block("l")`|`f.append_basic_block(&m, "l")`|requires the matching unverified module token|
||`Builder::build_int_add(a, b, name)`|`b.build_int_add::<W, _, _>(lhs: IntValue<'ctx, W>, rhs: IntValue<'ctx, W>, name)?` "" `W` is inferred at the call site, mismatched widths reject at compile time.|
||`Builder::build_int_sub` / `_mul`|`b.build_int_sub(...)` / `b.build_int_mul(...)`|same shape as `add`|
||`Builder::build_return(Some(v))`|`b.build_ret(value)?`|`value: impl IntoReturnValue<'ctx, R, B>`; type must match the function's return marker|
||`Builder::build_return(None)`|`b.build_ret_void()` (`R = ()`) or `b.build_ret_void()?` (`Dyn`)|typed `void` builders are infallible; the `Dyn` path errors if the function does not return `void`|
||`Builder::position_at_end(bb)`|`IRBuilder::new(&m).position_at_end(bb)`|consumes `self` and transitions `Unpositioned` → `Positioned`; `build_*` methods are only reachable in `Positioned`|
||—|`IRBuilder::new_for::<R>(&m)`|new — produces a return-marker-tagged builder for compile-time-checked `build_ret`|
||—|`m.add_function::<R>(name, fn_ty, linkage)?`|new — typed-return form; errors with `IrError::ReturnTypeMismatch` if the signature's return type does not match `R`|
||—|`m.function_builder::<R>(name, fn_ty)`|chainable: `.linkage()` / `.calling_conv()` / `.unnamed_addr()` / `.attribute()` / `.return_attribute(kind)` / `.param_attribute(slot, kind)` / `.param_name(slot, name)` / `.build()?`|
||`f.with_typed_params::<Params>()?`|wraps functions built through the existing `function_builder` path|
||`f.with_typed_signature::<fn(i32) -> i32>()?`|wraps an existing raw function with a function-pointer schema|
||`IRBuilder::new_for_return::<fn(i32) -> i32>(&m)`|creates a builder whose return typestate is taken from the function-pointer alias|
||`Builder::build_int_truncate(v, dst, name)`|`b.build_trunc::<Src, Dst>(value, dst_ty, name)?`|widths checked at compile time via `Src: WiderThan<Dst>`; widening fails to compile|
||—|`b.build_trunc_dyn(value, dst_ty, name)?`|runtime-checked fallback for `IntValue<Dyn>` paths; errors with `IrError::OperandWidthMismatch`|
||`Builder::build_int_z_extend(v, dst, name)`|`b.build_zext::<Src, Dst>(value, dst_ty, name)?`|widths checked at compile time via `Dst: WiderThan<Src>`|
||`Builder::build_int_s_extend(v, dst, name)`|`b.build_sext::<Src, Dst>(value, dst_ty, name)?`|widths checked at compile time via `Dst: WiderThan<Src>`|
||—|`b.build_zext_dyn` / `b.build_sext_dyn`|runtime-checked fallbacks for `IntValue<Dyn>` paths|
||`Builder::build_int_compare(p, l, r, name)`|`b.build_int_cmp::<W, _, _>(IntPredicate, lhs, rhs, name)?`|both operands share width `W`; result is `IntValue<'ctx, bool>`|
||`Builder::build_unconditional_branch(bb)`|`b.build_br(target)?`|target's `R` and module brand match the builder; foreign modules are rejected by the type signature|
||`Builder::build_conditional_branch(c, t, e)`|`b.build_cond_br(cond, then_bb, else_bb)?`|`cond` accepts any `IntoIntValue<'ctx, bool>`|
||`Builder::build_unreachable()`|`b.build_unreachable()`|infallible (no operands)|
||`Builder::build_phi(ty, name)` + `phi.add_incoming(&[...])`|`b.build_int_phi::<W, _, _>(ty, incoming, name)?` + `phi.add_incoming(value, block)?`|empty initial list allowed; mirrors `PHINode::addIncoming` for the loop-edge flow|

## Error model

Inkwell's `BuilderError` becomes llvmkit's crate-level `IrError`
(`thiserror`-flavored). Every fallible path is `Result<T, IrError>`
aliased as `IrResult<T>`. Pure constructors (`module.i32_type()`,
`module.bool_type()`) stay infallible.

There is no `IrError::NoInsertionPoint`: `IRBuilder<'_, 'ctx, B, F, S, R>`
encodes insertion state, and `S = Unpositioned` has no `build_*` methods. There
is no `IrError::WrongModule` for the common branded path; the module brand plus
`ModuleRef` checks reject cross-module values.

## Lifetime brand

llvmkit handles carry a generative module brand. Each `Module::with_new` call
creates a fresh `Brand<'brand>` and passes
`Module<'brand, Brand<'brand>, Unverified>` into a `for<'brand>` closure, so
handles from separate modules cannot be mixed in normal code.

## Things you give up

- **No FFI escape hatch.** There is no `LLVMValueRef`-style raw handle
  to drop down to.
- **No code generation.** llvmkit ends at IR; lowering / linking still
  goes through upstream LLVM.
- **No public raw `LLVMValueRef` / `ModuleCore` escape hatch.** Mutation requires
  the unverified `Module` token, and verification consumes that capability.


## AsmWriter (`format!("{module}")`)

`Module`, `FunctionValue`, `BasicBlock`, `Instruction`, and `Value` implement
`Display` and produce real `.ll`. The printer mirrors
`llvm/lib/IR/AsmWriter.cpp` for the shipped opcode surface: arithmetic, casts,
memory, GEP, calls, select, phi, terminators / EH / atomics, globals, target
directives, module asm, and modeled metadata forms. Slot numbering for unnamed
values shares a single per-function counter (arguments, block labels,
instruction results), matching the upstream `SlotTracker`.

## Compile-time invariants

llvmkit promotes LLVM runtime checks into Rust types where the modeled surface
can make invalid states unrepresentable. The current ledger of bugs that
compile down to a Rust type error rather than a runtime [`IrError`]:

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
  function's exact return shape (`i32` / `f32` / `Ptr` / `()` markers).
- `build_ret_void` is *only* reachable on a `void`-returning builder (`()`)
  or on `Dyn` with a runtime check.
- The runtime [`IrError::ReturnTypeMismatch`] survives only on the `Dyn` path
  — every static marker enforces the invariant at compile time.
- Verification consumes `Module<Unverified>` and yields `Module<Verified>`.
- Read-only pass managers preserve `Module<Verified>`; transform pass managers
  return `Module<Unverified>`.
- Saved-handle mutators require `&Module<Unverified>`, so verified modules
  cannot be mutated through old handles without explicitly `unverify()`ing.