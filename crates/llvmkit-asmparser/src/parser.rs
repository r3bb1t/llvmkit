//! Public parser facade.
//!
//! Mirrors `llvm/include/llvm/AsmParser/Parser.h` and
//! `llvm/lib/AsmParser/Parser.cpp`: callers use these stateless helpers for
//! one-shot parsing, while [`crate::ll_parser::Parser`] keeps the recursive
//! descent state private to the parsing operation.

use llvmkit_ir::{Constant, DynBrand, Module, ModuleBrand, Type, Unverified};

use super::asm_parser_context::AsmParserContext;
use llvmkit_ir::module_summary_index::ModuleSummaryIndex;

use super::ll_parser::{ParsedModule, Parser};
use super::parse_error::ParseResult;
use super::slot_mapping::SlotMapping;

// --------------------------------------------------------------------------
// Parser configuration
// --------------------------------------------------------------------------

/// Override for a module's data layout string.
///
/// Mirrors `DataLayoutCallbackTy`
/// (`llvm/include/llvm/AsmParser/Parser.h`), which is
/// `function_ref<std::optional<std::string>(StringRef, StringRef)>`: it is
/// handed the module's target triple and the layout string the file spelled,
/// and answers `Some(replacement)` to substitute one or `None` to keep the
/// file's. Borrowed rather than boxed, as `function_ref` is.
///
/// The point of the hook is importing a module whose layout string this build
/// cannot parse — the callback replaces it before
/// [`DataLayout::parse`](llvmkit_ir::DataLayout::parse) ever sees it.
pub type DataLayoutCallback<'cfg> = &'cfg dyn Fn(&str, &str) -> Option<String>;

/// What a parse run is configured with.
///
/// Gathers the two parameters of `LLParser::Run` — `UpgradeDebugInfo` and
/// `DataLayoutCallbackTy` — with the `-allow-incomplete-ir` `cl::opt` that
/// `LLParser.cpp` reads directly off the command line. llvmkit has no command
/// line, so the option travels with the other two.
///
/// Every entry point without a `_with_config` twin uses
/// [`ParserConfig::DEFAULT`], which is what `parseAssembly`,
/// `parseAssemblyFile` and `parseAssemblyWithIndex` pass.
#[derive(Clone, Copy)]
pub struct ParserConfig<'cfg> {
    /// Accept incomplete IR on a best-effort basis. Mirrors LLVM's
    /// `-allow-incomplete-ir` (`static cl::opt<bool> AllowIncompleteIR` in
    /// `LLParser.cpp`), **off** by default there and here.
    ///
    /// Upstream's `validateEndOfModule` reads it at three places. llvmkit
    /// implements the first:
    ///
    /// 1. **Implemented.** A leftover `ForwardRefVals` entry that is not an
    ///    intrinsic gets a declaration synthesised instead of ending the parse
    ///    with `use of undefined value '@x'` — a function at the one signature
    ///    every call site used (`GetCommonFunctionType`), or an `i8` global
    ///    when the uses disagree, are not calls, or do not exist.
    /// 2. **Not implemented.** `dropUnknownMetadataReferences`, which erases
    ///    attachments naming an undefined `!N` and the dbg / noalias-scope
    ///    intrinsics that carry one. llvmkit resolves metadata forward
    ///    references by reserve-then-fill on a stable
    ///    [`MetadataId`](llvmkit_ir::metadata::MetadataId) rather than through
    ///    temporary nodes, and has no attachment-removal API for the four
    ///    holders to erase through (`docs/divergences.md` D13).
    /// 3. **Not applicable.** The relaxed `InstsWithTBAATag` assertion, which
    ///    exists only because `UpgradeTBAANode` runs there. llvmkit has no
    ///    `AutoUpgrade` port, so there is no TBAA upgrade to tolerate a drop
    ///    in.
    pub allow_incomplete_ir: bool,
    /// Run the debug-info auto-upgrade at end of module. Mirrors `Run`'s
    /// `UpgradeDebugInfo` flag: `true` for every caller except `llvm-as` and
    /// `opt -disable-upgrade-debug-info`, which reach
    /// `parseAssemblyFileWithIndexNoUpgradeDebugInfo`.
    ///
    /// **Selects nothing today.** The flag guards exactly one statement
    /// upstream — `if (UpgradeDebugInfo) llvm::UpgradeDebugInfo(*M);` — and
    /// llvmkit has no `AutoUpgrade` port for it to guard
    /// (`docs/divergences.md` D11, and entry 19 for the missing port). It is
    /// carried so the entry-point surface is
    /// upstream's and so callers that mean `llvm-as` can say so; it starts
    /// selecting behaviour the moment the upgrade lands.
    pub upgrade_debug_info: bool,
    /// Replace the file's `target datalayout` string. `None` is upstream's
    /// default argument, the callback that always answers `std::nullopt`.
    /// Upstream's real callers of a non-default callback are `llvm-link` and
    /// the ThinLTO importers, both of which reach it through
    /// `parseAssemblyFileWithIndex` — a file-reading entry point, since
    /// llvmkit's parser reads no files itself.
    pub data_layout_callback: Option<DataLayoutCallback<'cfg>>,
}

