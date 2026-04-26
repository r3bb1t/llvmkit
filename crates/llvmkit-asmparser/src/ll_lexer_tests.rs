//! Lexer unit tests, organised into one submodule per category. Each test
//! is a tight, exact assertion (token kind + span) so regressions point
//! straight at the offending case.

#![allow(clippy::needless_pass_by_value)]

use super::super::ll_token::*;
use super::*;
use llvmkit_support::{Span, Spanned};
use std::borrow::Cow;
use std::num::NonZeroU32;

/// Drive the lexer to EOF, returning every produced token (including any
/// terminating error). EOF itself is dropped.
fn collect_all<'src>(src: &'src str) -> Vec<Result<Spanned<Token<'src>>, LexError>> {
    let mut lex = Lexer::from(src);
    let mut out = Vec::new();
    loop {
        match lex.next_token() {
            Ok(spanned) if matches!(spanned.value, Token::Eof) => break,
            other => {
                let is_err = other.is_err();
                out.push(other);
                if is_err {
                    break;
                }
            }
        }
    }
    out
}

fn collect_ok<'src>(src: &'src str) -> Vec<Spanned<Token<'src>>> {
    collect_all(src)
        .into_iter()
        .map(|r| r.expect("lex error"))
        .collect()
}

fn kinds<'src>(src: &'src str) -> Vec<Token<'src>> {
    collect_ok(src).into_iter().map(|s| s.value).collect()
}

fn first_err(src: &str) -> LexError {
    let mut lex = Lexer::from(src);
    loop {
        match lex.next_token() {
            Ok(Spanned {
                value: Token::Eof, ..
            }) => panic!("expected error, got EOF"),
            Ok(_) => continue,
            Err(e) => return e,
        }
    }
}

mod structural {
    use super::*;

    #[test]
    fn every_punctuation() {
        let toks = kinds("= , * [ ] { } < > ( ) ! | : #");
        assert_eq!(
            toks,
            vec![
                Token::Equal,
                Token::Comma,
                Token::Star,
                Token::LSquare,
                Token::RSquare,
                Token::LBrace,
                Token::RBrace,
                Token::Less,
                Token::Greater,
                Token::LParen,
                Token::RParen,
                Token::Exclaim,
                Token::Bar,
                Token::Colon,
                Token::Hash,
            ]
        );
    }

    #[test]
    fn dotdotdot() {
        assert_eq!(kinds("..."), vec![Token::DotDotDot]);
    }

    #[test]
    fn span_of_single_char_punct() {
        let lex = collect_ok("=,*");
        assert_eq!(lex[0].span, Span::new(0, 1));
        assert_eq!(lex[1].span, Span::new(1, 2));
        assert_eq!(lex[2].span, Span::new(2, 3));
    }
}

mod idents {
    use super::*;

    #[test]
    fn global_unquoted_borrows() {
        let toks = collect_ok("@foo @bar.baz @_x");
        let names: Vec<&[u8]> = toks
            .iter()
            .map(|s| match &s.value {
                Token::GlobalVar(c) => c.as_ref(),
                _ => panic!(),
            })
            .collect();
        assert_eq!(names, vec![&b"foo"[..], &b"bar.baz"[..], &b"_x"[..]]);
        // None of the unquoted forms allocate.
        for t in &toks {
            if let Token::GlobalVar(c) = &t.value {
                assert!(matches!(c, Cow::Borrowed(_)));
            }
        }
    }

