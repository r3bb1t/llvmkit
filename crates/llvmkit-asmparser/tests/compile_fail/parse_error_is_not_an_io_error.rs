//! Locks that the parser's error type cannot absorb an I/O failure.
//!
//! llvmkit-specific (**no upstream counterpart**): upstream's `SMDiagnostic`
//! deliberately spans both, because `parseAssemblyFile` opens the file itself.
//! llvmkit keeps the read in the caller — mirroring the split upstream draws
//! between `lib/AsmParser`, whose primitive takes a `MemoryBufferRef`, and
//! Support's `MemoryBuffer::getFileOrSTDIN` — so `ParseError` carries no I/O
//! variant and no `From<std::io::Error>`.
//!
//! A grep for `std::io` in the crate would go stale the moment someone added
//! the impl back. This cannot: the `?` below has no conversion to reach for.

use llvmkit_asmparser::ParseResult;

fn reads_a_file() -> ParseResult<Vec<u8>> {
    let bytes = std::fs::read("some.ll")?;
    Ok(bytes)
}

fn main() {
    let _ = reads_a_file();
}