impl ParserConfig<'_> {
    /// The configuration every plain entry point runs under: no incomplete
    /// IR, debug-info upgrade on, no data-layout override.
    pub const DEFAULT: Self = Self {
        allow_incomplete_ir: false,
        upgrade_debug_info: true,
        data_layout_callback: None,
    };

    /// [`ParserConfig::DEFAULT`], as a function.
    #[inline]
    pub const fn new() -> Self {
        Self::DEFAULT
    }
}

impl Default for ParserConfig<'_> {
    #[inline]
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Reports whether a callback is installed, never the callback itself — a
/// `&dyn Fn` has nothing printable about it.
impl core::fmt::Debug for ParserConfig<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ParserConfig")
            .field("allow_incomplete_ir", &self.allow_incomplete_ir)
            .field("upgrade_debug_info", &self.upgrade_debug_info)
            .field(
                "data_layout_callback",
                &self.data_layout_callback.map(|_| "<callback>"),
            )
            .finish()
    }
}

// --------------------------------------------------------------------------
// Owned-module entry points
// --------------------------------------------------------------------------
//
// These are the ordinary way to parse. They take no closure and hand back the
// module itself, so a caller can `verify()` it, store it in a struct, push it
// into a `Vec`, or move it to another thread — none of which the closure forms
// below allow.
//
// What they do *not* hand back is the [`ParsedModule`] slot-mapping
// by-product. That is not an oversight: `ParsedModule` holds `SlotMapping`,
// whose `GlobalRef` / `Type` entries are *borrowing handles* into the module
// it was parsed against. Returning both would be returning a struct and a
// borrow of that struct from one call — a self-reference Rust cannot express.
// Callers who need the slot tables keep using the closure forms; the closure
// is what supplies a region the by-product can borrow for.

/// Parse a complete textual IR module into `module`, returning it.
///
/// The primitive the other owned-module entry points are built from. The
/// caller constructs the module, so it picks the name and the brand —
/// [`module_new!`](llvmkit_ir::module_new),
/// [`Module::branded`](llvmkit_ir::Module::branded),
/// [`Module::branded_once`](llvmkit_ir::Module::branded_once), or
/// [`Module::dynamic`](llvmkit_ir::Module::dynamic) — and this never has to
/// invent either.
///
/// ```
/// use llvmkit_asmparser::parse_into;
/// use llvmkit_ir::module_new;
///
/// let m = parse_into(module_new!("lifted")?, "define void @f() {\nentry:\n  ret void\n}\n")?;
/// let m = m.verify()?;
/// assert!(m.to_string().contains("define void @f()"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Errors
///
/// Any [`crate::parse_error::ParseError`] the source provokes. On failure the module is dropped
/// along with whatever was parsed into it, so a half-built module never
/// escapes.
pub fn parse_into<B, S>(module: Module<B, Unverified>, src: S) -> ParseResult<Module<B, Unverified>>
where
    B: ModuleBrand,
    S: AsRef<[u8]>,
{
    parse_into_with_config(module, src, &ParserConfig::DEFAULT)
}

