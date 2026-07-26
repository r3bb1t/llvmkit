//! Public parser facade.
//!
//! Mirrors `llvm/include/llvm/AsmParser/Parser.h` and
//! `llvm/lib/AsmParser/Parser.cpp`: callers use these stateless helpers for
//! one-shot parsing, while [`crate::ll_parser::Parser`] keeps the recursive
//! descent state private to the parsing operation.

use std::fs::read as read_file;
use std::path::Path;
use std::str::from_utf8;

use llvmkit_ir::{Constant, DynBrand, IrError, Module, ModuleBrand, Type, Unverified};

use super::file_loc::{FileLoc, FileLocRange};

use super::asm_parser_context::AsmParserContext;
use super::ll_parser::{ParsedModule, Parser};
use super::module_summary::{self, ModuleSummaryIndex};
use super::parse_error::{ParseError, ParseResult};
use super::slot_mapping::SlotMapping;

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
/// Any [`ParseError`] the source provokes. On failure the module is dropped
/// along with whatever was parsed into it, so a half-built module never
/// escapes.
pub fn parse_into<B, S>(module: Module<B, Unverified>, src: S) -> ParseResult<Module<B, Unverified>>
where
    B: ModuleBrand,
    S: AsRef<[u8]>,
{
    // The `ParsedModule` by-product borrows `module`; dropping it here ends
    // that borrow, which is what lets the token be returned by value.
    Parser::new(src.as_ref(), &module)?.parse_module()?;
    Ok(module)
}

/// Parse a complete textual IR module under the **named** brand `B`, returning
/// the owned module.
///
/// ```
/// use llvmkit_asmparser::parse_branded;
/// use llvmkit_ir::ModuleBrand;
///
/// #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// struct Lifted;
/// impl ModuleBrand for Lifted {}
///
/// let m = parse_branded::<Lifted, _>("define void @f() {\nentry:\n  ret void\n}\n")?;
/// let m = m.verify()?;
/// assert!(m.to_string().contains("define void @f()"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Errors
///
/// [`ParseError::BrandInUse`] / [`ParseError::BrandRetired`] if `B` is not
/// available, plus any [`ParseError`] the source provokes.
pub fn parse_branded<B, S>(src: S) -> ParseResult<Module<B, Unverified>>
where
    B: ModuleBrand,
    S: AsRef<[u8]>,
{
    parse_into(branded_module::<B>("asm")?, src)
}

/// Parse a complete textual IR module under [`DynBrand`], returning the owned
/// module.
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
    parse_into(Module::dynamic("asm"), src)
}

/// Read and parse a file under the named brand `B`, returning the owned
/// module. The module is named after the file.
///
/// # Errors
///
/// [`ParseError::Io`] if the file cannot be read,
/// [`ParseError::BrandInUse`] / [`ParseError::BrandRetired`] if `B` is not
/// available, plus any [`ParseError`] the source provokes.
pub fn parse_file_branded<B, P>(path: P) -> ParseResult<Module<B, Unverified>>
where
    B: ModuleBrand,
    P: AsRef<Path>,
{
    let path = path.as_ref();
    let bytes = read_file(path).map_err(|e| ParseError::Io(e.to_string()))?;
    parse_into(branded_module::<B>(module_name_for(path))?, bytes)
}

/// Read and parse a file under [`DynBrand`], returning the owned module. The
/// module is named after the file.
///
/// # Errors
///
/// [`ParseError::Io`] if the file cannot be read, plus any [`ParseError`] the
/// source provokes.
pub fn parse_file_dynamic<P>(path: P) -> ParseResult<Module<DynBrand, Unverified>>
where
    P: AsRef<Path>,
{
    let path = path.as_ref();
    let bytes = read_file(path).map_err(|e| ParseError::Io(e.to_string()))?;
    parse_into(Module::dynamic(module_name_for(path)), bytes)
}

/// Claim brand `B`, translating the registry's refusal into a [`ParseError`].
fn branded_module<B: ModuleBrand>(name: &str) -> ParseResult<Module<B, Unverified>> {
    Module::branded::<B>(name).map_err(|err| match err {
        IrError::BrandRetired { brand } => ParseError::BrandRetired { brand },
        // `Module::branded` reports exactly `BrandInUse` or `BrandRetired`.
        IrError::BrandInUse { brand } => ParseError::BrandInUse { brand },
        other => ParseError::Io(other.to_string()),
    })
}

/// Module name for a parsed file: the file name, or `"asm"` if the path has
/// none (or one that is not UTF-8).
fn module_name_for(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("asm")
}

// --------------------------------------------------------------------------
// Closure entry points (slot-mapping by-product)
// --------------------------------------------------------------------------

