//! Lex an LLVM IR `.ll` file from disk and print every token with its
//! source location. The recommended I/O boundary pattern: read once into a
//! `Vec<u8>`, hand the slice to the lexer, drive `next_token` to EOF.
//!
//! Usage:
//!     cargo run -p llvmkit-asmparser --example lex_file -- path/to/file.ll
//!
//! On a lex error, the example prints a one-line diagnostic with `(line:col)`
//! pulled from a [`SourceMap`] and exits with status 1.

use std::fs::File;
use std::path::PathBuf;
use std::process::ExitCode;

use llvmkit_asmparser::ll_lexer::{LexError, Lexer};
use llvmkit_asmparser::read_to_owned;
use llvmkit_support::SourceMap;

fn main() -> ExitCode {
    let Some(path) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: lex_file <path/to/file.ll>");
        return ExitCode::from(2);
    };

    // Single I/O round-trip — every later operation works on the borrowed slice.
    let bytes = match File::open(&path).and_then(read_to_owned) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", path.display());
            return ExitCode::from(1);
        }
    };

    let source_map = SourceMap::new(&bytes);
    let mut lexer = Lexer::new(&bytes);
    let mut count = 0usize;

    loop {
        match lexer.next_token() {
            Ok(spanned) => {
                let (l, c) = source_map.line_col(spanned.span.start);
                println!(
                    "{path}:{l}:{c}  ({s_start}..{s_end})  {tok:?}",
                    path = path.display(),
                    l = l,
                    c = c,
                    s_start = spanned.span.start,
                    s_end = spanned.span.end,
                    tok = spanned.value,
                );
                if matches!(spanned.value, llvmkit_asmparser::ll_token::Token::Eof) {
                    break;
                }
                count += 1;
            }
            Err(err) => {
                report_error(&path, &source_map, &err);
                return ExitCode::from(1);
            }
        }
    }

    eprintln!("{count} token(s) lexed");
    ExitCode::SUCCESS
}

fn report_error(path: &std::path::Path, sm: &SourceMap<'_>, err: &LexError) {
    let span = err.span();
    let (l, c) = sm.line_col(span.start);
    eprintln!("{path}:{l}:{c}: {err}", path = path.display());
    if let Some(line) = sm.line_text(l) {
        eprintln!("  | {}", String::from_utf8_lossy(line));
        let underline_len = (span.end.saturating_sub(span.start) as usize).max(1);
        eprintln!(
            "  | {pad}{caret}",
            pad = " ".repeat((c - 1) as usize),
            caret = "^".repeat(underline_len)
        );
    }
}