/// [`parse_into`] under an explicit [`ParserConfig`].
///
/// The primitive every other `_with_config` entry point is built from; it is
/// `parseAssemblyInto`'s role, the low-level form upstream keeps separate "for
/// the convenience of interactive users that want to add recently parsed bits
/// to an existing module".
///
/// # Errors
///
/// Any [`crate::parse_error::ParseError`] the source provokes. On failure the module is dropped
/// along with whatever was parsed into it.
pub fn parse_into_with_config<B, S>(
    module: Module<B, Unverified>,
    src: S,
    config: &ParserConfig<'_>,
) -> ParseResult<Module<B, Unverified>>
where
    B: ModuleBrand,
    S: AsRef<[u8]>,
{
    // The `ParsedModule` by-product borrows `module`; dropping it here ends
    // that borrow, which is what lets the token be returned by value.
    Parser::new(src.as_ref(), &module)?.parse_module_with_config(config)?;
    Ok(module)
}

/// Parse a complete textual IR module under [`DynBrand`], returning the owned
/// module.
///
/// Named `"<string>"`, matching `parseAssemblyString`'s `MemoryBufferRef
/// F(AsmString, "<string>")`; use [`parse_into`] with
/// [`Module::dynamic`](llvmkit_ir::Module::dynamic) to choose a different
/// name.
///
/// Infallible in the brand: `DynBrand` is registry-exempt, so this can be
/// called any number of times concurrently and every result is a separate
/// module, separated by the runtime [`ModuleId`](llvmkit_ir::ModuleId) tag.
///
/// ```
/// use llvmkit_asmparser::parse_dynamic;
///
/// let modules = ["@a = global i32 1\n", "@b = global i32 2\n"]
///     .into_iter()
///     .map(parse_dynamic)
///     .collect::<Result<Vec<_>, _>>()?;
/// assert_eq!(modules.len(), 2);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn parse_dynamic<S>(src: S) -> ParseResult<Module<DynBrand, Unverified>>
where
    S: AsRef<[u8]>,
{
    parse_dynamic_with_config(src, &ParserConfig::DEFAULT)
}

/// [`parse_dynamic`] under an explicit [`ParserConfig`].
///
/// ```
/// use llvmkit_asmparser::{ParserConfig, parse_dynamic_with_config};
///
/// // `@undefined` is never declared; `-allow-incomplete-ir` declares it at
/// // the one signature its call site uses instead of failing the parse.
/// let config = ParserConfig {
///     allow_incomplete_ir: true,
///     ..ParserConfig::DEFAULT
/// };
/// let src = "define void @f() {\nentry:\n  call void @undefined()\n  ret void\n}\n";
/// let m = parse_dynamic_with_config(src, &config)?;
/// assert!(m.to_string().contains("declare void @undefined()"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Errors
///
/// Any [`crate::parse_error::ParseError`] the source provokes.
pub fn parse_dynamic_with_config<S>(
    src: S,
    config: &ParserConfig<'_>,
) -> ParseResult<Module<DynBrand, Unverified>>
where
    S: AsRef<[u8]>,
{
    parse_into_with_config(Module::dynamic("<string>"), src, config)
}

// --------------------------------------------------------------------------
// Closure entry points (slot-mapping by-product)
// --------------------------------------------------------------------------

