//! Mirrors the closing comment of `TEST(VectorTypesTest, CrossComparisons)`:
//! "We can't use ordering comparisons (<,<=,>,>=) between scalable and
//! non-scalable vector sizes." So `TypeSize` has no `PartialOrd`, and `<` between
//! two of them does not compile (`E0369`). A caller reads `known_min_value` or
//! `fixed_value` and compares those, which says which question it is asking.

use llvmkit_ir::TypeSize;

fn main() {
    let _ = TypeSize::fixed(64) < TypeSize::scalable(64);
}
