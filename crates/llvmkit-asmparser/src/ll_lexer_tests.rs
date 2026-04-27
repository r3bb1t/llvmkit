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

/// Upstream provenance: punctuation tokens emitted by
/// `LLLexer::LexToken` in `lib/AsmParser/LLLexer.cpp`.
mod structural {
    use super::*;

    /// Mirrors the punctuation cases in
    /// `lib/AsmParser/LLLexer.cpp::LexToken` (each char dispatches to
    /// its own token kind).
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

    /// Mirrors the `...` (DotDotDot) case in
    /// `lib/AsmParser/LLLexer.cpp::LexToken`.
    #[test]
    fn dotdotdot() {
        assert_eq!(kinds("..."), vec![Token::DotDotDot]);
    }

    /// Mirrors single-byte token span tracking via `LLLexer::TokStart` /
    /// `CurPtr` in `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn span_of_single_char_punct() {
        let lex = collect_ok("=,*");
        assert_eq!(lex[0].span, Span::new(0, 1));
        assert_eq!(lex[1].span, Span::new(1, 2));
        assert_eq!(lex[2].span, Span::new(2, 3));
    }
}

/// Upstream provenance: identifier sigils (`@`, `%`, `$`, `!`, `^`, `#`)
/// dispatched by `LLLexer::LexToken` to per-sigil helpers
/// (`LexAt`, `LexPercent`, `LexDollar`, `LexExclaim`, `LexCaret`, `LexHash`)
/// in `lib/AsmParser/LLLexer.cpp`.
mod idents {
    use super::*;

    /// Mirrors `LLLexer::LexAt` unquoted-name path in
    /// `lib/AsmParser/LLLexer.cpp` (borrows the lexeme; no escapes).
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

    /// Mirrors `LLLexer::LexAt` quoted-name path with no escapes in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn global_quoted_no_escape_borrows() {
        let toks = kinds(r#"@"plain name""#);
        match &toks[0] {
            Token::GlobalVar(Cow::Borrowed(b)) => assert_eq!(*b, b"plain name"),
            other => panic!("got {other:?}"),
        }
    }

    /// Mirrors `LLLexer::LexAt` quoted-name path through
    /// `UnEscapeLexed` in `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn global_quoted_with_escape_owns() {
        let toks = kinds(r#"@"a\41b""#);
        match &toks[0] {
            Token::GlobalVar(Cow::Owned(v)) => assert_eq!(v, b"aAb"),
            other => panic!("got {other:?}"),
        }
    }

    /// Mirrors `LLLexer::LexAt` numeric-id path in
    /// `lib/AsmParser/LLLexer.cpp` (`@<unsigned>`).
    #[test]
    fn global_id() {
        assert_eq!(kinds("@42"), vec![Token::GlobalId(42)]);
    }

    /// Mirrors the `\01` mangling-prefix decode through
    /// `LLLexer::LexAt` + `UnEscapeLexed` in `lib/AsmParser/LLLexer.cpp`;
    /// assembler shape `test/Assembler/unnamed_addr.ll`.
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

    /// Mirrors `LLLexer::LexPercent` unquoted/quoted dispatch in
    /// `lib/AsmParser/LLLexer.cpp`.
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

    /// Mirrors `LLLexer::LexDollar` (comdat sigil) in
    /// `lib/AsmParser/LLLexer.cpp`.
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

    /// Mirrors `LLLexer::LexExclaim` metadata-name dispatch in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn metadata_var_and_alone() {
        let toks = kinds("!foo !");
        match &toks[0] {
            Token::MetadataVar(c) => assert_eq!(c.as_ref(), b"foo"),
            _ => panic!(),
        }
        assert_eq!(toks[1], Token::Exclaim);
    }

    /// Mirrors `LLLexer::LexExclaim` -> `UnEscapeLexed` for metadata
    /// names in `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn metadata_var_decodes_escape() {
        let toks = kinds(r"!a\41b");
        match &toks[0] {
            Token::MetadataVar(Cow::Owned(v)) => assert_eq!(v, b"aAb"),
            other => panic!("got {other:?}"),
        }
    }

    /// Mirrors `LLLexer::LexCaret` summary-id (`^N`) in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn summary_id() {
        assert_eq!(
            kinds("^0 ^123"),
            vec![Token::SummaryId(0), Token::SummaryId(123)]
        );
    }

    /// Mirrors `LLLexer::LexHash` attribute-group sigil in
    /// `lib/AsmParser/LLLexer.cpp` (`#N` and bare `#`).
    #[test]
    fn attr_grp_and_lone_hash() {
        assert_eq!(
            kinds("#0 #1234 #"),
            vec![Token::AttrGrpId(0), Token::AttrGrpId(1234), Token::Hash,]
        );
    }

    /// Mirrors `LLLexer::LexAt` rejection of NUL inside a quoted name in
    /// `lib/AsmParser/LLLexer.cpp` (NUL is allowed in payload but not in
    /// names).
    #[test]
    fn nul_in_quoted_name_is_error() {
        let err = first_err(r#"@"a\00b""#);
        assert!(matches!(err, LexError::NulInName { .. }));
    }
}