/// Parse a complete textual IR module and inspect it together with its
/// [`ParsedModule`] slot mapping.
///
/// Named `"<string>"`, matching `parseAssemblyString`'s `MemoryBufferRef
/// F(AsmString, "<string>")`; use [`parse_assembly_with_name`] to choose a
/// different name.
///
/// Prefer [`parse_dynamic`] unless you need the slot tables: it returns the
/// module by value, so it can be verified, stored, and moved. This form
/// exists because [`ParsedModule`] *borrows* the module it was
/// parsed against, so the two cannot both be returned from one call — the
/// closure is what provides a region for the by-product to borrow for.
///
/// The module carries [`DynBrand`], the registry-exempt brand: a parse can be
/// re-entered and run concurrently any number of times, so a registry brand
/// would be claimed once and refused ever after. Distinct parses are therefore
/// separated by the runtime [`ModuleId`](llvmkit_ir::ModuleId) tag, not by
/// type. The bound stays higher-ranked over `'ctx` because that borrow is of a
/// local the caller cannot name.
pub fn parse_assembly<R, S, F>(src: S, f: F) -> ParseResult<R>
where
    S: AsRef<[u8]>,
    F: for<'ctx> FnOnce(&'ctx Module<DynBrand, Unverified>, ParsedModule<'ctx, DynBrand>) -> R,
{
    parse_assembly_with_name("<string>", src, &ParserConfig::DEFAULT, f)
}

/// [`parse_assembly`] under an explicit [`ParserConfig`].
pub fn parse_assembly_with_config<R, S, F>(
    src: S,
    config: &ParserConfig<'_>,
    f: F,
) -> ParseResult<R>
where
    S: AsRef<[u8]>,
    F: for<'ctx> FnOnce(&'ctx Module<DynBrand, Unverified>, ParsedModule<'ctx, DynBrand>) -> R,
{
    parse_assembly_with_name("<string>", src, config, f)
}

/// Parse a complete textual IR module under a caller-supplied module name, and
/// inspect it together with its [`ParsedModule`] slot mapping.
///
/// Mirrors `parseAssembly(MemoryBufferRef F, …)`, whose `F` carries the buffer
/// identifier that becomes `M->getModuleIdentifier()`. llvmkit hangs the
/// identifier on the [`Module`] instead, so the closure forms — which
/// construct the module themselves — take it as a parameter here.
/// [`parse_assembly`] and [`parse_assembly_with_config`] pass the fixed name
/// `"<string>"`, matching `parseAssemblyString`'s `MemoryBufferRef
/// F(AsmString, "<string>")`; this function publishes that parameter so a
/// caller can pick a different name instead.
///
/// # Errors
///
/// Any [`crate::parse_error::ParseError`] the source provokes.
pub fn parse_assembly_with_name<R, S, F>(
    name: &str,
    src: S,
    config: &ParserConfig<'_>,
    f: F,
) -> ParseResult<R>
where
    S: AsRef<[u8]>,
    F: for<'ctx> FnOnce(&'ctx Module<DynBrand, Unverified>, ParsedModule<'ctx, DynBrand>) -> R,
{
    let module = Module::dynamic(name);
    let parsed = Parser::new(src.as_ref(), &module)?.parse_module_with_config(config)?;
    Ok(f(&module, parsed))
}

/// Parse a complete textual IR module *and* the module summary index its `^N`
/// entries describe.
///
/// Mirrors `parseAssemblyWithIndex`: [`parse_assembly`] passes upstream a null
/// `ModuleSummaryIndex` and so skips every summary entry, where this one fills
/// [`ParsedModule::summary_index`].
///
/// [`ParsedModule`] is upstream's `ParsedModuleAndIndex` with a wider job: it
/// carries the slot mapping and the file-location registry beside the index,
/// because all three borrow the module the closure is lent.
///
/// The closure receives the module by reference; see [`parse_assembly`].
pub fn parse_assembly_with_index<R, S, F>(src: S, f: F) -> ParseResult<R>
where
    S: AsRef<[u8]>,
    F: for<'ctx> FnOnce(&'ctx Module<DynBrand, Unverified>, ParsedModule<'ctx, DynBrand>) -> R,
{
    parse_assembly_with_index_and_config(src, &ParserConfig::DEFAULT, f)
}

