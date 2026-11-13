//! Anti-drift guard: every place the crate reads a constant's lanes one at a
//! time is audited, so no new walk can cost a long splat's lane count.
//!
//! A splat, a zero, `undef` and `poison` store no lanes
//! (`docs/divergences.md` D40): `<4294967295 x i32> splat (i32 1)` is a few
//! bytes of text and one constant. A loop that reads every lane of such a
//! constant through `Constant::aggregate_element`, or through
//! `constant_fold.rs::extract_lane`, or a repeat-fill that writes one entry
//! per lane (`vec![x; n]`, `(0..n).map(|_| x)`) in the files that build
//! constants, costs 2^32 steps or allocations. The
//! walks found so far were each bounded by hand, so this test pins the sites:
//! each function that reads a lane, how many times, and why the reads it
//! makes are bounded, or, for the one that is not (`MASK_STORAGE`), that it
//! is not. A new read, or a second one in an audited function, fails here
//! until it is audited and listed.
//!
//! No upstream counterpart: upstream stores every lane of a fixed vector, so
//! a walk over them costs what building them already did.

use std::path::{Path, PathBuf};

/// Reads one lane, at an index it is given.
const ONE_LANE: &str = "reads one lane";
/// Walks `Constant::lanes_to_ask` lanes: one for a vector stored in one slot.
const ASKS: &str = "walks Constant::lanes_to_ask lanes";
/// Walks only behind `may_walk_lanes` / `lane_walk`, in the
/// function or in its one caller: an operand stores the lanes, or there are
/// at most `MAX_EXPANDED_LANES`.
const CAPPED: &str = "walks only behind the MAX_EXPANDED_LANES cap";
/// Walks a stored element list only (a uniform constant answers first).
const STORED: &str = "walks a stored element list only";
/// Reads the elements a load of known size covers, or a struct's fields.
const SIZED: &str = "reads the elements a sized access covers";
/// Reads one source lane per stored mask lane, or one for a uniform mask.
const MASK: &str = "reads one lane per stored mask lane";
/// Reads one lane of a uniform mask, then writes one decoded entry per lane:
/// the `shufflevector` instruction stores its mask that way, as upstream's
/// `ShuffleMask` does. Not bounded: a long uniform mask costs its lane count
/// here. Recorded, unresolved, in `docs/future-work.md` ("A `shufflevector`
/// mask stores one entry per lane").
const MASK_STORAGE: &str = "reads one lane, but writes one mask entry per lane (unbounded)";

/// `(file under crates/llvmkit-ir/src, enclosing function, read count, why)`.
const AUDITED: &[(&str, &str, usize, &str)] = &[
    ("constant_fold.rs", "aggregate_elements", 1, CAPPED),
    (
        "constant_fold.rs",
        "constant_fold_extract_element_instruction_trusting_same_module",
        1,
        ONE_LANE,
    ),
    (
        "constant_fold.rs",
        "constant_fold_extract_value_instruction",
        1,
        ONE_LANE,
    ),
    (
        "constant_fold.rs",
        "constant_fold_select_instruction_trusting_same_module",
        3,
        CAPPED,
    ),
    (
        "constant_fold.rs",
        "constant_matches_negative_zero_fp_pattern",
        1,
        CAPPED,
    ),
    ("constant_fold.rs", "extract_lane", 1, ONE_LANE),
    (
        "constant_fold.rs",
        "fixed_vector_elements_for_rebuild",
        2,
        CAPPED,
    ),
    ("constant_fold.rs", "fold_shuffle_vector", 2, MASK),
    (
        "constant_fold.rs",
        "shufflevector_mask_from_constant",
        4,
        MASK_STORAGE,
    ),
    (
        "constant_fold.rs",
        "aggregate_elements_for_rebuild",
        1,
        CAPPED,
    ),
    ("constant_folding.rs", "aggregate_first_element", 3, SIZED),
    ("constant_folding.rs", "constant_at_offset", 1, SIZED),
    (
        "constant_folding.rs",
        "fold_const_vector_to_ap_int",
        2,
        CAPPED,
    ),
    ("constant_folding.rs", "merge_vector_lanes", 1, CAPPED),
    (
        "constant_folding.rs",
        "read_sequence_data_from_global",
        1,
        SIZED,
    ),
    (
        "constant_folding.rs",
        "read_struct_data_from_global",
        1,
        SIZED,
    ),
    ("constant_folding.rs", "split_vector_lanes", 1, CAPPED),
    ("pointer_analysis.rs", "find_inserted_value", 1, ONE_LANE),
    (
        "value_tracking.rs",
        "constant_int_predicate_matches",
        1,
        ASKS,
    ),
    ("value_tracking.rs", "contains_constant_expression", 1, ASKS),
    ("value_tracking.rs", "contains_undefined_element", 1, ASKS),
    (
        "value_tracking.rs",
        "expression_shuffle_mask_has_poison_lane",
        1,
        ONE_LANE,
    ),
    ("value_tracking.rs", "shift_amount_known_in_range", 1, ASKS),
    ("vector_utils.rs", "constant_mask_lanes_satisfy", 1, ASKS),
    ("vector_utils.rs", "find_scalar_element", 2, ONE_LANE),
    (
        "vector_utils.rs",
        "possibly_demanded_elements_in_mask",
        1,
        STORED,
    ),
];

