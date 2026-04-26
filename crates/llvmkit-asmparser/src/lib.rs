#![forbid(unsafe_code)]
//! `.ll` (LLVM IR) lexer and parser.
//!
//! The crate's module layout mirrors the LLVM 22.1.4 `AsmParser` source tree
//! (`include/llvm/AsmParser/*.h` + `lib/AsmParser/*.cpp`). One Rust file per
//! C++ translation unit; header-only C++ files map to a Rust file of the
//! same stem.
//!
//! | Rust module                  | LLVM source                            | Status   |
//! |------------------------------|----------------------------------------|----------|
//! | [`ll_lexer`]                 | `LLLexer.h` + `LLLexer.cpp`            | done     |
//! | [`ll_token`]                 | `LLToken.h`                            | done     |
//! | `ll_parser` (planned)        | `LLParser.h` + `LLParser.cpp`          | future   |
//! | `parser` (planned)           | `Parser.h` + `Parser.cpp`              | future   |
//! | `asm_parser_context` (plan.) | `AsmParserContext.h` + `.cpp`          | future   |
//! | `file_loc` (planned)         | `FileLoc.h`                            | future   |
//! | `slot_mapping` (planned)     | `SlotMapping.h`                        | future   |
//! | `numbered_values` (planned)  | `NumberedValues.h`                     | future   |
//!
//! The "future" entries are deliberately absent — `LLLexer` itself does not
//! depend on any of them (verified by grep), so adding empty stubs would
//! lie about the crate's capabilities. They land alongside the parser work.

pub mod ll_lexer;
pub mod ll_token;

use std::io::{self, Read};

/// Drain `r` into a fresh `Vec<u8>`. Convenience helper for the common case
/// where a caller has any `Read` source and wants to feed it to
/// [`ll_lexer::Lexer::new`].
///
/// The lexer itself takes a borrowed slice — no I/O traits in its signature
/// — so this is the recommended boundary between `Read` sources and the
/// lexer.
pub fn read_to_owned<R: Read>(mut r: R) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    r.read_to_end(&mut buf)?;
    Ok(buf)
}