/// [`parse_assembly_with_index`] under an explicit [`ParserConfig`]. Mirrors
/// `parseAssemblyWithIndex(MemoryBufferRef F, …)`, the buffer-taking
/// primitive `parseAssemblyFileWithIndex` and its `…NoUpgradeDebugInfo` twin
/// wrap after reading a file; the two file-taking wrappers differ from each
/// other only in what they pass through to it.
///
/// Its default module name is `"<string>"`, upstream's convention for a
/// caller-supplied buffer with no name of its own — an inference rather than
/// a direct port, since `parseAssemblyWithIndex` has no string-only overload
/// to take the identifier from.
pub fn parse_assembly_with_index_and_config<R, S, F>(
    src: S,
    config: &ParserConfig<'_>,
    f: F,
) -> ParseResult<R>
where
    S: AsRef<[u8]>,
    F: for<'ctx> FnOnce(&'ctx Module<DynBrand, Unverified>, ParsedModule<'ctx, DynBrand>) -> R,
{
    let module = Module::dynamic("<string>");
    let parsed =
        Parser::with_summary_index(src.as_ref(), &module)?.parse_module_with_config(config)?;
    Ok(f(&module, parsed))
}

/// Parse a textual LLVM module summary index, reading past everything else.
///
/// Mirrors `parseSummaryIndexAssembly`, which runs `LLParser` with a null
/// `Module`: only `^N` entries and `source_filename` are read, and every other
/// top-level entity is lexed past.
pub fn parse_summary_index_assembly<S: AsRef<[u8]>>(src: S) -> ParseResult<ModuleSummaryIndex> {
    // The module exists only so the parser has somewhere to record the source
    // file name, which is what a local symbol's GUID is computed from. Nothing
    // is built into it, and the index does not borrow it.
    let module = Module::dynamic("summary");
    let parsed = Parser::summary_index_only(src.as_ref(), &module)?.parse_module()?;
    Ok(parsed.summary_index.unwrap_or_default())
}

/// Parse a complete textual IR module and return source locations inside the closure.
///
/// Mirrors `parseAssemblyString(…, AsmParserContext *)`: the ranges are
/// recorded by the parser itself, at the three sites `LLParser` records them —
/// `parseDefine` for a function, `parseBasicBlock` for a block and for each of
/// its instructions — each spanning the construct's first token to
/// `LLLexer::PrevTokEnd`.
///
/// The closure receives the module by reference; see [`parse_assembly`].
pub fn parse_assembly_with_context<R, S, F>(src: S, f: F) -> ParseResult<R>
where
    S: AsRef<[u8]>,
    F: for<'ctx> FnOnce(
        &'ctx Module<DynBrand, Unverified>,
        ParsedModule<'ctx, DynBrand>,
        AsmParserContext<'ctx, DynBrand>,
    ) -> R,
{
    parse_assembly_with_context_and_config(src, &ParserConfig::DEFAULT, f)
}

/// [`parse_assembly_with_context`] under an explicit [`ParserConfig`].
pub fn parse_assembly_with_context_and_config<R, S, F>(
    src: S,
    config: &ParserConfig<'_>,
    f: F,
) -> ParseResult<R>
where
    S: AsRef<[u8]>,
    F: for<'ctx> FnOnce(
        &'ctx Module<DynBrand, Unverified>,
        ParsedModule<'ctx, DynBrand>,
        AsmParserContext<'ctx, DynBrand>,
    ) -> R,
{
    let module = Module::dynamic("<string>");
    let mut parsed =
        Parser::with_context(src.as_ref(), &module)?.parse_module_with_config(config)?;
    // `Parser::with_context` installs the registry, so `parse_module` always
    // hands one back; the `unwrap_or_default` is the shape of the `Option`,
    // not a fallback anyone reaches.
    let context = parsed.parser_context.take().unwrap_or_default();
    Ok(f(&module, parsed, context))
}