/// Parse a complete textual IR module and inspect it together with its
/// [`ParsedModule`] slot mapping.
///
/// Prefer [`parse_dynamic`] / [`parse_branded`] unless you need the slot
/// tables: they return the module by value, so it can be verified, stored, and
/// moved. This form exists because [`ParsedModule`] *borrows* the module it was
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
    parse_assembly_with_name("asm", src, f)
}

fn parse_assembly_with_name<R, S, F>(name: &str, src: S, f: F) -> ParseResult<R>
where
    S: AsRef<[u8]>,
    F: for<'ctx> FnOnce(&'ctx Module<DynBrand, Unverified>, ParsedModule<'ctx, DynBrand>) -> R,
{
    let module = Module::dynamic(name);
    let parsed = Parser::new(src.as_ref(), &module)?.parse_module()?;
    Ok(f(&module, parsed))
}

/// Parse a complete textual IR module from a UTF-8 string under a fresh brand.
///
/// The closure receives the module by reference; see [`parse_assembly`].
pub fn parse_assembly_string<R, F>(src: &str, f: F) -> ParseResult<R>
where
    F: for<'ctx> FnOnce(&'ctx Module<DynBrand, Unverified>, ParsedModule<'ctx, DynBrand>) -> R,
{
    parse_assembly(src.as_bytes(), f)
}

/// Read and parse a complete textual IR module under a fresh module brand.
///
/// The closure receives the module by reference; see [`parse_assembly`].
pub fn parse_assembly_file<R, P, F>(path: P, f: F) -> ParseResult<R>
where
    P: AsRef<Path>,
    F: for<'ctx> FnOnce(&'ctx Module<DynBrand, Unverified>, ParsedModule<'ctx, DynBrand>) -> R,
{
    let path = path.as_ref();
    let module_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("asm");
    let bytes = read_file(path).map_err(|e| ParseError::Io(e.to_string()))?;
    parse_assembly_with_name(module_name, bytes, f)
}

/// Parse a textual LLVM module summary index from bytes.
pub fn parse_summary_index_assembly(src: &[u8]) -> ParseResult<ModuleSummaryIndex> {
    module_summary::parse_summary_index(src)
}

/// Read and parse a textual LLVM module summary index.
pub fn parse_summary_index_assembly_file<P>(path: P) -> ParseResult<ModuleSummaryIndex>
where
    P: AsRef<Path>,
{
    let bytes = read_file(path).map_err(|e| ParseError::Io(e.to_string()))?;
    parse_summary_index_assembly(&bytes)
}

/// Parse a complete textual IR module and return source locations inside the closure.
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
    let module = Module::dynamic("asm");
    let bytes = src.as_ref();
    let parsed = Parser::new(bytes, &module)?.parse_module()?;
    let mut context = AsmParserContext::new();
    record_parser_context(bytes, &module, &mut context)?;
    Ok(f(&module, parsed, context))
}

/// Parse a single LLVM type and require end-of-input.
pub fn parse_type<'ctx, B: ModuleBrand + 'ctx>(
    src: &[u8],
    module: &'ctx Module<B, Unverified>,
    slots: Option<&SlotMapping<'ctx, B>>,
) -> ParseResult<Type<'ctx, B>> {
    let parser = match slots {
        Some(slots) => Parser::with_slot_mapping(src, module, slots)?,
        None => Parser::new(src, module)?,
    };
    parser.parse_standalone_type().map_err(|err| match err {
        ParseError::Lex(crate::ll_lexer::LexError::UnknownToken { span }) => ParseError::Expected {
            expected: "end of string".into(),
            loc: crate::parse_error::DiagLoc::span(span),
        },
        other => other,
    })
}

/// Parse one LLVM type prefix and report the number of consumed bytes.
pub fn parse_type_at_beginning<'ctx, B: ModuleBrand + 'ctx>(
    src: &[u8],
    module: &'ctx Module<B, Unverified>,
    slots: Option<&SlotMapping<'ctx, B>>,
) -> ParseResult<(Type<'ctx, B>, usize)> {
    let parser = match slots {
        Some(slots) => Parser::with_slot_mapping(src, module, slots)?,
        None => Parser::new(src, module)?,
    };
    parser.parse_type_at_beginning()
}

/// Parse one constant value of the supplied LLVM type and require EOF.
pub fn parse_constant_value<'ctx, B: ModuleBrand + 'ctx>(
    src: &[u8],
    module: &'ctx Module<B, Unverified>,
    ty: Type<'ctx, B>,
    slots: Option<&SlotMapping<'ctx, B>>,
) -> ParseResult<Constant<'ctx, B>> {
    let parser = match slots {
        Some(slots) => Parser::with_slot_mapping(src, module, slots)?,
        None => Parser::new(src, module)?,
    };
    parser.parse_standalone_constant_value(ty)
}

