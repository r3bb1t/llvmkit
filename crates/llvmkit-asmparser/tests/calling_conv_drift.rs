//! Drift lock: the calling-convention tables against the vendored
//! `CallingConv.h`, and against each other.
//!
//! Same shape as `attribute_td_drift.rs` and `dwarf_def_drift.rs`, and
//! vendored for the same reason — `orig_cpp/` is gitignored, so a test that
//! reads it passes locally and fails in CI.
//!
//! There are two halves, and the second is the one with teeth. The first
//! checks llvmkit's id space against upstream's enum, so an LLVM bump that
//! adds or renumbers a convention fails here. The second checks the
//! **parser** table against the **printer** table: every convention llvmkit
//! can print must be one llvmkit can read back. That is precisely the
//! invariant that was broken — the printer knew all 60-odd mnemonics while
//! the parser matched 31, so two dozen conventions printed to text that would
//! not re-parse.

use llvmkit_asmparser::parse_dynamic;
use llvmkit_ir::CallingConv;

const CALLING_CONV_H: &str = include_str!("../tablegen/llvm-22.1.4/include/llvm/IR/CallingConv.h");

/// Every `Name = <number>` assignment inside `enum ID`.
///
/// The file's only other `=` lines are the `MaxID` sentinel and preprocessor
/// noise outside the enum, so a plain scan is enough; `MaxID` is filtered by
/// name because it bounds the space rather than naming a convention.
fn assigned_ids(source: &str) -> Vec<(String, u32)> {
    source
        .lines()
        .filter_map(|line| {
            let line = line.trim().trim_end_matches(',');
            let (name, value) = line.split_once('=')?;
            let name = name.trim();
            if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return None;
            }
            let value: u32 = value.trim().parse().ok()?;
            Some((name.to_string(), value))
        })
        .filter(|(name, _)| name != "MaxID")
        .collect()
}

/// The vendored header is readable and holds the whole enum.
#[test]
fn vendored_calling_conv_header_is_parseable() {
    let ids = assigned_ids(CALLING_CONV_H);
    assert!(
        ids.len() > 60,
        "expected to recognise most of CallingConv.h's ID assignments, got {}",
        ids.len()
    );
    let by_name = |n: &str| ids.iter().find(|(name, _)| name == n).map(|(_, v)| *v);
    assert_eq!(by_name("C"), Some(0));
    assert_eq!(by_name("Fast"), Some(8));
    assert_eq!(by_name("CHERIoT_LibraryCall"), Some(127));
    // Out of numeric order in the header, which is why the tables are checked
    // by value rather than by position.
    assert_eq!(
        by_name("AArch64_SME_ABI_Support_Routines_PreserveMost_From_X1"),
        Some(111)
    );
}

/// Exactly which ids have no mnemonic.
///
/// `printCallingConv` (`lib/IR/AsmWriter.cpp`) names 54 of the conventions and
/// falls back to a bare number for the rest, and `parseOptionalCallingConv`
/// has no keyword for those same ones — they are reachable only through the
/// API or bitcode, never written in `.ll` except as `cc <N>`. Pinning the
/// list means an LLVM release that *gives* one a keyword fails here instead of
/// silently printing it as a number.
const UNNAMED_UPSTREAM_IDS: &[(&str, u32)] = &[
    ("HiPE", 11),
    ("AVR_BUILTIN", 86),
    ("MSP430_BUILTIN", 94),
    ("WASM_EmscriptenInvoke", 99),
    ("M68k_INTR", 101),
    ("ARM64EC_Thunk_X64", 108),
    ("ARM64EC_Thunk_Native", 109),
];

#[test]
fn the_ids_without_a_mnemonic_are_exactly_upstreams() {
    let unnamed: Vec<(String, u32)> = assigned_ids(CALLING_CONV_H)
        .into_iter()
        .filter(|(_, value)| {
            let conv = CallingConv::from_raw(*value);
            conv.name().is_none() && conv.riscv_vls_vlen().is_none()
        })
        .collect();
    let expected: Vec<(String, u32)> = UNNAMED_UPSTREAM_IDS
        .iter()
        .map(|(name, value)| ((*name).to_string(), *value))
        .collect();
    assert_eq!(
        unnamed, expected,
        "the set of calling conventions with no mnemonic has drifted from \
         `printCallingConv`'s named cases"
    );
}

