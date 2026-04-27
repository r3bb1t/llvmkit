//! Escape-sequence decoding for quoted names and string constants.
//!
//! Mirrors `UnEscapeLexed` from
//! `orig_cpp/llvm-project-llvmorg-22.1.4/llvm/lib/AsmParser/LLLexer.cpp:124`.
//!
//! Rules (LangRef + LLLexer):
//! * `\\\\` → one literal backslash byte (`b'\\'`).
//! * `\\HH` (two ASCII hex digits) → byte with that value.
//! * Any other `\\…` sequence: the backslash is kept verbatim and the next
//!   byte is processed normally. This lenient behavior is required — real-world
//!   IR depends on it (e.g. `@"\01_foo"` is decoded fine because `0` and `1`
//!   are hex digits, but a pathological case must not error).
//!
//! Returns a borrowed slice when no decoding actually changes the bytes (the
//! input contains no `\\` at all), otherwise allocates a `Vec<u8>`.

use std::borrow::Cow;

#[inline]
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Decode `input`, returning a `Cow` that borrows when no `\` was present.
pub(super) fn unescape(input: &[u8]) -> Cow<'_, [u8]> {
    // Fast path: nothing to decode.
    let Some(first_bs) = input.iter().position(|&b| b == b'\\') else {
        return Cow::Borrowed(input);
    };

    let mut out = Vec::with_capacity(input.len());
    out.extend_from_slice(&input[..first_bs]);

    let mut i = first_bs;
    while i < input.len() {
        let b = input[i];
        if b != b'\\' {
            out.push(b);
            i += 1;
            continue;
        }
        // We saw '\'.
        // 1. "\\" → single backslash.
        if i + 1 < input.len() && input[i + 1] == b'\\' {
            out.push(b'\\');
            i += 2;
            continue;
        }
        // 2. "\HH" with two hex digits → that byte.
        if i + 2 < input.len() {
            if let (Some(hi), Some(lo)) = (hex_digit(input[i + 1]), hex_digit(input[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        // 3. Lenient: keep the literal '\' and advance one byte.
        out.push(b'\\');
        i += 1;
    }

    Cow::Owned(out)
}

/// Upstream provenance: mirrors `UnEscapeLexed` in
/// `lib/AsmParser/LLLexer.cpp`. Inputs trace assembler fixture shapes from
/// `test/Assembler/*.ll` (e.g. `unnamed_addr.ll` for `\01` mangling).
#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors the no-op fast path in
    /// `lib/AsmParser/LLLexer.cpp::UnEscapeLexed` (no `\` byte present).
    #[test]
    fn no_escapes_borrows() {
        let s = b"plain text 123";
        let out = unescape(s);
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(&*out, &b"plain text 123"[..]);
    }

    /// Mirrors the `\\` collapse case in
    /// `lib/AsmParser/LLLexer.cpp::UnEscapeLexed`.
    #[test]
    fn double_backslash_collapses() {
        let out = unescape(br"a\\b");
        assert_eq!(&*out, b"a\\b");
        assert!(matches!(out, Cow::Owned(_)));
    }

    /// Mirrors the `\xx` two-hex-digit decode in
    /// `lib/AsmParser/LLLexer.cpp::UnEscapeLexed`.
    #[test]
    fn hex_escape_decodes() {
        // \41 == 'A', \22 == '"'
        let out = unescape(br"x\41\22");
        assert_eq!(&*out, b"xA\"");
    }

    /// Mirrors `\00` decoding in
    /// `lib/AsmParser/LLLexer.cpp::UnEscapeLexed` (NUL is a legal payload byte).
    #[test]
    fn nul_byte_escape() {
        let out = unescape(br"\00");
        assert_eq!(&*out, &[0u8][..]);
    }

    /// Mirrors the `\01` mangling-suppression marker handled by
    /// `lib/AsmParser/LLLexer.cpp::UnEscapeLexed`; assembler shape lives in
    /// `test/Assembler/unnamed_addr.ll`.
    #[test]
    fn mangling_prefix_decodes() {
        // \01 is the front-end mangling-suppression marker; must round-trip
        // to a literal 0x01 byte for symbols like @"\01_foo".
        let out = unescape(br"\01_foo");
        assert_eq!(&*out, &[1u8, b'_', b'f', b'o', b'o'][..]);
    }

    /// llvmkit-specific: lenient EOF-after-backslash. Closest upstream:
    /// `lib/AsmParser/LLLexer.cpp::UnEscapeLexed` (which would diagnose).
    #[test]
    fn lenient_keeps_bad_backslash_at_eof() {
        // A trailing '\' has nothing after it; the backslash survives.
        let out = unescape(br"x\");
        assert_eq!(&*out, b"x\\");
    }

    /// llvmkit-specific: lenient bad-hex-digit recovery. Closest upstream:
    /// `lib/AsmParser/LLLexer.cpp::UnEscapeLexed`.
    #[test]
    fn lenient_keeps_bad_hex() {
        // \xZ — only one valid hex digit; the second is 'Z'. The backslash is
        // kept and processing resumes after it. So output is "\xZ".
        let out = unescape(br"\xZ");
        assert_eq!(&*out, b"\\xZ");
    }

    /// llvmkit-specific: lenient single-hex-digit at EOF. Closest upstream:
    /// `lib/AsmParser/LLLexer.cpp::UnEscapeLexed`.
    #[test]
    fn lenient_keeps_one_hex_at_eof() {
        // \4 with nothing after — the backslash and '4' both survive.
        let out = unescape(br"\4");
        assert_eq!(&*out, b"\\4");
    }

    /// llvmkit-specific: empty-input borrow path. Closest upstream:
    /// `lib/AsmParser/LLLexer.cpp::UnEscapeLexed` (no-op when input empty).
    #[test]
    fn empty_input_borrows() {
        let out = unescape(b"");
        assert!(matches!(out, Cow::Borrowed(_)));
        assert!(out.is_empty());
    }

    /// Mirrors mixed escape + literal input through
    /// `lib/AsmParser/LLLexer.cpp::UnEscapeLexed`.
    #[test]
    fn mixed_escapes_and_text() {
        let out = unescape(br"hello\20world\\!");
        // \20 is space; \\ is single backslash.
        assert_eq!(&*out, b"hello world\\!");
    }
}