fn record_parser_context<'ctx, B: ModuleBrand + 'ctx>(
    src: &[u8],
    module: &'ctx Module<B, Unverified>,
    context: &mut AsmParserContext<'ctx, B>,
) -> ParseResult<()> {
    let lines = source_lines(src);
    for function_view in module.as_view().functions() {
        let Some(function) = module
            .function_by_name_dyn(function_view.name())
            .map(|id| module.view(id))
        else {
            continue;
        };
        let Some((start, end)) = function_range(&lines, Some(function.name())) else {
            continue;
        };
        context
            .add_function_location(function, FileLocRange::new(start, end))
            .map_err(location_error)?;

        let mut instruction_lines = instruction_lines_in_range(&lines, start.line, end.line);
        for block in function.basic_blocks() {
            let block_start = match block
                .name()
                .and_then(|name| label_line_in_range(&lines, start.line, end.line, &name))
                .or_else(|| instruction_lines.first().copied())
            {
                Some(loc) => loc,
                None => start,
            };
            context
                .add_block_location(&block, FileLocRange::new(block_start, end))
                .map_err(location_error)?;
            for instruction in block.instructions() {
                let Some(inst_start) = instruction_lines.first().copied() else {
                    break;
                };
                instruction_lines.remove(0);
                context
                    .add_instruction_location(
                        &instruction,
                        FileLocRange::new(inst_start, line_end(&lines, inst_start.line)),
                    )
                    .map_err(location_error)?;
            }
        }
    }
    Ok(())
}

fn location_error(_: crate::asm_parser_context::LocationError) -> ParseError {
    ParseError::Expected {
        expected: "unique parser source location".into(),
        loc: crate::parse_error::DiagLoc::span(llvmkit_support::Span::new(0, 0)),
    }
}

fn source_lines(src: &[u8]) -> Vec<&str> {
    from_utf8(src).unwrap_or("").lines().collect()
}

fn function_range(lines: &[&str], name: Option<&str>) -> Option<(FileLoc, FileLoc)> {
    let start_index = lines.iter().position(|line| {
        line.trim_start().starts_with("define ")
            && match name {
                Some(name) => line.contains(&format!("@{name}(")),
                None => true,
            }
    })?;
    let end_index = match lines
        .iter()
        .enumerate()
        .skip(start_index)
        .find_map(|(idx, line)| (line.trim() == "}").then_some(idx))
    {
        Some(idx) => idx,
        None => start_index,
    };
    Some((
        FileLoc::new(u32::try_from(start_index).ok()?, 0),
        line_end(lines, u32::try_from(end_index).ok()?),
    ))
}

fn label_line_in_range(lines: &[&str], start: u32, end: u32, label: &str) -> Option<FileLoc> {
    let start = usize::try_from(start).ok()?;
    let end = usize::try_from(end).ok()?;
    lines
        .iter()
        .enumerate()
        .take(end.saturating_add(1))
        .skip(start)
        .find_map(|(idx, line)| {
            if line.trim() == format!("{label}:") {
                Some(FileLoc::new(u32::try_from(idx).ok()?, 0))
            } else {
                None
            }
        })
}

fn instruction_lines_in_range(lines: &[&str], start: u32, end: u32) -> Vec<FileLoc> {
    let Some(start) = usize::try_from(start).ok() else {
        return Vec::new();
    };
    let Some(end) = usize::try_from(end).ok() else {
        return Vec::new();
    };
    lines
        .iter()
        .enumerate()
        .take(end.saturating_add(1))
        .skip(start)
        .filter_map(|(idx, line)| {
            let trimmed = line.trim_start();
            (!trimmed.is_empty()
                && !trimmed.ends_with(':')
                && trimmed != "}"
                && !trimmed.starts_with("define "))
            .then(|| {
                let col = line.len().saturating_sub(trimmed.len());
                let line_idx = u32::try_from(idx).unwrap_or(u32::MAX);
                let col = u32::try_from(col).unwrap_or(u32::MAX);
                FileLoc::new(line_idx, col)
            })
        })
        .collect()
}

fn line_end(lines: &[&str], line: u32) -> FileLoc {
    let len = match usize::try_from(line).ok().and_then(|idx| lines.get(idx)) {
        Some(line) => line.len(),
        None => 0,
    };
    let col = u32::try_from(len).unwrap_or(u32::MAX);
    FileLoc::new(line, col)
}