    #[test]
    fn global_quoted_no_escape_borrows() {
        let toks = kinds(r#"@"plain name""#);
        match &toks[0] {
            Token::GlobalVar(Cow::Borrowed(b)) => assert_eq!(*b, b"plain name"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn global_quoted_with_escape_owns() {
        let toks = kinds(r#"@"a\41b""#);
        match &toks[0] {
            Token::GlobalVar(Cow::Owned(v)) => assert_eq!(v, b"aAb"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn global_id() {
        assert_eq!(kinds("@42"), vec![Token::GlobalId(42)]);
    }

    #[test]
    fn mangling_prefix_decodes_to_01() {
        let toks = kinds(r#"@"\01_foo""#);
        match &toks[0] {
            Token::GlobalVar(Cow::Owned(v)) => {
                assert_eq!(v, &[1u8, b'_', b'f', b'o', b'o'])
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn local_unquoted_id_and_quoted() {
        let toks = kinds(r#"%foo %0 %"a b""#);
        match &toks[0] {
            Token::LocalVar(c) => assert_eq!(c.as_ref(), b"foo"),
            other => panic!("got {other:?}"),
        }
        assert_eq!(toks[1], Token::LocalVarId(0));
        match &toks[2] {
            Token::LocalVar(Cow::Borrowed(b)) => assert_eq!(*b, b"a b"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn comdat_var() {
        let toks = kinds(r#"$foo $"x""#);
        match &toks[0] {
            Token::ComdatVar(c) => assert_eq!(c.as_ref(), b"foo"),
            _ => panic!(),
        }
        match &toks[1] {
            Token::ComdatVar(c) => assert_eq!(c.as_ref(), b"x"),
            _ => panic!(),
        }
    }

    #[test]
    fn metadata_var_and_alone() {
        let toks = kinds("!foo !");
        match &toks[0] {
            Token::MetadataVar(c) => assert_eq!(c.as_ref(), b"foo"),
            _ => panic!(),
        }
        assert_eq!(toks[1], Token::Exclaim);
    }

    #[test]
    fn metadata_var_decodes_escape() {
        let toks = kinds(r"!a\41b");
        match &toks[0] {
            Token::MetadataVar(Cow::Owned(v)) => assert_eq!(v, b"aAb"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn summary_id() {
        assert_eq!(
            kinds("^0 ^123"),
            vec![Token::SummaryId(0), Token::SummaryId(123)]
        );
    }

    #[test]
    fn attr_grp_and_lone_hash() {
        assert_eq!(
            kinds("#0 #1234 #"),
            vec![Token::AttrGrpId(0), Token::AttrGrpId(1234), Token::Hash,]
        );
    }

    #[test]
    fn nul_in_quoted_name_is_error() {
        let err = first_err(r#"@"a\00b""#);
        assert!(matches!(err, LexError::NulInName { .. }));
    }
}

mod labels {
    use super::*;

    #[test]
    fn ident_label() {
        let toks = kinds("bb1: ");
        match &toks[0] {
            Token::LabelStr(c) => assert_eq!(c.as_ref(), b"bb1"),
            _ => panic!(),
        }
    }

    #[test]
    fn quoted_label() {
        let toks = kinds(r#""quoted label":"#);
        match &toks[0] {
            Token::LabelStr(c) => assert_eq!(c.as_ref(), b"quoted label"),
            _ => panic!(),
        }
    }

    #[test]
    fn numeric_label() {
        let toks = kinds("42:");
        match &toks[0] {
            Token::LabelStr(c) => assert_eq!(c.as_ref(), b"42"),
            _ => panic!(),
        }
    }

    #[test]
    fn negative_label() {
        let toks = kinds("-1:");
        match &toks[0] {
            Token::LabelStr(c) => assert_eq!(c.as_ref(), b"-1"),
            _ => panic!(),
        }
    }

    #[test]
    fn ignore_colon_in_idents_suppresses_label() {
        let mut lex = Lexer::from("ret:");
        lex.ignore_colon_in_idents = true;
        // The `:` survives as its own token instead of being absorbed into a
        // label. Mirrors `LLLexer::IgnoreColonInIdentifiers` (LLLexer.cpp:511).
        assert_eq!(
            lex.next_token().unwrap().value,
            Token::Instruction(Opcode::Ret)
        );
        assert_eq!(lex.next_token().unwrap().value, Token::Colon);
    }
}

mod types {
    use super::*;

    fn nz(n: u32) -> NonZeroU32 {
        NonZeroU32::new(n).unwrap()
    }

    #[test]
    fn primitive_types() {
        let toks = kinds(
            "void half bfloat float double x86_fp80 fp128 ppc_fp128 label metadata x86_amx token ptr",
        );
        let expected = vec![
            Token::PrimitiveType(PrimitiveTy::Void),
            Token::PrimitiveType(PrimitiveTy::Half),
            Token::PrimitiveType(PrimitiveTy::BFloat),
            Token::PrimitiveType(PrimitiveTy::Float),
            Token::PrimitiveType(PrimitiveTy::Double),
            Token::PrimitiveType(PrimitiveTy::X86Fp80),
            Token::PrimitiveType(PrimitiveTy::Fp128),
            Token::PrimitiveType(PrimitiveTy::PpcFp128),
            Token::PrimitiveType(PrimitiveTy::Label),
            Token::PrimitiveType(PrimitiveTy::Metadata),
            Token::PrimitiveType(PrimitiveTy::X86Amx),
            Token::PrimitiveType(PrimitiveTy::Token),
            Token::PrimitiveType(PrimitiveTy::Ptr),
        ];
        assert_eq!(toks, expected);
    }

    #[test]
    fn integer_types_basic() {
        let toks = kinds("i1 i32 i65535");
        assert_eq!(
            toks,
            vec![
                Token::PrimitiveType(PrimitiveTy::Integer(nz(1))),
                Token::PrimitiveType(PrimitiveTy::Integer(nz(32))),
                Token::PrimitiveType(PrimitiveTy::Integer(nz(65535))),
            ]
        );
    }

    #[test]
    fn integer_type_at_max() {
        let toks = kinds("i16777215");
        assert_eq!(
            toks,
            vec![Token::PrimitiveType(PrimitiveTy::Integer(nz(16777215)))]
        );
    }

    #[test]
    fn integer_type_overflow_errors() {
        let err = first_err("i16777216");
        assert!(matches!(err, LexError::IntegerWidthOutOfRange { .. }));
    }

    #[test]
    fn i_alone_is_unknown() {
        // Bare `i` with no digits has no integer-type interpretation and is
        // not a keyword → error. (Matches LLLexer.cpp:1073 fallthrough.)
        let err = first_err("i ");
        assert!(matches!(err, LexError::UnknownToken { .. }));
    }
}

mod numbers {
    use super::*;

    fn int(sign: Sign, base: NumBase, digits: &str) -> Token<'_> {
        Token::IntegerLit(IntLit { sign, base, digits })
    }

    #[test]
    fn decimal_int() {
        assert_eq!(kinds("42"), vec![int(Sign::Pos, NumBase::Dec, "42")]);
        assert_eq!(kinds("-1"), vec![int(Sign::Neg, NumBase::Dec, "1")]);
        assert_eq!(kinds("0"), vec![int(Sign::Pos, NumBase::Dec, "0")]);
    }

    #[test]
    fn hex_apsint_signed_unsigned() {
        assert_eq!(
            kinds("s0xff"),
            vec![int(Sign::Pos, NumBase::HexSigned, "ff")]
        );
        assert_eq!(
            kinds("u0xFF"),
            vec![int(Sign::Pos, NumBase::HexUnsigned, "FF")]
        );
    }

    #[test]
    fn hex_double() {
        assert_eq!(
            kinds("0x12ab"),
            vec![Token::FloatLit(FpLit::HexDouble("12ab"))]
        );
    }

    #[test]
    fn hex_x87_quad_ppc() {
        assert_eq!(
            kinds("0xK00 0xL00 0xM00"),
            vec![
                Token::FloatLit(FpLit::HexX87("00")),
                Token::FloatLit(FpLit::HexQuad("00")),
                Token::FloatLit(FpLit::HexPpc128("00")),
            ]
        );
    }

    #[test]
    fn hex_half_and_bfloat() {
        assert_eq!(
            kinds("0xH4000"),
            vec![Token::FloatLit(FpLit::HexHalf("4000"))]
        );
        assert_eq!(
            kinds("0xR3f80"),
            vec![Token::FloatLit(FpLit::HexBFloat("3f80"))]
        );
    }

    #[test]
    fn hex_half_overflow_errors() {
        let err = first_err("0xH10000"); // 17 bits
        assert!(matches!(
            err,
            LexError::HexFpTooLarge {
                target: HexFpKind::Half,
                ..
            }
        ));
    }

    #[test]
    fn hex_bfloat_overflow_errors() {
        let err = first_err("0xR10000");
        assert!(matches!(
            err,
            LexError::HexFpTooLarge {
                target: HexFpKind::BFloat,
                ..
            }
        ));
    }

    #[test]
    fn fp_decimal_borrows_full_lexeme() {
        assert_eq!(kinds("1.5"), vec![Token::FloatLit(FpLit::Decimal("1.5"))]);
        assert_eq!(
            kinds("1.5e+10"),
            vec![Token::FloatLit(FpLit::Decimal("1.5e+10"))]
        );
        assert_eq!(
            kinds("-3.14"),
            vec![Token::FloatLit(FpLit::Decimal("-3.14"))]
        );
        assert_eq!(kinds("+1.5"), vec![Token::FloatLit(FpLit::Decimal("+1.5"))]);
    }

    #[test]
    fn plus_without_digit_errors() {
        let err = first_err("+");
        assert!(matches!(err, LexError::UnknownToken { .. }));
    }
}

mod keywords_cat {
    use super::*;

    #[test]
    fn structural_keywords() {
        assert_eq!(
            kinds("define declare global constant"),
            vec![
                Token::Kw(Keyword::Define),
                Token::Kw(Keyword::Declare),
                Token::Kw(Keyword::Global),
                Token::Kw(Keyword::Constant),
            ]
        );
    }

    #[test]
    fn instructions() {
        assert_eq!(
            kinds("add load store call ret br switch alloca"),
            vec![
                Token::Instruction(Opcode::Add),
                Token::Instruction(Opcode::Load),
                Token::Instruction(Opcode::Store),
                Token::Instruction(Opcode::Call),
                Token::Instruction(Opcode::Ret),
                Token::Instruction(Opcode::Br),
                Token::Instruction(Opcode::Switch),
                Token::Instruction(Opcode::Alloca),
            ]
        );
    }

    #[test]
    fn flags_and_attrs() {
        assert_eq!(
            kinds("nuw nsw inbounds fastcc volatile noinline nounwind splat vscale"),
            vec![
                Token::Kw(Keyword::Nuw),
                Token::Kw(Keyword::Nsw),
                Token::Kw(Keyword::Inbounds),
                Token::Kw(Keyword::Fastcc),
                Token::Kw(Keyword::Volatile),
                Token::Kw(Keyword::Noinline),
                Token::Kw(Keyword::Nounwind),
                Token::Kw(Keyword::Splat),
                Token::Kw(Keyword::Vscale),
            ]
        );
    }

    #[test]
    fn attributes_keyword() {
        assert_eq!(
            kinds("attributes #0"),
            vec![Token::Kw(Keyword::Attributes), Token::AttrGrpId(0),]
        );
    }

    #[test]
    fn cc_with_digits_rewinds() {
        // `cc1234` → `cc` + integer `1234`.
        let toks = kinds("cc1234");
        assert_eq!(
            toks,
            vec![
                Token::Kw(Keyword::Cc),
                Token::IntegerLit(IntLit {
                    sign: Sign::Pos,
                    base: NumBase::Dec,
                    digits: "1234"
                }),
            ]
        );
    }
}

mod dwarf {
    use super::*;

    #[test]
    fn dwarf_tag() {
        let toks = kinds("DW_TAG_subprogram");
        assert_eq!(toks, vec![Token::DwarfTag("DW_TAG_subprogram")]);
    }

    #[test]
    fn dwarf_op() {
        assert_eq!(kinds("DW_OP_plus"), vec![Token::DwarfOp("DW_OP_plus")]);
    }

    #[test]
    fn diflag_and_dispflag() {
        assert_eq!(
            kinds("DIFlagPrototyped"),
            vec![Token::DiFlag("DIFlagPrototyped")]
        );
        assert_eq!(
            kinds("DISPFlagDefinition"),
            vec![Token::DiSpFlag("DISPFlagDefinition")]
        );
    }

    #[test]
    fn checksum_kind() {
        assert_eq!(kinds("CSK_MD5"), vec![Token::ChecksumKind("CSK_MD5")]);
    }

    #[test]
    fn dbg_record_type_strips_prefix() {
        let toks = kinds("dbg_value dbg_declare dbg_assign dbg_label dbg_declare_value");
        assert_eq!(
            toks,
            vec![
                Token::DbgRecordType("value"),
                Token::DbgRecordType("declare"),
                Token::DbgRecordType("assign"),
                Token::DbgRecordType("label"),
                Token::DbgRecordType("declare_value"),
            ]
        );
    }

    #[test]
    fn emission_kind() {
        assert_eq!(
            kinds("NoDebug FullDebug"),
            vec![
                Token::EmissionKind("NoDebug"),
                Token::EmissionKind("FullDebug"),
            ]
        );
    }

    #[test]
    fn name_table_and_fixed_point() {
        assert_eq!(
            kinds("GNU Apple Default"),
            vec![
                Token::NameTableKind("GNU"),
                Token::NameTableKind("Apple"),
                Token::NameTableKind("Default"),
            ]
        );
        assert_eq!(
            kinds("Binary Decimal Rational"),
            vec![
                Token::FixedPointKind("Binary"),
                Token::FixedPointKind("Decimal"),
                Token::FixedPointKind("Rational"),
            ]
        );
    }
}

mod strings {
    use super::*;

    #[test]
    fn string_constant_borrows() {
        let toks = kinds(r#""hello world""#);
        match &toks[0] {
            Token::StringConstant(Cow::Borrowed(b)) => assert_eq!(*b, b"hello world"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn string_constant_owns_with_escape() {
        let toks = kinds(r#""a\41b""#);
        match &toks[0] {
            Token::StringConstant(Cow::Owned(v)) => assert_eq!(v, b"aAb"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn nul_in_string_is_allowed() {
        // c"...\00..." — NUL is allowed in StringConstant payload, banned in names.
        let toks = kinds(r#""\00""#);
        match &toks[0] {
            Token::StringConstant(c) => assert_eq!(c.as_ref(), &[0u8][..]),
            _ => panic!(),
        }
    }

    #[test]
    fn newline_inside_string_does_not_terminate() {
        // LangRef permits literal newlines inside string constants. The byte
        // `\n` (0x0a) is preserved verbatim until the closing `"`.
        let src = "\"line1\nline2\"";
        let toks = kinds(src);
        match &toks[0] {
            Token::StringConstant(c) => assert_eq!(c.as_ref(), b"line1\nline2"),
            _ => panic!(),
        }
    }

    #[test]
    fn unterminated_string_errors() {
        let err = first_err(r#""never closes"#);
        assert!(matches!(err, LexError::UnterminatedString { .. }));
    }

    #[test]
    fn quote_followed_by_colon_is_label() {
        let toks = kinds(r#""block":"#);
        match &toks[0] {
            Token::LabelStr(c) => assert_eq!(c.as_ref(), b"block"),
            _ => panic!(),
        }
    }
}

mod comments {
    use super::*;

    #[test]
    fn line_comment_consumed() {
        let toks = kinds("; line comment\n%a");
        match &toks[0] {
            Token::LocalVar(c) => assert_eq!(c.as_ref(), b"a"),
            _ => panic!(),
        }
        assert_eq!(toks.len(), 1);
    }

    #[test]
    fn block_comment_consumed() {
        let toks = kinds("/* block */ %a");
        match &toks[0] {
            Token::LocalVar(c) => assert_eq!(c.as_ref(), b"a"),
            _ => panic!(),
        }
        assert_eq!(toks.len(), 1);
    }

    #[test]
    fn unterminated_block_comment_errors() {
        let err = first_err("/* never closes");
        assert!(matches!(err, LexError::UnterminatedBlockComment { .. }));
    }

    #[test]
    fn slash_without_star_errors() {
        let err = first_err("/x");
        assert!(matches!(err, LexError::StraySlash { .. }));
    }
}

mod errors {
    use super::*;

    #[test]
    fn unknown_token_for_question_mark() {
        let err = first_err("?");
        assert!(matches!(err, LexError::UnknownToken { .. }));
    }

    #[test]
    fn id_overflow_errors() {
        // 2^33 is bigger than u32::MAX.
        let err = first_err("@8589934592");
        assert!(matches!(err, LexError::IdOverflow { .. }));
    }

    #[test]
    fn lex_error_carries_span() {
        // Two valid tokens followed by an unknown one; span must point at the
        // bad byte, not the start of input.
        let err = first_err("42 ?");
        assert_eq!(err.span(), Span::new(3, 4));
    }
}

mod escape_round_trip {
    use super::*;

    #[test]
    fn no_escape_borrows() {
        let toks = kinds(r#"@plain"#);
        match &toks[0] {
            Token::GlobalVar(Cow::Borrowed(_)) => {}
            other => panic!("expected borrowed; got {other:?}"),
        }
    }

    #[test]
    fn escape_owns() {
        let toks = kinds(r#"@"escaped\41""#);
        match &toks[0] {
            Token::GlobalVar(Cow::Owned(v)) => assert_eq!(v, b"escapedA"),
            other => panic!("expected owned; got {other:?}"),
        }
    }
}

mod whitespace {
    use super::*;

    #[test]
    fn nul_byte_is_whitespace() {
        // Mirrors LLLexer.cpp:182. A mid-buffer NUL is silently skipped.
        let src = b"\x00@x\x00";
        let mut lex = Lexer::new(src);
        let t = lex.next_token().unwrap().value;
        match t {
            Token::GlobalVar(c) => assert_eq!(c.as_ref(), b"x"),
            other => panic!("got {other:?}"),
        }
        // Then EOF (the trailing NUL is more whitespace).
        let t = lex.next_token().unwrap().value;
        assert_eq!(t, Token::Eof);
    }

    #[test]
    fn crlf_handled() {
        let toks = kinds("a:\r\nb:");
        assert_eq!(toks.len(), 2);
        for t in &toks {
            assert!(matches!(t, Token::LabelStr(_)));
        }
    }

    #[test]
    fn empty_input_is_eof() {
        let mut lex = Lexer::from("");
        let t = lex.next_token().unwrap().value;
        assert_eq!(t, Token::Eof);
    }
}

mod span_fidelity {
    use super::*;

    #[test]
    fn integer_lit_span_excludes_following_whitespace() {
        let toks = collect_ok("42  ");
        assert_eq!(toks[0].span, Span::new(0, 2));
    }

    #[test]
    fn keyword_span_matches_keyword() {
        let toks = collect_ok("define i32");
        assert_eq!(toks[0].span, Span::new(0, 6));
    }

    #[test]
    fn quoted_global_span_includes_sigil_and_quotes() {
        let toks = collect_ok(r#"@"x""#);
        // `@` at 0, `"` at 1, `x` at 2, `"` at 3 → span [0, 4).
        assert_eq!(toks[0].span, Span::new(0, 4));
    }

    #[test]
    fn label_span_includes_colon() {
        let toks = collect_ok("foo:");
        assert_eq!(toks[0].span, Span::new(0, 4));
    }
}
