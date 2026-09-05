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
//! | [`parser`]                   | `Parser.h` + `Parser.cpp`              | seeded   |
//!
//! The substrate listed as `done` is the support layer the parser-first
//! roadmap pulls in alongside `LLParser`; it ships before the parser core
//! so the parser core can wire its resolution / location pipelines against
//! a stable interface (Roadmap section 10.5 forward-reference typestate).

pub mod asm_parser_context;
pub mod file_loc;
pub mod ll_lexer;
pub mod ll_parser;
pub mod ll_token;
pub mod numbered_values;
pub mod parse_error;
pub mod parser;
pub mod slot_mapping;

/// Every parsing entry point, at the crate root.
///
/// [`parse_dynamic`] is the ordinary way in: it returns the owned
/// [`Module`](llvmkit_ir::Module) itself. The parser takes `&[u8]` and
/// performs no I/O of its own — reading a file is the caller's job, the same
/// split upstream draws between `lib/AsmParser` and `MemoryBuffer::
/// getFileOrSTDIN` in Support. The `parse_assembly*` forms take a closure
/// instead, because they also hand back the [`ParsedModule`] slot mapping — a
/// by-product that *borrows* the module, so the two cannot both be returned
/// from one call; [`parse_assembly_with_name`] is the primitive of that
/// family for a caller supplying its own module name. The `parse_type*` /
/// `parse_constant_value*` family parses one fragment against an existing
/// module, mirroring `Parser.h`'s standalone entry points.
///
/// Every form that reads a whole module has a `_with_config` twin taking a
/// [`ParserConfig`] — upstream's `Run(UpgradeDebugInfo, DataLayoutCallback)`
/// parameters plus the `-allow-incomplete-ir` option. The plain forms run
/// under [`ParserConfig::DEFAULT`], which is what `parseAssembly` passes.
pub use parser::{
    DataLayoutCallback, ParserConfig, parse_assembly, parse_assembly_with_config,
    parse_assembly_with_context, parse_assembly_with_context_and_config, parse_assembly_with_index,
    parse_assembly_with_index_and_config, parse_assembly_with_name, parse_constant_value,
    parse_constant_value_with_slots, parse_dynamic, parse_dynamic_with_config, parse_into,
    parse_into_with_config, parse_summary_index_assembly, parse_type, parse_type_at_beginning,
    parse_type_at_beginning_with_slots, parse_type_with_slots,
};

/// The types those entry points speak: what they return, what they take, and
/// how they fail. [`Lexer`](ll_lexer::Lexer), [`Token`](ll_token::Token) and
/// the parser state machine stay module-scoped — they mirror LLVM's
/// `LLLexer` / `LLParser` plumbing rather than the surface a caller drives.
pub use ll_parser::ParsedModule;
#[doc(inline)]
pub use llvmkit_ir::module_summary_index::ModuleSummaryIndex;
pub use parse_error::{ParseError, ParseResult, SymbolId, SymbolKind};
pub use slot_mapping::{GlobalRef, SlotMapping};