/// The two lane readers' call spellings.
const READS: [&str; 2] = [".aggregate_element(", "extract_lane("];

/// The files that build constants or read their lanes, where a repeat-fill
/// (`vec![x; n]`, `(0..n).map(|_| x)`) can stand for a walk of lanes that
/// were never stored. Elsewhere such a fill sizes words or table rows.
const FILL_FILES: [&str; 7] = [
    "constant.rs",
    "constants.rs",
    "constant_fold.rs",
    "constant_folding.rs",
    "value_tracking.rs",
    "vector_utils.rs",
    "pointer_analysis.rs",
];

/// How many repeat-fills `code` holds: `vec![x; n]` or `.map(|_| x)`.
fn fills(code: &str) -> usize {
    let repeats = code
        .match_indices("vec![")
        .filter(|(at, _)| {
            code[*at..]
                .split_once(']')
                .is_some_and(|(inside, _)| inside.contains(';'))
        })
        .count();
    repeats + code.matches(".map(|_|").count()
}

/// Every `.rs` file under `dir`, recursively, sorted.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

/// The name of the function whose body holds line `index`: the nearest
/// earlier line that declares one, comments skipped.
fn enclosing_function(lines: &[&str], index: usize) -> Option<String> {
    lines[..=index].iter().rev().find_map(|line| {
        let code = line.trim_start();
        if code.starts_with("//") {
            return None;
        }
        let after = code.split_once("fn ")?.1;
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        (!name.is_empty()).then_some(name)
    })
}

/// How many lanes `code` reads: calls of either reader, not counting the
/// definition of `extract_lane` or a longer name ending in it.
fn reads_a_lane(code: &str) -> usize {
    READS
        .iter()
        .map(|read| {
            code.match_indices(read)
                .filter(|(at, _)| {
                    *read != "extract_lane("
                        || (!code[..*at].ends_with("fn ")
                            && !code[..*at]
                                .chars()
                                .last()
                                .is_some_and(|c| c.is_alphanumeric() || c == '_'))
                })
                .count()
        })
        .sum()
}

/// `(file, function) -> read count` for every lane read under `source_root`.
fn read_sites(source_root: &Path) -> Vec<(String, String, usize)> {
    let mut sites: Vec<(String, String, usize)> = Vec::new();
    for path in rust_files(source_root) {
        let source = std::fs::read_to_string(&path).expect("source file is readable");
        let lines: Vec<&str> = source.lines().collect();
        let file = path
            .strip_prefix(source_root)
            .expect("under the source root")
            .to_string_lossy()
            .replace('\\', "/");
        for (index, line) in lines.iter().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            let reads = reads_a_lane(code)
                + if FILL_FILES.contains(&file.as_str()) {
                    fills(code)
                } else {
                    0
                };
            if reads == 0 {
                continue;
            }
            let function = enclosing_function(&lines, index)
                .unwrap_or_else(|| panic!("{file}: a lane read outside any function"));
            match sites.iter_mut().find(|(site_file, site_function, _)| {
                *site_file == file && *site_function == function
            }) {
                Some((_, _, count)) => *count += reads,
                None => sites.push((file.clone(), function, reads)),
            }
        }
    }
    sites
}

/// Every lane read sits in an audited function, as many times as audited, and
/// every audited function still holds its reads.
///
/// llvmkit-specific (D40): the guard that keeps a long splat's lanes unwalked.
#[test]
fn every_lane_read_is_audited() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found = read_sites(&source_root);
    let mut problems = Vec::new();
    for (file, function, count) in &found {
        match AUDITED
            .iter()
            .find(|(audited_file, audited_function, _, _)| {
                audited_file == file && audited_function == function
            }) {
            None => problems.push(format!("unaudited: {file}::{function} ({count} read(s))")),
            Some((_, _, audited_count, _)) if audited_count != count => problems.push(format!(
                "{file}::{function}: {count} read(s), audited {audited_count}"
            )),
            Some(_) => {}
        }
    }
    for (file, function, _, _) in AUDITED {
        if !found
            .iter()
            .any(|(found_file, found_function, _)| found_file == file && found_function == function)
        {
            problems.push(format!("stale audit entry: {file}::{function}"));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The scanner counts a read against its enclosing function, skips comments
/// and `extract_lane`'s own definition, and does not take a longer name for
/// `extract_lane`.
///
/// llvmkit-specific: the scanner's own contract.
#[test]
fn the_scanner_counts_reads_by_function() {
    assert_eq!(reads_a_lane("let x = c.aggregate_element(0);"), 1);
    assert_eq!(reads_a_lane("fn extract_lane<'ctx>("), 0);
    assert_eq!(reads_a_lane("let x = my_extract_lane(v, 0);"), 0);
    assert_eq!(fills("return Some(vec![Lane(0); lane_count]);"), 1);
    assert_eq!(fills("let v = vec![a, b];"), 0);
    assert_eq!(fills("(0..count).map(|_| fill).collect()"), 1);
    assert_eq!(
        reads_a_lane("(extract_lane(a, 0)?, extract_lane(b, 1)?)"),
        2
    );
    let lines = [
        "fn walker() {",
        "    // c.aggregate_element(0)",
        "    c.aggregate_element(1);",
        "}",
    ];
    assert_eq!(enclosing_function(&lines, 2).as_deref(), Some("walker"));
}
