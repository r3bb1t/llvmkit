# Inkwell → llvmkit migration guide

`llvmkit` is a from-scratch Rust IR data model. It is **not** a wrapper
around `libLLVM`. Migration from
[`inkwell`](https://github.com/TheDan64/inkwell) is mostly renames of the crate
path plus a few intentional API tightenings, with three places where the shape
of the calling code genuinely changes; this page lists every difference so the
diff stays mechanical.

Migration is feasible for the shipped textual-IR and IR-construction surface:
types, constants, functions, globals, instructions, the modeled IrBuilder,
parser entry points, verifier typestate, and pass / analysis infrastructure. A
small set of built-in analyses (`DominatorTreeAnalysis`, `KnownBitsAnalysis`,
`DemandedBitsAnalysis`) and cleanup transforms (`DcePass`, `InstSimplifyPass`,
`SimplifyDemandedBitsPass`) ship, but a broad transform library,
`PassBuilder`-style runnable pipelines, bitcode, and code generation do not.

The API described here is the one frozen for **0.0.4** (2026-07-26).

## Three structural differences to read first

Most rows in the tables below are renames. These three are not: they change the
shape of the calling code, and they are the places where the migration gives
something back rather than only tightening a check.

1. **There is no `Context`, and no context lifetime to plumb.** inkwell's
   `Context::create()` produces a value that every handle borrows from, so
   `'ctx` propagates into every struct field and function signature holding an
   inkwell handle. llvmkit has no separate context value — the module is the
   root — and a module's *identity* is a `'static` type parameter `B`, not a
   lifetime.
2. **The module is an owned value.** `Module<B, S>` has no lifetime parameter,
   owns its storage (`Box<ModuleCore>`), and is `Send`. It can be returned from
   a function, held in a struct field, collected into a `Vec`, and moved to
   another thread.
3. **What you store is an id; a handle is what you take to read.**
   Declarations and value-producing builders return a `Copy + Send + 'static`
   **id** — `IntValueId<W, B>`, `FloatValueId<K, B>`, `PointerValueId<B>`,
   `FunctionId<R, B>`, `TypedFunctionId<Ret, Params, B>`, `GlobalId<B>`,
   `BlockId<R, B, Params>`, and the instruction ids. A borrowing handle (the
   analogue of inkwell's `IntValue<'ctx>`) is minted per operation with
   `m.view(id)` — which panics on an id from another module — or
   `m.try_view(id)`, which returns `None`. So the inkwell habit of parking an
   `IntValue<'ctx>` in a `HashMap<u64, _>` becomes parking an
   `IntValueId<i32, B>` there, and the map borrows nothing.

Not every builder returns an id, and the exceptions are deliberate.
**Blocks** are minted as linear, `!Copy` insertion handles —
`FunctionValue::append_basic_block` returns `BasicBlock<'ctx, R, Unterminated, B>`
and `IrBuilder::append_block_with_params` returns a
`(BasicBlock<..>, Vec<Value<..>>)` pair — and `.id()` on the handle gives the
storable `BlockId`. No API hands back a `BlockId` directly. **Terminator**
builders consume the builder by value and return borrowing handles, not ids:
`br` / `cond_br` / `ret` return `TerminatedBlockInst<'ctx, R, B>`
(a `(BasicBlock<'ctx, R, Terminated, B>, Instruction<'ctx, Attached, B>)` pair)
and `ret_void` returns `VoidReturnInst<'ctx, B>`, its `R = ()` alias.
`BasicBlock` is deliberately not `Copy`: it is an insertion token, and
consuming it is what makes "append past a terminator" unrepresentable.

## Crate path

```diff
- use inkwell::context::Context;
- use inkwell::module::Module;
- use inkwell::types::IntType;
+ use llvmkit_ir::{Module, IntType};
```

There is no umbrella `llvmkit` crate: the IR data model is the
`llvmkit_ir` crate directly (the `IrStruct` derive rides along behind its
`macros` feature), and the `.ll` lexer / parser layers live in
`llvmkit_asmparser`.

## Context vs Module

Inkwell:

```rust
let ctx = Context::create();
let module = ctx.create_module("foo");
let i32 = ctx.i32_type();
```

llvmkit:

```rust
use llvmkit_ir::module_new;

let module = module_new!("foo")?;
let i32 = module.i32_type();
// Build or parse IR using `&module`, then `module.verify()?`.
```

`module_new!` yields a `Module<B, Unverified>` — an owned token carrying a
compile-time brand `B` (declared at the macro's expansion site, so unnameable
elsewhere) and the unverified mutation authority. `Module::branded::<B>` takes a
brand you name; `Module::dynamic` opts out of the compile-time half of identity
when the module count is decided at run time. There is no separate `Context`
value to construct first, and there is no public raw `ModuleCore` handle.

The token owns its storage and borrows nothing, so it can be returned from a
function, stored in a struct or `Vec`, and moved across a thread boundary.

End to end, a two-argument `add` — the shape every row below composes into:

```rust
use llvmkit_ir::{IrBuilder, IrError, Linkage, module_new};

fn build() -> Result<(), IrError> {
    let m = module_new!("demo")?;
    // A declaration returns a storable id, not a handle.
    let f = m.add_typed_function::<i32, (i32, i32), _>("add", Linkage::External)?;
    // `m.view(f)` is the borrowing handle; the block it appends back is a
    // linear insertion token (call `.id()` on it to store it).
    let entry = m.view(f).append_basic_block(&m, "entry");

    let b = IrBuilder::at_end(entry);
    let (lhs, rhs) = m.view(f).params();
    let sum = b.int_add::<i32, _, _, _>(lhs, rhs, "sum")?; // IntValueId<i32, _>
    b.ret(sum)?;                                           // consumes `b`

    print!("{m}");
    Ok(())
}
```

## Type identity

Inkwell hands out typed wrappers around `LLVMTypeRef` — a `*mut LLVMType`.
Equality is pointer-identity at the FFI boundary.

llvmkit type handles are `Type<'ctx, B>` records pairing an interned arena
index (`TypeSlot`) with a `ModuleRef<'ctx, B>` that carries a process-global
`ModuleId`. Identity is derived from those integer fields — no pointers, no
`as` casts. Two modules' handles compare unequal even if their numeric
`TypeSlot` happens to match; and when the two modules have *distinct* brand
types, mixing their handles does not compile at all, so no comparison is
reached.

The arena indices (`TypeSlot`, `ValueSlot`) have no public constructor, and the
`(ModuleId, slot)` payload inside every public id is private with no
`from_raw_parts`. A slot cannot be forged from an arbitrary integer.

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
|"this is a sized type"|runtime `is_sized()`|`SizedType<'ctx, B>` refinement (`TryFrom<Type>`); the verifier's alloca / load / store / GEP element-sizedness rules are phrased through it — `alloca` accepts any `IrType`, and sized-ness is enforced at verification|
|"this is first-class"|n/a|`BasicTypeEnum<'ctx, B>` excludes function / label / metadata / token / void / opaque-struct|
|"this is an aggregate"|n/a|`AggregateType<'ctx, B>` (array or struct only — vector is *first-class but not aggregate* per LangRef)|
|"this is basic-or-metadata" (variadic intrinsic)|n/a|`BasicMetadataTypeEnum<'ctx, B>`|
|"this is any IR type"|n/a|sealed `IrType<'ctx, B>` trait — closed extension point|
|"int predicate vs FP predicate"|inkwell uses two distinct enums (good)|`IntPredicate` + `FloatPredicate` are distinct types|
|"integer width is valid"|panic on bad width|`Module::custom_width_int_type` returns `IrResult`|
|"this handle belongs to this module"|n/a (a `Value *` from another `Context` is accepted and diagnosed later, if at all)|the owning module is the `B: ModuleBrand` type parameter on every handle, id, and builder. Two modules with **distinct** brands cannot exchange operands: a type error, no runtime check involved. Where a brand is deliberately shared (`Module::dynamic`'s `DynBrand`, or a named brand re-issued after the previous module dropped) the runtime `ModuleId` tag on the id is the backstop — `IrError::ForeignValueId`, `try_view` → `None`, or a `view` panic|
|"the builder has an insertion point"|runtime `BuilderError::NoInsertionPoint`|`IrBuilder<'_, 'ctx, B, F, S, R>` typestate: `S = Unpositioned` has no emitter methods at all; `position_at_end` consumes `self` and returns `IrBuilder<..., Positioned, R>`. Calling `int_add` on an unpositioned builder is a compile-time error.|
|"a block ends in exactly one terminator"|runtime `Verifier::visitBasicBlock`|`BasicBlock<'ctx, R, Term, B, Params>` termination typestate: terminator builders (`br`, `ret`, ...) consume the `Positioned` builder **and** the block token, and return the insertion block re-tagged `Terminated`; `position_at_end` accepts only `Unterminated` blocks. A second terminator call is `E0382` (use of moved value), where upstream's `IRBuilder` keeps its insertion point and silently appends one. The copyable `BasicBlockLabel<'ctx, R, B, Params>` — the view `m.view(block_id)` resolves to — names a block as a branch target / phi predecessor without granting insertion.|
|"an instruction lifecycle handle cannot be reminted from a copyable view"|raw `InstructionValue`/`LLVMValueRef` handles can be copied and reused for mutation|`Instruction<Attached>` is linear; copyable discovery returns `InstructionView`, while mutation uses builder results, `BlockCursor`, or detached reinsertion.|
|"this value is an integer"|runtime `is_int_value()` / `as_int_value()`|`IntValue<'ctx, W, B>` per-kind handle, with `IntValueId<W, B>` as its storable form. `int_add` takes `IntoIntValue<'ctx, W, B>` operands — the typed handle, the typed id, or a Rust scalar literal — and rejects non-int arguments at the type level. Same for `FloatValue`, `PointerValue`, etc.|
|"add operands have the same width"|runtime `assert_eq!(lhs.ty(), rhs.ty())` inside LLVM|`int_add<W: IntWidth, Lhs, Rhs, Name>(lhs: Lhs, rhs: Rhs, name: Name) -> IrResult<IntValueId<W, B>>` pins both operands to one `W` via `IntoIntValue<'ctx, W, B>`. Mixing an `i32` value with an `i64` one is a compile error — no runtime check.|
|"`ret` value matches function return type"|runtime `BuilderError::TypeMismatch`|`FunctionValue<'ctx, R, B>` carries a `ReturnMarker`. The IrBuilder's `ret` is dispatched per `R`: integer Rust marker types require the matching `IntValue`, float Rust marker types require the matching `FloatValue`, `Ptr` requires a `PointerValue`, and `()` exposes only `ret_void()`. The runtime type-equality check survives only on `Dyn`-marked builders.|

Width markers are **Rust scalar types**: `bool`, `i8`, `i16`, `i32`,
`i64`, `i128` for static widths, plus `IntDyn` for parsed-IR / runtime
integer widths. Float kinds follow the same shape: `f32`, `f64` for the
binary32 / binary64 IEEE kinds; `Half`, `BFloat`, `Fp128`, `X86Fp80`,
`PpcFp128` for kinds without a Rust scalar counterpart; `FloatDyn` for the
runtime-checked float path. The top-level `Dyn` marks fully-erased return
shapes and is distinct from `IntDyn` / `FloatDyn`.

## Method-name deltas

In the llvmkit column, `m` is an owned `Module<B, Unverified>` and `b` a
`Positioned` `IrBuilder`. Two spelling conventions make many llvmkit cells
shorter than their inkwell twins: emitters drop inkwell's `build_` prefix
(`b.int_add(..)` vs `builder.build_int_add(..)`), and lookups are bare nouns
with no `get_` prefix (`m.global(n)` vs `module.get_global(n)`; `get_` survives
only in the std-consistent `get_or_insert_*` entry points). Where a declaration
or builder hands back an id, the
row says so; `m.view(id)` (or `b.view(id)` mid-chain) is how you get the
borrowing handle to read from.

`f` is a plain `FunctionId<R, B>`, so `m.view(f)` is a `FunctionValue`. `typed_f`
is a `TypedFunctionId<Ret, Params, B>`, and `m.view(typed_f)` is the
`TypedFunctionValue` facade — which is where `.as_function()` lives, for handing
the plain form to an API that wants one. `crates/llvmkit-ir/examples/factorial.rs`
takes the typed route, which is why its call reads
`m.view(f).as_function()` where the row below reads `m.view(f)`.

|Inkwell|llvmkit|Notes|
|---|---|---|
|`Context::create()`|`module_new!(name)?`|owned, branded module token; no separate context|
|`context.create_module(n)`|`module_new!(n)?` / `Module::branded::<B, _>(n)?` / `Module::dynamic(n)`|same, three brand policies. Only `dynamic` is infallible — the other two claim a brand in the process-global registry (`IrError::BrandInUse` / `BrandRetired`)|
|storing an `IntValue<'ctx>` in a struct or `HashMap`|store the id (`IntValueId<W, B>`, `FunctionId<R, B>`, `BlockId<R, B, Params>`, …); read through `m.view(id)` / `m.try_view(id)`|ids are `Copy + Send + 'static` and borrow nothing. `handle.id()` mints one from a handle. A stale or foreign id is `IrError::ForeignValueId`, `None`, or a panic — never a dangling read (`#![forbid(unsafe_code)]` workspace-wide)|
|`context.i32_type()`|`m.i32_type()`|on the module (or its `ModuleView`), not on a context|
|`context.custom_width_int_type(n)`|`m.custom_width_int_type(n)?`|fallible (returns `IrResult<IntType<'ctx, IntDyn, B>>`)|
|`context.struct_type(&fields, packed)`|`m.struct_type(fields)` / `m.packed_struct_type(fields)`|takes any `IntoIterator<Item = impl Into<Type<'ctx, B>>>`; packed-ness is the method, not a trailing `bool`|
|`context.opaque_struct_type(n)`|`m.get_or_insert_named_struct(n)`|get-or-create, name preserved; the bare noun `m.named_struct(n)` is the lookup (`Option`), and `m.opaque_struct(n)?` is the typestate form (`StructType<'ctx, Opaque, B>`, `Err` if the name is taken)|
|`StructType::set_body(...)`|`m.set_struct_body(opaque, fields, packed)?`|on `Module`; upgrades `Opaque` → `BodySet` in the type system. `set_struct_body_dyn(st, fields, packed)?` is the runtime-checked erased path (returns `Err` on second-set or non-named struct)|
|`fn_type(&params, var_args)`|`m.function_type(ret, params)` / `m.variadic_function_type(ret, params)`|return type explicit; variadic-ness is the method, not a trailing `bool` (`*_no_parameters` twins cover the empty parameter list)|
||`m.add_typed_function::<Ret, Params, _>(name, linkage)?`|builds the function signature from Rust marker types and returns `IrResult<TypedFunctionId<Ret, Params, B>>`; `m.view(id)` gives the `TypedFunctionValue` facade|
||`m.add_typed_function_of::<fn(i32) -> i32, _>(name, linkage)?`|builds the same typed facade from a Rust function-pointer alias; `unsafe` / `extern "C"` / `extern "system"` aliases are accepted|
||`#[derive(IrStruct)] struct Point { x: i32, y: i32 }`|derive-backed named struct schemas (the `macros` feature, on by default); generated `PointValue<'ctx, B>` wrappers expose typed field accessors and builders over `extractvalue` / `insertvalue`. See [`docs/ir-struct-derive.md`](docs/ir-struct-derive.md).|
||`WindowPlacementValue::try_from(raw)?`|validates an existing raw `StructValue`, `Value`, `Argument`, `Constant`, or attached `Instruction` against the derived schema before returning the generated wrapper.|
||`StructFields<WindowPlacement>`|typed-function parameter schema that emits one LLVM parameter per top-level field while keeping nested struct fields as generated wrappers.|
|`int_type.const_array(&values)`|`array_type.const_array(elements)?`|on the array type; takes any `IntoIterator<Item: IntoConstantValue<'ctx, B>>` and validates element type + length|
|`int_type.const_int(v, sign_extend)`|`int_type.const_int(v_rust)`|infallible when the Rust input fits losslessly — sign-vs-zero extend is driven by the Rust input type's signedness via `IntoConstantInt<'ctx, W, B>`; `const_int_checked` / `const_int_raw(v: u64, sign_extend: bool)` are the fallible / raw paths|
|`float_type.const_float(d)` (f64)|`f64_ty.const_double(value)` / `f32_ty.const_float(value)`|infallible; `const_from_bits(u128)` covers the half / bfloat / fp128 / x86_fp80 / ppc_fp128 kinds|
|`pointer_type.const_null()`|`pointer_type.const_null()`|same; also `const_zero()`|
|`type.get_undef()` / `get_poison()`|`ty.undef()` / `ty.poison()`|bare-noun lookups — shorter than inkwell's spelling|
|`module.add_function(name, fn_ty, linkage)`|`m.add_function_dyn(name, fn_ty, linkage)?`|fallible (`Err(DuplicateFunctionName)`); returns `IrResult<FunctionId<Dyn, B>>` — the runtime-checked shape. Re-type later with the checked `m.function::<R>(name)?`|
|`module.get_function(name)`|`m.function_dyn(name)`|`Option<FunctionId<Dyn, B>>` — an id, symmetric with `add_function_dyn`. `m.function::<R>(name)?` is the narrowing form: `IrResult<Option<FunctionId<R, B>>>`, where a signature that does not match `R` is `IrError::ReturnTypeMismatch` rather than a silently widened id|
|`module.get_global(name)`|`m.global(name)`|`Option<GlobalId<B>>`; `alias` / `ifunc` follow the same bare-noun shape — llvmkit's lookup is shorter than inkwell's. All of these take `&self` and work on a `Verified` module too|
|`module.get_functions()`|`m.as_view().functions()`|`ExactSizeIterator<Item = FunctionView>` on the read-only module view|
|`Context::create_module_from_ir(buf)`|`parse_dynamic(src)?` / `parse_branded::<B>(src)?` / `parse_file_dynamic(path)?` / `parse_file_branded::<B>(path)?`|in `llvmkit_asmparser`; textual `.ll` only — there is no bitcode reader. Each returns the owned `Module<_, Unverified>` with its brand intact, so a parsed module can be verified, stored and moved. `parse_into(module, src)?` parses into a module you already made and hands it back. The closure form `parse_assembly(src, |m, parsed| ..)` remains only for callers who need the `ParsedModule` slot mapping, which borrows the module it came from|
|—|`m.function_builder::<Dyn, _>(name, fn_ty)`|new — chainable `.linkage()` / `.calling_conv()` / `.attribute()` / `.build()?`, where `build()` yields `IrResult<FunctionId<R, B>>`|
|`function.get_nth_param(n)`|`m.view(f).param(n)?`|fallible (`Err(ArgumentIndexOutOfRange)`); returns `Argument<'ctx, B>`|
|`function.get_param_iter()`|`m.view(f).params()`|`ExactSizeIterator<Item = Argument<'ctx, B>>`|
||`m.view(typed_f).params()`|returns a typed tuple such as `(IntValue<'_, i32, _>, PointerValue<'_, _>)`|
|`function.get_first_basic_block()`|`m.view(f).entry_block()`|`Option<BasicBlock<'ctx, R, Terminated, B>>` — a read-only view; appends go through the `Unterminated` handle from `append_basic_block`|
|`function.get_basic_blocks()`|`m.view(f).basic_blocks()`|`ExactSizeIterator` of the same read-only `Terminated` views|
|`function.append_basic_block("l")`|`m.view(f).append_basic_block(&m, "l")`|requires the matching unverified module token; returns the linear `BasicBlock<'ctx, R, Unterminated, B>` insertion handle (not an id — call `.id()` for the storable `BlockId`)|
|`Builder::build_int_add(a, b, name)`|`b.int_add(lhs, rhs, name)?`|no `build_` prefix — the llvmkit call is shorter. Operands are anything satisfying `IntoIntValue<'ctx, W, B>` — an `IntValue<'ctx, W, B>`, an `IntValueId<W, B>`, or a Rust scalar literal; `W` is inferred at the call site (explicit form `::<W, _, _, _>`) and mismatched widths reject at compile time. Returns `IrResult<IntValueId<W, B>>`. `b.int_add_dyn(lhs, rhs, name)?` is the runtime-checked `Value`-level fallback|
|`Builder::build_int_sub` / `_mul`|`b.int_sub(...)` / `b.int_mul(...)`|same shape as `add`, including the `_dyn` fallbacks|
|`Builder::build_call(callee, &[args], name)`|`b.call(typed_callee, (args...), name)?` for a typed callee, or `b.call_dyn(callee, [args], name)?` for a plain function|typed path returns `IrResult<TypedCallInstId<Ret, B>>`; wrong arity / wrong-typed argument are compile errors via `CallArgs`/`IntoCallArg`, and `b.view(call).result()` narrows to the callee's real return type with no `try_into`. Dyn path returns `IrResult<CallInstId<R, B>>` and rejects a wrong arity or argument type at build time with `IrError::CallArgumentCountMismatch` / `CallArgumentTypeMismatch` instead of reaching the verifier|
|`Builder::build_return(Some(v))`|`b.ret(value)?`|`value: impl IntoReturnValue<'ctx, R, B>`; type must match the function's return marker. Consumes the `Positioned` builder and returns `TerminatedBlockInst<'ctx, R, B>` — the (`BasicBlock<'ctx, R, Terminated, B>`, `Instruction`) pair, a borrowing handle rather than an id|
|`Builder::build_return(None)`|`b.ret_void()` (`R = ()`) or `b.ret_void()?` (`Dyn`)|typed `void` builders are infallible and return `VoidReturnInst<'ctx, B>`; the `Dyn` path errors if the function does not return `void`|
|`Builder::position_at_end(bb)`|`IrBuilder::new(&m).position_at_end(bb)`, or `IrBuilder::at_end(bb)`|`position_at_end` consumes `self` and transitions `Unpositioned` → `Positioned`; the emitter methods are only reachable in `Positioned`. Accepts only `Unterminated` blocks. `IrBuilder::at_end(bb)` is the one-step form — it takes the module from the block and infers `R` from it. `b.position_at_end_dyn(block_id)?` is the checked escape for a dynamically discovered CFG, returning `IrError::ForeignValueId` on an id from another module|
|—|`IrBuilder::new_for::<R>(&m)`|new — produces a return-marker-tagged builder for compile-time-checked `ret`|
|—|`m.add_typed_function::<i32, (), _>(name, linkage)?`|typed form — the signature is *derived from* the `Ret`/`Params` markers (no separate `fn_ty`, a marker/signature mismatch is unrepresentable); parameters come back typed via `m.view(id).params()`|
|—|`m.function_builder::<R, _>(name, fn_ty)`|chainable: `.linkage()` / `.calling_conv()` / `.unnamed_addr()` / `.attribute()` / `.return_attribute(kind)` / `.param_attribute(slot, kind)` / `.param_name(slot, name)` / `.build()?`|
||`m.view(f).with_typed_params::<Params>()?`|wraps functions built through the existing `function_builder` path; yields `IrResult<TypedFunctionId<R, Params, B>>`|
||`m.view(f).with_typed_signature::<fn(i32) -> i32>()?`|wraps an existing raw function with a function-pointer schema, yielding the same kind of id|
||`IrBuilder::new_for_return::<fn(i32) -> i32>(&m)`|creates a builder whose return typestate is taken from the function-pointer alias|
|`Builder::build_int_truncate(v, dst, name)`|`b.trunc(value, dst_ty, name)?`|`Src` / `Dst` are inferred from the value and the `IntType<'ctx, Dst, B>` argument; widths checked at compile time via `Src: WiderThan<Dst>` — widening fails to compile. Returns `IrResult<IntValueId<Dst, B>>`|
|—|`b.trunc_dyn(value, dst_ty, name)?`|runtime-checked fallback for `IntDyn`-width paths; errors with `IrError::OperandWidthMismatch`|
|`Builder::build_int_z_extend(v, dst, name)`|`b.zext(value, dst_ty, name)?`|widths checked at compile time via `Dst: WiderThan<Src>`|
|`Builder::build_int_s_extend(v, dst, name)`|`b.sext(value, dst_ty, name)?`|widths checked at compile time via `Dst: WiderThan<Src>`|
|—|`b.zext_dyn` / `b.sext_dyn`|runtime-checked fallbacks for `IntDyn`-width paths|
|`Builder::build_int_compare(p, l, r, name)`|`b.int_cmp(pred, lhs, rhs, name)?`|`pred` is an `IntPredicate`; both operands share width `W` and the result is `IrResult<IntValueId<bool, B>>`. `icmp_eq` / `icmp_ne` / `icmp_ult` / ... are per-predicate shorthands|
|`Builder::build_unconditional_branch(bb)`|`b.br(target)?`|target is any `IntoBasicBlockLabel<'ctx, R, B>` — a `BlockId`, a `BasicBlockLabel`, or an in-scope `BasicBlock`. Its `R` and module brand match the builder, so a foreign *brand* is rejected by the type signature and a foreign-tagged `BlockId` is `IrError::ForeignValueId`. Consumes the builder and returns the insertion block re-tagged `Terminated`|
|`Builder::build_conditional_branch(c, t, e)`|`b.cond_br(cond, then_bb, else_bb)?`|`cond` accepts any `IntoIntValue<'ctx, bool, B>`; consumes the builder like `br`|
|`Builder::build_unreachable()`|`b.unreachable()`|infallible (no operands); returns the (`Terminated` block, instruction) pair directly|
|`Builder::build_phi(ty, name)` + `phi.add_incoming(&[...])`|`b.append_block_with_named_params(m.view(f), [(i32_ty.as_type(), "acc")], "loop")?` returns the block plus its head-phi parameter values; each edge then supplies them, e.g. `b.cond_br_with_args(cond, other_id, &[], loop_id, &[init.as_erased()])?`|**Replaced, not renamed.** The marker-typed phi builders and `PhiInst::add_incoming` are crate-internal; block arguments are the public phi-authoring surface. A block declares its head-phis as *parameters* and each incoming edge supplies *block arguments*, so an edge and its phi operands are written together instead of being patched afterwards. `append_block_with_params` is the unnamed form; `IrBuilder::append_block_typed::<Params>` is the compile-time-checked form, whose `label.call(args)` edge — consumed by `br_call` / `cond_br_call` — makes a wrong-arity or wrong-typed block argument a compile error. `crates/llvmkit-ir/examples/factorial.rs` builds a loop this way end to end.|
|—|`b.phi_dyn(ty, name)?` / `int_phi_dyn` / `fp_phi_dyn` / `pointer_phi_in_addrspace`|the erased phi builders that remain public for parser- and pass-shaped code that must materialise a bare phi. They return a phi id; `m.view(id).remove_incoming(&m, index)?` mirrors `PHINode::removeIncomingValue`. Prefer block arguments in new construction code|
|manual `builder.build_phi` + `phi.add_incoming` for loop-carried values|`SsaState::for_function(&m, m.view(f))?` then `SsaBuilder::for_function(&m, m.view(f), &mut state)?`, plus `declare_int_var::<W, _>` / `declare_float_var` / `declare_pointer_var` and `def_*_var` / `use_*_var`|no direct inkwell counterpart. Braun et al. on-the-fly SSA construction: declare a typed variable once, then read/write it like a mutable local. The engine inserts, completes, and trivial-phi-eliminates the phis itself as blocks are sealed (`seal_block` / `finish`); no manual phi pre-declaration or incoming-edge patching. `SsaState<B>` is owned, `'static`, `Send` and `Clone`, so a lifter can park it in a struct field between steps. See the README's "Auto-SSA" section and `examples/factorial_auto_ssa.rs`.|
|`value.into_pointer_value()` narrowing plus manual bookkeeping of "what this pointer points to"|`ptr.with_pointee::<T>()` → `TypedPointerValue<'ctx, T, B>`|no direct inkwell counterpart. Rust-side-only pointee-schema overlay on an opaque pointer; `typed_alloca::<T, _>(name)` / `typed_load` / `typed_store` / `field_gep::<S, I, _>` skip the runtime type-narrow the erased path needs. These four return the `TypedPointerValue` handle (or the loaded value) rather than an id, since the pointee schema is a Rust-side overlay. Printed IR is byte-identical to the erased path — this is a compile-time ergonomics layer, not a new IR construct.|

## Error model

Inkwell's `BuilderError` becomes llvmkit's crate-level `IrError`
(`thiserror`-flavored). Every fallible path is `Result<T, IrError>`
aliased as `IrResult<T>`. Pure constructors (`module.i32_type()`,
`module.bool_type()`) stay infallible.

There is no `IrError::NoInsertionPoint`: `IrBuilder<'_, 'ctx, B, F, S, R>`
encodes insertion state, and `S = Unpositioned` has no emitter methods. There
is no `IrError::WrongModule` either: two modules with distinct brand types
cannot exchange operands at all, so there is no runtime check to report. Where a
brand is deliberately shared — every `Module::dynamic` module is `DynBrand`, and
a named brand is re-issued once the previous module drops — the runtime
`ModuleId` tag on the id is the backstop, and it reports
`IrError::ForeignValueId` (or `None` from `try_view`, or a panic from `view`).

`IrError` is `#[non_exhaustive]`, so matching on it needs a `_` arm.

## Module brand

llvmkit handles carry a compile-time module brand: a type that names the owning
module. `module_new!` mints one per expansion site (unnameable outside it),
`Module::branded::<B>` takes one you name, `Module::branded_once::<B>` retires it
permanently on drop, and `DynBrand` (via `Module::dynamic`) opts out in favour of
the runtime module tag alone. A process-global registry admits at most one live
module per named brand (`IrError::BrandInUse` / `BrandRetired`); `DynBrand` is
exempt from it, which is precisely why it buys no compile-time separation.
Handles from two distinct brands cannot be mixed in normal code.

The trait is empty and unsealed, so you can name your own brand — a bare unit
struct is a complete declaration:

```rust
struct LiftedBin;
impl llvmkit_ir::ModuleBrand for LiftedBin {}

let m = llvmkit_ir::Module::branded::<LiftedBin, _>("lifted")?;
```

`ModuleBrand: 'static` — nothing more. The `'static` exists because the
uniqueness registry keys brands by `TypeId`. (Until the 0.0.4 freeze the trait
also required `Copy + Debug + Eq + Hash`, an artifact of the brand-generic
handles using std `#[derive]`; they now use a bound-free derive, so the
requirement is gone.)

Because a brand is a type rather than a lifetime, the module token is an ordinary
owned value: `Module<B, Unverified>` has no lifetime parameter. Handles still
borrow it (`Value<'ctx, B>`), while storable ids (`ValueId`, `FunctionId`,
`BlockId`, …) carry the brand without the borrow and so outlive the module — the
runtime `ModuleId` tag is what refuses a stale one.

**Metadata is covered too.** It was the last currency outside the id model — a
bare `MetadataSlot` arena index with neither a `ModuleId` tag nor a brand — and
the 0.0.4 freeze closed that: the public currency is `MetadataId<B>`, so a
mix-up across named brands is a compile error and within one brand it is
`IrError::ForeignMetadataId` (out-of-range native ids stay
`IrError::UnknownMetadataSlot`). Named metadata follows the same shape:
`NamedMetadataId<B>` plus the validated `NamedMetadataName` replace the raw
index, `m.named_metadata(&name)` is the lookup, and `named_metadata_get(id)`
resolves an id back to its node.

## Things you give up

- **No FFI escape hatch, and no raw handle at all.** There is no
  `LLVMValueRef` / `LLVMModuleRef` equivalent and no public `ModuleCore`, so
  there is nothing to drop down to when the modeled surface stops short.
  Mutation requires the unverified `Module` token, and verification consumes
  that capability.
- **No code generation.** llvmkit ends at IR; lowering / linking still
  goes through upstream LLVM.
- **No bitcode, and no runnable optimization pipeline.** `.bc` reading and
  writing are unimplemented, and there is no `PassBuilder` / `default<O2>` to
  run — the capability-graded pass API and a small set of built-in analyses and
  cleanup passes ship, but not a transform library.


## AsmWriter (`format!("{module}")`)

`Module`, `FunctionValue`, `BasicBlock`, `Instruction`, and `Value` implement
`Display` and produce real `.ll` (as do `ModuleView` and `InstructionView`, so a
pass can print what it is looking at). The printer mirrors
`llvm/lib/IR/AsmWriter.cpp` for the shipped opcode surface: arithmetic, casts,
memory, GEP, calls, select, phi, terminators / EH / atomics, globals, target
directives, module asm, and modeled metadata forms. Slot numbering for unnamed
values shares a single per-function counter (arguments, block labels,
instruction results), matching the upstream `SlotTracker`.

## Compile-time invariants

llvmkit promotes LLVM runtime checks into Rust types where the modeled surface
can make invalid states unrepresentable. The current ledger of bugs that
compile down to a Rust type error rather than a runtime `IrError` — 82
compile-fail fixtures in `crates/llvmkit-ir/tests/compile_fail/` lock these:

- The IrBuilder must be positioned (`Unpositioned` has no emitter methods).
- Terminator builders consume the `Positioned` builder *and* the block token,
  and re-tag the insertion block `Terminated`; `position_at_end` accepts only
  `Unterminated` blocks. Appending past a terminator is unrepresentable, and a
  second terminator on the same builder is a use-of-moved-value error.
- Integer-arithmetic operands must share a width (`W: IntWidth`).
- `trunc`'s source / destination widths are tagged statically;
  the runtime check only fires on the `_dyn`-flavoured fallbacks.
- `zext` / `sext` reject narrowing the same way: the
  destination width must be statically wider than the source.
- `int_cmp`'s result is statically `IntValueId<bool, B>`
  (`i1`); downstream `cond_br` accepts it without further
  narrowing.
- `cond_br`'s condition slot accepts any `IntoIntValue<'ctx, bool, B>`.
- Branch targets share the parent function's `R` so the typed-return
  invariant flows transitively across branches.
- Block arguments on a typed branch edge match the target block's declared
  parameter schema — `label.call(args)` carries a `CallArgs<Params>` bound, so
  a wrong arity or a wrong-typed position does not compile.
- `ret` on a typed-return builder requires a value of the
  function's exact return shape (`i32` / `f32` / `Ptr` / `()` markers).
- `ret_void` is *only* reachable on a `void`-returning builder (`()`)
  or on `Dyn` with a runtime check.
- The runtime `IrError::ReturnTypeMismatch` survives only on the `Dyn` path
  — every static marker enforces the invariant at compile time.
- A borrowing handle cannot outlive the owned module it was minted from
  (`E0597`), which is exactly why the `.id()` form exists: the same program
  written against ids compiles.
- Two modules with distinct brand types cannot exchange handles, ids, or
  builders at all — no runtime check is involved and no `IrError` variant
  exists for it.
- Verification consumes `Module<B, Unverified>` and yields `Module<B, Verified>`.
- A pass pipeline's output typestate is derived from its members' capability
  rungs: an all-`Inspect` (read-only) run preserves `Module<B, Verified>`, while
  any mutating pass downgrades it to `Module<B, Unverified>`.
- Saved-handle mutators require `&Module<B, Unverified>`, so verified modules
  cannot be mutated through old handles without explicitly `unverify()`ing. As
  of 0.0.4 this includes the instruction metadata mutators
  (`InstructionView::set_metadata`, `push_debug_record`, and their
  `Instruction` twins), which previously took no token.