/// **Every convention llvmkit prints, llvmkit parses.**
///
/// Walks the whole id space, printing each convention that has a mnemonic and
/// feeding it back through the parser on a function header. A convention the
/// printer emits but the parser does not match is a broken round-trip: the
/// module prints to text that is not valid input to the same crate.
///
/// This is the check that was missing. `parse_optional_calling_conv` matched
/// 31 keywords against a printer that knew 60, and every one of the other 29
/// — `spir_kernel`, `ptx_kernel`, `graalcc`, the AArch64 SME set, the CHERIoT
/// set — was silently read as `ccc` and printed back wrong.
#[test]
fn every_printable_calling_convention_round_trips_through_the_parser() {
    let mut broken = Vec::new();
    for raw in 0..=CallingConv::MAX {
        let conv = CallingConv::from_raw(raw);
        // `ccc` is the default and `printFunction` omits it entirely, so there
        // is no text to look for.
        if conv == CallingConv::C {
            continue;
        }
        // `cc <N>` is the numeric fallback and is covered separately; here we
        // want the conventions with a real spelling.
        if conv.name().is_none() && conv.riscv_vls_vlen().is_none() {
            continue;
        }
        let spelled = conv.to_string();
        let source = format!("declare {spelled} void @f()\n");
        match parse_dynamic(source.as_str()) {
            Ok(module) => {
                let printed = format!("{module}");
                if !printed.contains(spelled.as_str()) {
                    broken.push(format!(
                        "{spelled} (raw {raw}): printed back as {printed:?}"
                    ));
                }
            }
            Err(e) => broken.push(format!("{spelled} (raw {raw}): {e}")),
        }
    }
    assert!(
        broken.is_empty(),
        "these calling conventions do not survive print -> parse:\n  {}",
        broken.join("\n  ")
    );
}

/// `riscv_vls_cc(<ABI_VLEN>)`: the twelve legal widths, and the diagnostic
/// for anything else. Ports `parseOptionalCallingConv`'s `CC_VLS_CASE` block.
#[test]
fn riscv_vls_calling_convention_takes_an_abi_vlen() {
    for vlen in [
        32u32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536,
    ] {
        let source = format!("declare riscv_vls_cc({vlen}) void @f()\n");
        let module =
            parse_dynamic(source.as_str()).unwrap_or_else(|e| panic!("riscv_vls_cc({vlen}): {e}"));
        assert!(
            format!("{module}").contains(&format!("riscv_vls_cc({vlen})")),
            "riscv_vls_cc({vlen}) did not print back:\n{module}"
        );
    }

    let err = parse_dynamic("declare riscv_vls_cc(48) void @f()\n")
        .expect_err("48 is not a legal ABI VLEN")
        .to_string();
    assert_eq!(err, "unknown RISC-V ABI VLEN");
}

/// **A reproduced upstream bug.** The `kw_riscv_vls_cc` arm consumes its own
/// keyword and then, finding no `(`, `break`s to the switch tail — which
/// consumes a second token. So a bare `riscv_vls_cc` eats whatever follows,
/// and `declare riscv_vls_cc void @f()` loses its return type.
///
/// Unreachable from printed IR (`printCallingConv` always writes the
/// parameterised form), reproduced because the contract is upstream's
/// behaviour. Recorded in `docs/future-work.md`; if upstream fixes it, this
/// test is what says so.
#[test]
fn a_bare_riscv_vls_cc_swallows_the_next_token() {
    assert!(
        parse_dynamic("declare riscv_vls_cc void @f()\n").is_err(),
        "upstream consumes `void` here, leaving the declaration malformed"
    );
    // With the return type doubled, the extra token upstream eats is supplied,
    // and the declaration parses — at the default ABI_VLEN of 128.
    let module = parse_dynamic("declare riscv_vls_cc void void @f()\n")
        .expect("the swallowed token is the first `void`");
    assert!(
        format!("{module}").contains("riscv_vls_cc(128)"),
        "a bare keyword means the default ABI_VLEN:\n{module}"
    );
}

/// The numeric escape hatch, in both directions. `parseOptionalCallingConv`'s
/// `kw_cc` arm is a bare `parseUInt32(CC)`, so any `u32` is legal — `MaxID`
/// bounds the bitcode encoding, not the grammar.
#[test]
fn the_numeric_calling_convention_form_round_trips() {
    // 12 is unassigned (it was `WebKit_JS`, removed), so it prints numerically.
    let module = parse_dynamic("declare cc 12 void @f()\n").expect("cc 12 parses");
    assert!(format!("{module}").contains("cc 12"), "{module}");

    let module = parse_dynamic("declare cc 5000 void @f()\n").expect("cc 5000 parses");
    assert!(format!("{module}").contains("cc 5000"), "{module}");
}
