//! End-to-end lexer integration tests.
//!
//! Exercises every advertised input path against the same fixture and
//! confirms the public error / `Cow` semantics behave as documented.

use std::borrow::Cow;
use std::io::Cursor;

use llvmkit_asmparser::ll_lexer::{LexError, Lexer};
use llvmkit_asmparser::ll_token::{Keyword, PrimitiveTy, Token};
use llvmkit_asmparser::read_to_owned;
use llvmkit_support::Spanned;

const SRC: &str = include_str!("fixtures/demo.ll");

fn collect(mut lex: Lexer<'_>) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    loop {
        match lex.next_token().expect("lex error") {
            Spanned {
                value: Token::Eof, ..
            } => break,
            sp => out.push(sp.value),
        }
    }
    out
}

#[test]
fn three_input_paths_yield_identical_streams() {
    // Path 1: From<&str> (most ergonomic for in-memory data).
    let a = collect(Lexer::from(SRC));

    // Path 2: From<&[u8]> / Lexer::new (canonical constructor).
    let b = collect(Lexer::new(SRC.as_bytes()));

    // Path 3: from any `Read` source via the documented helper.
    let bytes = read_to_owned(Cursor::new(SRC.as_bytes())).expect("read");
    let c = collect(Lexer::new(&bytes));

    assert_eq!(a, b);
    assert_eq!(a, c);
    assert!(!a.is_empty(), "fixture should produce tokens");
}

#[test]
fn snapshot_landmark_tokens() {
    let toks = collect(Lexer::from(SRC));

    // First non-comment token in the fixture is `source_filename`.
    assert_eq!(toks.first(), Some(&Token::Kw(Keyword::SourceFilename)));

    // The fixture contains `define i32 @main()`. Find the keyword and check
    // the next two tokens.
    let define_idx = toks
        .iter()
        .position(|t| matches!(t, Token::Kw(Keyword::Define)))
        .expect("Kw(Define) somewhere in stream");
    assert!(matches!(
        toks[define_idx + 1],
        Token::PrimitiveType(PrimitiveTy::Integer(n)) if n.get() == 32
    ));
    assert!(matches!(
        toks[define_idx + 2],
        Token::GlobalVar(ref c) if c.as_ref() == b"main"
    ));

    // The fixture ends with `}` on its own line, so the last token is RBrace.
    assert_eq!(toks.last(), Some(&Token::RBrace));
}

#[test]
fn lex_error_propagates_via_question_mark() -> Result<(), LexError> {
    // Two valid tokens, then an unterminated string. `?` walks through the
    // first two and surfaces the third.
    let mut lex = Lexer::from(r#"@x = "unterminated"#);
    let _at_x = lex.next_token()?; // @x
    let _eq = lex.next_token()?; // =
    let err = lex.next_token().expect_err("third token must be an error");
    assert!(matches!(err, LexError::UnterminatedString { .. }));
    Ok(())
}

#[test]
fn cow_borrows_when_possible() {
    // First name has no escapes → borrowed. Second has `\41` → owned.
    let mut lex = Lexer::from(r#"@plain @"escaped\41""#);
    let t1 = lex.next_token().expect("ok").value;
    let t2 = lex.next_token().expect("ok").value;

    match t1 {
        Token::GlobalVar(Cow::Borrowed(b)) => assert_eq!(b, b"plain"),
        other => panic!("expected borrowed @plain; got {other:?}"),
    }
    match t2 {
        Token::GlobalVar(Cow::Owned(v)) => assert_eq!(v, b"escapedA"),
        other => panic!("expected owned escape decode; got {other:?}"),
    }
}
