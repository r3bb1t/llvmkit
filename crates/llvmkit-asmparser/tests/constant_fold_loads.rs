//! Fixtures of `test/Transforms/InstSimplify/ConstProp/loads.ll`, at
//! `ConstantFoldLoadFromConst`.
//!
//! The fixtures load through `ConstantFoldLoadFromConstPtr`, which strips the
//! pointer to a global and an offset and passes the initializer on. These ask
//! the same question one step in, with the offset the `getelementptr` would
//! have given. The globals are upstream's, verbatim.
//!
//! Two kinds are left out. `test_array_of_zero_size_array`, because llvmkit
//! spells `[4294967295 x [0 x i32]] zeroinitializer` as a list of that many
//! elements. And every fixture that needs `ReadDataFromGlobal`
//! (`docs/divergences.md` D26), such as `load_i8_from_i9` and the
//! `load_i40_*_padding` group.

use llvmkit_asmparser::parser;
use llvmkit_ir::{ApInt, DynBrand, Module, Type, Unverified, constant_fold_load_from_const};

fn parsed() -> Module<DynBrand, Unverified> {
    let source = r#"
@g7 = constant {[0 x i32], [0 x i8], ptr} { [0 x i32] poison, [0 x i8] poison, ptr null }
@g8 = constant {[4294967295 x [0 x i32]], i64} { [4294967295 x [0 x i32]] poison, i64 123 }
@g_undef = constant { i128 } undef
@g_poison = constant { i128 } poison
@g11 = constant <{ [8 x i8], [8 x i8] }> <{ [8 x i8] poison, [8 x i8] zeroinitializer }>, align 4
@g_with_padding = constant { i32, [4 x i8] } { i32 0, [4 x i8] poison }
@g_all_poison = constant { i32, [4 x i8] } poison
"#;
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}"),
    }
}

/// What a load of `ty` at byte `offset` of `@global`'s initializer folds to.
fn load<'m>(
    module: &'m Module<DynBrand, Unverified>,
    global: &str,
    ty: Type<'m, DynBrand>,
    offset: u64,
) -> Option<String> {
    let initializer = module
        .globals()
        .find(|candidate| candidate.name().as_deref() == Some(global))
        .unwrap_or_else(|| panic!("fixture defines @{global}"))
        .initializer()
        .unwrap_or_else(|| panic!("@{global} has an initializer"));
    let data_layout = module.data_layout();
    constant_fold_load_from_const(
        initializer,
        ty,
        ApInt::from_words(64, &[offset]),
        &data_layout,
    )
    .expect("the fold succeeds")
    .map(|folded| folded.to_string())
}

/// Ports `test_leading_zero_size_elems`, `test_leading_zero_size_elems_big`,
/// `test_undef_aggregate`, `test_poison_aggregate`,
/// `test_trailing_zero_gep_index`, `load_padding` and `load_all_poison`.
#[test]
fn a_load_from_a_constant_answers_as_upstream_fixtures_do() {
    let module = parsed();
    let ptr = module.ptr_type(llvmkit_ir::AddressSpace::DEFAULT).as_type();
    let i32_ty = module.i32_type().as_type();
    let i64_ty = module.i64_type().as_type();

    assert_eq!(load(&module, "g7", ptr, 0).as_deref(), Some("ptr null"));
    assert_eq!(load(&module, "g8", i64_ty, 0).as_deref(), Some("i64 123"));
    assert_eq!(
        load(&module, "g_undef", ptr, 0).as_deref(),
        Some("ptr undef")
    );
    assert_eq!(
        load(&module, "g_poison", ptr, 0).as_deref(),
        Some("ptr poison")
    );
    assert_eq!(load(&module, "g11", ptr, 8).as_deref(), Some("ptr null"));
    assert_eq!(
        load(&module, "g_with_padding", i32_ty, 4).as_deref(),
        Some("i32 poison")
    );
    assert_eq!(
        load(&module, "g_all_poison", i32_ty, 4).as_deref(),
        Some("i32 poison")
    );
}