/// Upstream provenance: label disambiguation in
/// `lib/AsmParser/LLLexer.cpp::LexIdentifier` /
/// `LexQuote` / colon-lookahead path.
mod labels {
    use super::*;

    /// Mirrors identifier+colon promotion to label in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier`.
    #[test]
    fn ident_label() {
        let toks = kinds("bb1: ");
        match &toks[0] {
            Token::LabelStr(c) => assert_eq!(c.as_ref(), b"bb1"),
            _ => panic!(),
        }
    }

    /// Mirrors quoted-string+colon promotion to label in
    /// `lib/AsmParser/LLLexer.cpp::LexQuote`.
    #[test]
    fn quoted_label() {
        let toks = kinds(r#""quoted label":"#);
        match &toks[0] {
            Token::LabelStr(c) => assert_eq!(c.as_ref(), b"quoted label"),
            _ => panic!(),
        }
    }

    /// Mirrors numeric-literal+colon promotion to label in
    /// `lib/AsmParser/LLLexer.cpp::LexDigitOrNegative`.
    #[test]
    fn numeric_label() {
        let toks = kinds("42:");
        match &toks[0] {
            Token::LabelStr(c) => assert_eq!(c.as_ref(), b"42"),
            _ => panic!(),
        }
    }

    /// Mirrors `-N:` negative numeric label in
    /// `lib/AsmParser/LLLexer.cpp::LexDigitOrNegative`.
    #[test]
    fn negative_label() {
        let toks = kinds("-1:");
        match &toks[0] {
            Token::LabelStr(c) => assert_eq!(c.as_ref(), b"-1"),
            _ => panic!(),
        }
    }

    /// llvmkit-specific: optional colon-suppression flag for inline-asm
    /// contexts. Closest upstream:
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier` (which always promotes).
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

/// Upstream provenance: type-keyword + integer-type lexing in
/// `lib/AsmParser/LLLexer.cpp::LexIdentifier` (`i<N>` arithmetic-type
/// branch).
mod types {
    use super::*;

    fn nz(n: u32) -> NonZeroU32 {
        NonZeroU32::new(n).unwrap()
    }

    /// Mirrors `TYPEKEYWORD` cases for primitive types in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier`.
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

    /// Mirrors `i<N>` arithmetic-type branch in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier`.
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

    /// Mirrors the `MaxIntBits` upper bound (`(1<<24)-1`) in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier`.
    #[test]
    fn integer_type_at_max() {
        let toks = kinds("i16777215");
        assert_eq!(
            toks,
            vec![Token::PrimitiveType(PrimitiveTy::Integer(nz(16777215)))]
        );
    }

    /// Mirrors the `i<N>` overflow diagnostic in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier`.
    #[test]
    fn integer_type_overflow_errors() {
        let err = first_err("i16777216");
        assert!(matches!(err, LexError::IntegerWidthOutOfRange { .. }));
    }

    /// Mirrors the bare-`i` fallthrough in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier` (no digits -> not a
    /// type, not a keyword).
    #[test]
    fn i_alone_is_unknown() {
        // Bare `i` with no digits has no integer-type interpretation and is
        // not a keyword → error. (Matches LLLexer.cpp:1073 fallthrough.)
        let err = first_err("i ");
        assert!(matches!(err, LexError::UnknownToken { .. }));
    }
}

/// Upstream provenance: numeric literals (decimal, hex APSInt, hex FP
/// constants) emitted by `LLLexer::LexDigitOrNegative` /
/// `LexAPSInt` / `LexHexFP*` in `lib/AsmParser/LLLexer.cpp`.
mod numbers {
    use super::*;

    fn int(sign: Sign, base: NumBase, digits: &str) -> Token<'_> {
        Token::IntegerLit(IntLit { sign, base, digits })
    }

