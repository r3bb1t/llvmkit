//! Public parser facade.
//!
//! Mirrors `llvm/include/llvm/AsmParser/Parser.h` and
//! `llvm/lib/AsmParser/Parser.cpp`: callers use these stateless helpers for
//! one-shot parsing, while [`crate::ll_parser::Parser`] keeps the recursive
//! descent state private to the parsing operation.

use std::fs::read as read_file;
use std::path::Path;
use std::str::from_utf8;

use llvmkit_ir::{Brand, Constant, Module, ModuleBrand, Type, Unverified};

use super::file_loc::{FileLoc, FileLocRange};

use super::asm_parser_context::AsmParserContext;
use super::ll_parser::{ParsedModule, Parser};
use super::module_summary::{self, ModuleSummaryIndex};
use super::parse_error::{ParseError, ParseResult};
use super::slot_mapping::SlotMapping;

/// Parse a complete textual IR module from bytes under a fresh module brand.
pub fn parse_assembly<R, S, F>(src: S, f: F) -> ParseResult<R>
where
    S: AsRef<[u8]>,
    F: for<'ctx> FnOnce(
        Module<'ctx, Brand<'ctx>, Unverified>,
        ParsedModule<'ctx, Brand<'ctx>>,
    ) -> R,
{
    parse_assembly_with_name("asm", src, f)
}

fn parse_assembly_with_name<R, S, F>(name: &str, src: S, f: F) -> ParseResult<R>
where
    S: AsRef<[u8]>,
    F: for<'ctx> FnOnce(
        Module<'ctx, Brand<'ctx>, Unverified>,
        ParsedModule<'ctx, Brand<'ctx>>,
    ) -> R,
{
    Module::with_new::<_, _, _>(name, |module| {
        let parsed = Parser::new(src.as_ref(), &module)?.parse_module()?;
        Ok(f(module, parsed))
    })
}

/// Parse a complete textual IR module from a UTF-8 string under a fresh brand.
pub fn parse_assembly_string<R, F>(src: &str, f: F) -> ParseResult<R>
where
    F: for<'ctx> FnOnce(
        Module<'ctx, Brand<'ctx>, Unverified>,
        ParsedModule<'ctx, Brand<'ctx>>,
    ) -> R,
{
    parse_assembly(src.as_bytes(), f)
}

/// Read and parse a complete textual IR module under a fresh module brand.
pub fn parse_assembly_file<R, P, F>(path: P, f: F) -> ParseResult<R>
where
    P: AsRef<Path>,
    F: for<'ctx> FnOnce(
        Module<'ctx, Brand<'ctx>, Unverified>,
        ParsedModule<'ctx, Brand<'ctx>>,
    ) -> R,
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
pub fn parse_assembly_with_context<R, S, F>(src: S, f: F) -> ParseResult<R>
where
    S: AsRef<[u8]>,
    F: for<'ctx> FnOnce(
        Module<'ctx, Brand<'ctx>, Unverified>,
        ParsedModule<'ctx, Brand<'ctx>>,
        AsmParserContext<'ctx>,
    ) -> R,
{
    Module::with_new::<_, _, _>("asm", |module| {
        let bytes = src.as_ref();
        let parsed = Parser::new(bytes, &module)?.parse_module()?;
        let mut context = AsmParserContext::new();
        record_parser_context(bytes, &module, &mut context)?;
        Ok(f(module, parsed, context))
    })
}

/// Parse a single LLVM type and require end-of-input.
pub fn parse_type<'ctx, B: ModuleBrand + 'ctx>(
    src: &[u8],
    module: &Module<'ctx, B, Unverified>,
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
    module: &Module<'ctx, B, Unverified>,
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
    module: &Module<'ctx, B, Unverified>,
    ty: Type<'ctx, B>,
    slots: Option<&SlotMapping<'ctx, B>>,
) -> ParseResult<Constant<'ctx, B>> {
    let parser = match slots {
        Some(slots) => Parser::with_slot_mapping(src, module, slots)?,
        None => Parser::new(src, module)?,
    };
    parser.parse_standalone_constant_value(ty)
}

fn record_parser_context<'ctx>(
    src: &[u8],
    module: &Module<'ctx, Brand<'ctx>, Unverified>,
    context: &mut AsmParserContext<'ctx>,
) -> ParseResult<()> {
    let lines = source_lines(src);
    for function_view in module.as_view().iter_functions() {
        let Some(function) = module.function_by_name(function_view.name()) else {
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
