//! Compile-fail lock (error-surface cleanup Task 24 fix round 2, D7): a
//! function pass's report carries its module's brand. The report holds the
//! reshape pass's `CfgUpdate<B>` log for the driver of that module; before
//! `FnReport<B>` it was brand-free, holding the log under the crate-private
//! storage brand and re-branding it for whatever function the driver ran,
//! with nothing checked. A report of one brand is now not a report of
//! another (`E0308`).

use llvmkit_ir::{FnReport, ModuleBrand};

fn relabel<Left: ModuleBrand, Right: ModuleBrand>(report: FnReport<Left>) -> FnReport<Right> {
    report
}

fn main() {}