    /// Mirrors `LLLexer::LexDigitOrNegative` decimal path in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn decimal_int() {
        assert_eq!(kinds("42"), vec![int(Sign::Pos, NumBase::Dec, "42")]);
        assert_eq!(kinds("-1"), vec![int(Sign::Neg, NumBase::Dec, "1")]);
        assert_eq!(kinds("0"), vec![int(Sign::Pos, NumBase::Dec, "0")]);
    }

    /// Mirrors `s0x.../u0x...` APSInt prefixes handled by
    /// `LLLexer::LexDigitOrNegative` in `lib/AsmParser/LLLexer.cpp`.
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

    /// Mirrors `0x` 64-bit hex double in
    /// `lib/AsmParser/LLLexer.cpp::LexDigitOrNegative` (no precision tag).
    #[test]
    fn hex_double() {
        assert_eq!(
            kinds("0x12ab"),
            vec![Token::FloatLit(FpLit::HexDouble("12ab"))]
        );
    }

    /// Mirrors `0xK` (x87_fp80), `0xL` (fp128), `0xM` (ppc_fp128)
    /// hex-FP tagged literals in
    /// `lib/AsmParser/LLLexer.cpp::LexDigitOrNegative`.
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

    /// Mirrors `0xH` (half) / `0xR` (bfloat) tagged literals in
    /// `lib/AsmParser/LLLexer.cpp::LexDigitOrNegative`.
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

    /// Mirrors `0xH` overflow diagnostic in
    /// `lib/AsmParser/LLLexer.cpp::LexDigitOrNegative`.
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

    /// Mirrors `0xR` overflow diagnostic in
    /// `lib/AsmParser/LLLexer.cpp::LexDigitOrNegative`.
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

    /// Mirrors decimal floating-point literal lexeme in
    /// `lib/AsmParser/LLLexer.cpp::LexDigitOrNegative`.
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

    /// Mirrors the `+`-with-no-digit error path in
    /// `lib/AsmParser/LLLexer.cpp::LexPositive` (rejects bare sign).
    #[test]
    fn plus_without_digit_errors() {
        let err = first_err("+");
        assert!(matches!(err, LexError::UnknownToken { .. }));
    }
}

/// Upstream provenance: textual keyword categories from
/// `lib/AsmParser/LLLexer.cpp::LexIdentifier` (KEYWORD/INSTKEYWORD/
/// attribute KEYWORD macros).
mod keywords_cat {
    use super::*;

    /// Mirrors `KEYWORD(define)` / `KEYWORD(declare)` / `KEYWORD(global)` /
    /// `KEYWORD(constant)` cases in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier`.
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

    /// Mirrors `INSTKEYWORD` cases in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier`.
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

    /// Mirrors flag/attribute KEYWORD cases (`nuw`, `nsw`, `inbounds`,
    /// `fastcc`, `volatile`, `noinline`, `nounwind`, `splat`, `vscale`) in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier`.
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

    /// Mirrors `attributes` keyword + `#N` attribute group reference in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier` and `LexHash`.
    #[test]
    fn attributes_keyword() {
        assert_eq!(
            kinds("attributes #0"),
            vec![Token::Kw(Keyword::Attributes), Token::AttrGrpId(0),]
        );
    }

    /// Mirrors the `cc<digits>` rewind logic in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier` (`cc` is a keyword,
    /// trailing digits are a separate integer literal).
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

/// Upstream provenance: DWARF/debug-info enum tokens emitted by
/// `lib/AsmParser/LLLexer.cpp::LexIdentifier` (DW_TAG_*, DW_OP_*,
/// DIFlag*, CSK_*, DSPFlag*, debug-info name-table keywords).
mod dwarf {
    use super::*;

    /// Mirrors `DW_TAG_<name>` recognition in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier`.
    #[test]
    fn dwarf_tag() {
        let toks = kinds("DW_TAG_subprogram");
        assert_eq!(toks, vec![Token::DwarfTag("DW_TAG_subprogram")]);
    }

    /// Mirrors `DW_OP_<name>` recognition in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier`.
    #[test]
    fn dwarf_op() {
        assert_eq!(kinds("DW_OP_plus"), vec![Token::DwarfOp("DW_OP_plus")]);
    }

    /// Mirrors `DIFlag<name>` / `DISPFlag<name>` recognition in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier`.
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