/// Parse a single LLVM type and require end-of-input.
///
/// Mirrors `llvm::parseType` (`Parser.cpp`), which runs
/// `parseTypeAtBeginning` and then reports `expected end of string` when the
/// type did not consume the whole buffer. llvmkit reaches the same message
/// through `require_eof`, because trailing garbage now arrives as
/// [`Token::Error`](crate::ll_token::Token::Error) rather than aborting the
/// lexer.
pub fn parse_type<'ctx, B: ModuleBrand + 'ctx, S: AsRef<[u8]>>(
    src: S,
    module: &'ctx Module<B, Unverified>,
) -> ParseResult<Type<'ctx, B>> {
    Parser::new(src.as_ref(), module)?.parse_standalone_type()
}

/// [`parse_type`], resolving numbered/named forward references through a
/// caller-supplied slot mapping (parsed-IR workflows).
pub fn parse_type_with_slots<'ctx, B: ModuleBrand + 'ctx, S: AsRef<[u8]>>(
    src: S,
    module: &'ctx Module<B, Unverified>,
    slots: &SlotMapping<'ctx, B>,
) -> ParseResult<Type<'ctx, B>> {
    Parser::with_slot_mapping(src.as_ref(), module, slots)?.parse_standalone_type()
}

/// Parse one LLVM type prefix and report the number of consumed bytes.
pub fn parse_type_at_beginning<'ctx, B: ModuleBrand + 'ctx, S: AsRef<[u8]>>(
    src: S,
    module: &'ctx Module<B, Unverified>,
) -> ParseResult<(Type<'ctx, B>, usize)> {
    Parser::new(src.as_ref(), module)?.parse_type_at_beginning()
}

/// [`parse_type_at_beginning`] with a caller-supplied slot mapping.
pub fn parse_type_at_beginning_with_slots<'ctx, B: ModuleBrand + 'ctx, S: AsRef<[u8]>>(
    src: S,
    module: &'ctx Module<B, Unverified>,
    slots: &SlotMapping<'ctx, B>,
) -> ParseResult<(Type<'ctx, B>, usize)> {
    Parser::with_slot_mapping(src.as_ref(), module, slots)?.parse_type_at_beginning()
}

/// Parse one constant value of the supplied LLVM type and require EOF.
pub fn parse_constant_value<'ctx, B: ModuleBrand + 'ctx, S: AsRef<[u8]>>(
    src: S,
    module: &'ctx Module<B, Unverified>,
    ty: Type<'ctx, B>,
) -> ParseResult<Constant<'ctx, B>> {
    Parser::new(src.as_ref(), module)?.parse_standalone_constant_value(ty)
}

/// [`parse_constant_value`] with a caller-supplied slot mapping.
pub fn parse_constant_value_with_slots<'ctx, B: ModuleBrand + 'ctx, S: AsRef<[u8]>>(
    src: S,
    module: &'ctx Module<B, Unverified>,
    ty: Type<'ctx, B>,
    slots: &SlotMapping<'ctx, B>,
) -> ParseResult<Constant<'ctx, B>> {
    Parser::with_slot_mapping(src.as_ref(), module, slots)?.parse_standalone_constant_value(ty)
}

// The line-scanning heuristic that used to stand in for `AsmParserContext`
// lived here: it re-read the source after the parse, matched `"define "` and
// `"@name("` and `"}"` textually, and handed every instruction the whole
// remaining line. It is gone — `LLParser` records token positions at the three
// sites it builds the constructs, and so does llvmkit now
// (`Parser::record_function_location` / `record_block_location` and the tail of
// `finish_trailing_metadata`).
