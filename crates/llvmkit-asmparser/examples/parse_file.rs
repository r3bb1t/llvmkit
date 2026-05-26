//! Parse an LLVM IR `.ll` file from disk and print the round-tripped module.
//!
//! Companion to [`lex_file`](./lex_file.rs); the same single-I/O-round-trip
//! pattern (read into `Vec<u8>`, hand the slice to the parser) but exercising
//! the full module-level grammar instead of stopping at the token stream.
//!
//! Usage:
//!     cargo run -p llvmkit-asmparser --example parse_file -- path/to/file.ll
//!
//! On success: prints the parsed [`llvmkit_ir::Module`] via its `Display`
//! impl (the AsmWriter), giving a textual round-trip of the input. On a
//! lex/parse error, prints a one-line `(line:col)` diagnostic against a
//! [`SourceMap`] over the original bytes and exits with status 1.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_asmparser::read_to_owned;
use llvmkit_ir::Module;
use llvmkit_support::{SourceMap, Span};

fn main() -> ExitCode {
    let Some(path) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: parse_file <path/to/file.ll>");
        return ExitCode::from(2);
    };

    // Single I/O round-trip — the parser borrows from this slice for its
    // entire run.
    let bytes = match File::open(&path).and_then(read_to_owned) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", path.display());
            return ExitCode::from(1);
        }
    };

    // The module's `'ctx` brand keeps parser state from leaking past this
    // borrow (mirrors upstream `LLVMContext` ownership).
    let module_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("input");
    let module = Module::new(module_name);

    let parser = match Parser::new(&bytes, &module) {
        Ok(p) => p,
        Err(err) => {
            report_error(&path, &bytes, &err);
            return ExitCode::from(1);
        }
    };

    match parser.parse_module() {
        Ok(_parsed) => {
            // `_parsed.slot_mapping` carries the numbered-global table for
            // follow-on `parse_constant_value` / `parse_type` calls; this
            // example just round-trips the module.
            print!("{module}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            report_error(&path, &bytes, &err);
            ExitCode::from(1)
        }
    }
}

fn report_error(path: &Path, src: &[u8], err: &ParseError) {
    let sm = SourceMap::new(src);
    let span = err.loc().map(|l| l.span).unwrap_or(Span::new(0, 0));
    let (line, col) = sm.line_col(span.start);
    eprintln!("{path}:{line}:{col}: {err}", path = path.display());
    if let Some(line_bytes) = sm.line_text(line) {
        eprintln!("  | {}", String::from_utf8_lossy(line_bytes));
        let underline_len = (span.end.saturating_sub(span.start) as usize).max(1);
        eprintln!(
            "  | {pad}{caret}",
            pad = " ".repeat((col - 1) as usize),
            caret = "^".repeat(underline_len)
        );
    }
}