    /// Mirrors `CSK_<name>` (debug-info checksum kind) recognition in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier`.
    #[test]
    fn checksum_kind() {
        assert_eq!(kinds("CSK_MD5"), vec![Token::ChecksumKind("CSK_MD5")]);
    }

    /// Mirrors `dbg_<kind>` debug-record token recognition in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier`.
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

    /// Mirrors emission-kind keywords (`NoDebug`, `FullDebug`, etc.) in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier`.
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

    /// Mirrors name-table / fixed-point keyword categories in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier`.
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

/// Upstream provenance: string-constant lexing in
/// `lib/AsmParser/LLLexer.cpp::LexQuote` /
/// `LexIdentifier` (with `c"..."` payload).
mod strings {
    use super::*;

    /// Mirrors `LLLexer::LexQuote` borrow path (no escapes) in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn string_constant_borrows() {
        let toks = kinds(r#""hello world""#);
        match &toks[0] {
            Token::StringConstant(Cow::Borrowed(b)) => assert_eq!(*b, b"hello world"),
            other => panic!("got {other:?}"),
        }
    }

    /// Mirrors `LLLexer::LexQuote` -> `UnEscapeLexed` owned path in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn string_constant_owns_with_escape() {
        let toks = kinds(r#""a\41b""#);
        match &toks[0] {
            Token::StringConstant(Cow::Owned(v)) => assert_eq!(v, b"aAb"),
            other => panic!("got {other:?}"),
        }
    }

    /// Mirrors `LLLexer::LexQuote` allowing `\00` payload in
    /// `lib/AsmParser/LLLexer.cpp` (string constants tolerate NUL,
    /// names do not).
    #[test]
    fn nul_in_string_is_allowed() {
        // c"...\00..." — NUL is allowed in StringConstant payload, banned in names.
        let toks = kinds(r#""\00""#);
        match &toks[0] {
            Token::StringConstant(c) => assert_eq!(c.as_ref(), &[0u8][..]),
            _ => panic!(),
        }
    }

    /// Mirrors `LLLexer::LexQuote` literal-newline tolerance in
    /// `lib/AsmParser/LLLexer.cpp` (LangRef allows raw \n inside `"..."`).
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

    /// Mirrors `LLLexer::LexQuote` unterminated-string diagnostic in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn unterminated_string_errors() {
        let err = first_err(r#""never closes"#);
        assert!(matches!(err, LexError::UnterminatedString { .. }));
    }

    /// Mirrors `LLLexer::LexQuote` -> label promotion (`"...":`) in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn quote_followed_by_colon_is_label() {
        let toks = kinds(r#""block":"#);
        match &toks[0] {
            Token::LabelStr(c) => assert_eq!(c.as_ref(), b"block"),
            _ => panic!(),
        }
    }
}

/// Upstream provenance: comment skipping in
/// `lib/AsmParser/LLLexer.cpp::SkipLineComment` /
/// `SkipBlockComment` (`;` and `/* */`).
mod comments {
    use super::*;

    /// Mirrors `LLLexer::SkipLineComment` (`;` to EOL) in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn line_comment_consumed() {
        let toks = kinds("; line comment\n%a");
        match &toks[0] {
            Token::LocalVar(c) => assert_eq!(c.as_ref(), b"a"),
            _ => panic!(),
        }
        assert_eq!(toks.len(), 1);
    }

    /// Mirrors `LLLexer::SkipBlockComment` (`/* ... */`) in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn block_comment_consumed() {
        let toks = kinds("/* block */ %a");
        match &toks[0] {
            Token::LocalVar(c) => assert_eq!(c.as_ref(), b"a"),
            _ => panic!(),
        }
        assert_eq!(toks.len(), 1);
    }

    /// Mirrors `LLLexer::SkipBlockComment` unterminated diagnostic in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn unterminated_block_comment_errors() {
        let err = first_err("/* never closes");
        assert!(matches!(err, LexError::UnterminatedBlockComment { .. }));
    }

    /// Mirrors `LLLexer::LexToken` `/`-without-`*` rejection in
    /// `lib/AsmParser/LLLexer.cpp` (lone `/` is not a token).
    #[test]
    fn slash_without_star_errors() {
        let err = first_err("/x");
        assert!(matches!(err, LexError::StraySlash { .. }));
    }
}

/// Upstream provenance: error paths in `lib/AsmParser/LLLexer.cpp`
/// (`LexToken` unknown char, `LexAt`/`LexPercent` overflow, span
/// reporting).
mod errors {
    use super::*;

