//! llvmkit-specific compile-fail (D7): `DynBrand` can name any number of live
//! modules, so it is no `UniqueBrand`, and code bounded `B: UniqueBrand`
//! refuses a `Module::dynamic` module with `UniqueBrand`'s own message. The
//! positive half is `module_brands.rs::a_module_new_brand_is_unique`.

use llvmkit_ir::{Module, UniqueBrand};

fn unique<B: UniqueBrand>(_: &Module<B>) {}

fn main() {
    let m = Module::dynamic("m");
    unique(&m);
}
