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
//! | [`file_loc`]                 | `FileLoc.h`                            | done     |
//! | [`numbered_values`]          | `NumberedValues.h`                     | done     |
//! | [`slot_mapping`]             | `SlotMapping.h`                        | done     |
//! | [`asm_parser_context`]       | `AsmParserContext.h` + `.cpp`          | done     |
//! | [`parse_error`]              | `LLParser.{h,cpp}` diagnostic surface  | seeded   |
//! | [`ll_parser`]                | `LLParser.h` + `LLParser.cpp` (subset) | seeded   |
//! | `parser` (planned)           | `Parser.h` + `Parser.cpp`              | future   |
//!
//! The substrate listed as `done` is the support layer the parser-first
//! roadmap pulls in alongside `LLParser`; it ships before the parser core
//! so Sessions 2-3 can wire their resolution / location pipelines against
//! a stable interface (Roadmap section 10.5 forward-reference typestate).

pub mod asm_parser_context;
pub mod file_loc;
pub mod ll_lexer;
pub mod ll_parser;
pub mod ll_token;
pub mod numbered_values;
pub mod parse_error;
pub mod slot_mapping;

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