    /// Mirrors `LLLexer::LexToken` unknown-character diagnostic in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn unknown_token_for_question_mark() {
        let err = first_err("?");
        assert!(matches!(err, LexError::UnknownToken { .. }));
    }

    /// Mirrors `LLLexer::LexAt` numeric-id overflow diagnostic in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn id_overflow_errors() {
        // 2^33 is bigger than u32::MAX.
        let err = first_err("@8589934592");
        assert!(matches!(err, LexError::IdOverflow { .. }));
    }

    /// llvmkit-specific: structured `Span` carried by every `LexError`.
    /// Closest upstream: `LLLexer::Error(SMLoc, ...)` reporting in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn lex_error_carries_span() {
        // Two valid tokens followed by an unknown one; span must point at the
        // bad byte, not the start of input.
        let err = first_err("42 ?");
        assert_eq!(err.span(), Span::new(3, 4));
    }
}

/// Upstream provenance: end-to-end exercise of `LLLexer::UnEscapeLexed`
/// invocation through `LexAt`/`LexQuote` in `lib/AsmParser/LLLexer.cpp`.
mod escape_round_trip {
    use super::*;

    /// Mirrors `LLLexer::LexAt` borrow path through `UnEscapeLexed` in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn no_escape_borrows() {
        let toks = kinds(r#"@plain"#);
        match &toks[0] {
            Token::GlobalVar(Cow::Borrowed(_)) => {}
            other => panic!("expected borrowed; got {other:?}"),
        }
    }

    /// Mirrors `LLLexer::LexAt` owned path through `UnEscapeLexed` in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn escape_owns() {
        let toks = kinds(r#"@"escaped\41""#);
        match &toks[0] {
            Token::GlobalVar(Cow::Owned(v)) => assert_eq!(v, b"escapedA"),
            other => panic!("expected owned; got {other:?}"),
        }
    }
}

/// Upstream provenance: whitespace handling in
/// `lib/AsmParser/LLLexer.cpp::LexToken` (NUL skip, CRLF, EOF).
mod whitespace {
    use super::*;

    /// Mirrors `LLLexer.cpp::LexToken` mid-buffer NUL skip
    /// (LLLexer.cpp:182 case).
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

    /// Mirrors `LLLexer.cpp::LexToken` CRLF treatment as whitespace
    /// in `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn crlf_handled() {
        let toks = kinds("a:\r\nb:");
        assert_eq!(toks.len(), 2);
        for t in &toks {
            assert!(matches!(t, Token::LabelStr(_)));
        }
    }

    /// Mirrors `LLLexer::LexToken` EOF return in
    /// `lib/AsmParser/LLLexer.cpp` (empty buffer -> `Eof`).
    #[test]
    fn empty_input_is_eof() {
        let mut lex = Lexer::from("");
        let t = lex.next_token().unwrap().value;
        assert_eq!(t, Token::Eof);
    }
}

/// Upstream provenance: span fidelity tracked by
/// `LLLexer::TokStart` / `CurPtr` in `lib/AsmParser/LLLexer.cpp`
/// (every token records `[TokStart, CurPtr)`).
mod span_fidelity {
    use super::*;

    /// Mirrors `LLLexer::TokStart`/`CurPtr` span exclusion of
    /// trailing whitespace in `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn integer_lit_span_excludes_following_whitespace() {
        let toks = collect_ok("42  ");
        assert_eq!(toks[0].span, Span::new(0, 2));
    }

    /// Mirrors `LLLexer::TokStart`/`CurPtr` span tracking for keywords
    /// in `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn keyword_span_matches_keyword() {
        let toks = collect_ok("define i32");
        assert_eq!(toks[0].span, Span::new(0, 6));
    }

    /// Mirrors `LLLexer::LexAt` span including sigil and quotes in
    /// `lib/AsmParser/LLLexer.cpp`.
    #[test]
    fn quoted_global_span_includes_sigil_and_quotes() {
        let toks = collect_ok(r#"@"x""#);
        // `@` at 0, `"` at 1, `x` at 2, `"` at 3 → span [0, 4).
        assert_eq!(toks[0].span, Span::new(0, 4));
    }

    /// Mirrors label-token span inclusion of trailing `:` in
    /// `lib/AsmParser/LLLexer.cpp::LexIdentifier`.
    #[test]
    fn label_span_includes_colon() {
        let toks = collect_ok("foo:");
        assert_eq!(toks[0].span, Span::new(0, 4));
    }
}
