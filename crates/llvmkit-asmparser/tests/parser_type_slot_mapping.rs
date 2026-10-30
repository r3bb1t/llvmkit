//! Ports `TEST(AsmParserTest, TypeWithSlotMappingParsing)` and
//! `TEST(AsmParserTest, TypeAtBeginningWithSlotMappingParsing)`
//! (`unittests/AsmParser/AsmParserTest.cpp`), whole and in upstream's order.
//!
//! `parseAssemblyString(Source, Error, Ctx, &Mapping)` is `Parser::parse_module`
//! and its `slot_mapping`. `cast<StructType>` and `cast<FixedVectorType>` are
//! [`struct_of`] and [`vector_of`]. `getPrimitiveSizeInBits` and
//! `getFixedValue` are `Type::primitive_size_in_bits`, whose number is the
//! fixed size for the fixed vector read here.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::{SlotMapping, parse_type_at_beginning_with_slots, parse_type_with_slots};
use llvmkit_ir::{
    AnyTypeEnum, DynBrand, ElemDyn, LenDyn, Module, StructBodyDyn, StructType, Type, TypeKind,
    TypeSize, VectorType,
};

/// Both tests' `Source`, split where upstream splits its literals.
const SOURCE: &str = concat!(
    "%st = type { i32, i32 }\n",
    "@v = common global [50 x %st] zeroinitializer, align 16\n",
    "%0 = type { i32, i32, i32, i32 }\n",
    "@g = common global [50 x %0] zeroinitializer, align 16\n",
    "define void @marker4(i64 %d) {\n",
    "entry:\n",
    "  %conv = trunc i64 %d to i32\n",
    "  store i32 %conv, ptr getelementptr inbounds ",
    "    ([50 x %st], ptr @v, i64 0, i64 0, i32 0), align 16\n",
    "  store i32 %conv, ptr getelementptr inbounds ",
    "    ([50 x %0], ptr @g, i64 0, i64 0, i32 0), align 16\n",
    "  ret void\n",
    "}",
);

/// `parseAssemblyString(Source, Error, Ctx, &Mapping)`.
fn parse_source(m: &Module<DynBrand>) -> SlotMapping<'_, DynBrand> {
    Parser::new(SOURCE.as_bytes(), m)
        .expect("lexer primes")
        .parse_module()
        .expect("`Source` parses")
        .slot_mapping
}

/// `cast<StructType>(Ty)`.
fn struct_of(ty: Type<'_, DynBrand>) -> StructType<'_, StructBodyDyn, DynBrand> {
    match AnyTypeEnum::from(ty) {
        AnyTypeEnum::Struct(st) => st,
        _ => panic!("`{ty}` is not a struct"),
    }
}

/// `cast<FixedVectorType>(Ty)`.
fn vector_of(ty: Type<'_, DynBrand>) -> VectorType<'_, ElemDyn, LenDyn, DynBrand> {
    assert_eq!(ty.kind(), TypeKind::FixedVector, "`{ty}`");
    match AnyTypeEnum::from(ty) {
        AnyTypeEnum::Vector(vt) => vt,
        _ => panic!("`{ty}` is not a vector"),
    }
}

/// Every element of `st` is an `i32`, as both tests' loops check.
fn assert_all_elements_are_i32(st: StructType<'_, StructBodyDyn, DynBrand>) {
    for i in 0..st.field_count() {
        let ty = st.field_type(i).expect("`i` is below `field_count`");
        assert!(ty.is_integer());
        assert_eq!(ty.primitive_size_in_bits(), TypeSize::fixed(32));
    }
}

/// Port of `TEST(AsmParserTest, TypeWithSlotMappingParsing)`.
#[test]
fn upstream_type_with_slot_mapping_parsing() {
    let m = Module::dynamic("AsmParserTest");
    let mapping = parse_source(&m);

    // Check we properly parse integer types.
    let ty = parse_type_with_slots(b"i32", &m, &mapping).expect("`i32` parses");
    assert!(ty.is_integer());
    assert_eq!(ty.primitive_size_in_bits(), TypeSize::fixed(32));

    // Check we properly parse integer types with exotic size.
    let ty = parse_type_with_slots(b"i13", &m, &mapping).expect("`i13` parses");
    assert!(ty.is_integer());
    assert_eq!(ty.primitive_size_in_bits(), TypeSize::fixed(13));

    // Check we properly parse floating point types.
    let ty = parse_type_with_slots(b"float", &m, &mapping).expect("`float` parses");
    assert_eq!(ty.kind(), TypeKind::Float);

    let ty = parse_type_with_slots(b"double", &m, &mapping).expect("`double` parses");
    assert_eq!(ty.kind(), TypeKind::Double);

    // Check we properly parse struct types.
    // Named struct.
    let ty = parse_type_with_slots(b"%st", &m, &mapping).expect("`%st` parses");
    assert_eq!(ty.kind(), TypeKind::Struct);

    // Check the details of the struct.
    let st = struct_of(ty);
    assert_eq!(st.field_count(), 2);
    assert_all_elements_are_i32(st);

    // Anonymous struct.
    let ty = parse_type_with_slots(b"%0", &m, &mapping).expect("`%0` parses");
    assert_eq!(ty.kind(), TypeKind::Struct);

    // Check the details of the struct.
    let st = struct_of(ty);
    assert_eq!(st.field_count(), 4);
    assert_all_elements_are_i32(st);

    // Check we properly parse vector types.
    let ty = parse_type_with_slots(b"<5 x i32>", &m, &mapping).expect("`<5 x i32>` parses");
    assert!(ty.is_vector());

    // Check the details of the vector.
    let vt = vector_of(ty);
    assert_eq!(vt.min_len().get(), 5);
    assert_eq!(vt.as_type().primitive_size_in_bits(), TypeSize::fixed(160));
    let ty = vt.element();
    assert!(ty.is_integer());
    assert_eq!(ty.primitive_size_in_bits(), TypeSize::fixed(32));

    // Opaque struct.
    let ty = parse_type_with_slots(b"%opaque", &m, &mapping).expect("`%opaque` parses");
    assert_eq!(ty.kind(), TypeKind::Struct);

    let st = struct_of(ty);
    assert!(st.is_opaque());

    // Check we properly parse pointer types.
    let ty = parse_type_with_slots(b"ptr", &m, &mapping).expect("`ptr` parses");
    assert!(ty.is_pointer());

    // Check that we reject types with garbage.
    parse_type_with_slots(b"i32 garbage", &m, &mapping).expect_err("trailing `garbage`");
}

/// Port of `TEST(AsmParserTest, TypeAtBeginningWithSlotMappingParsing)`.
#[test]
fn upstream_type_at_beginning_with_slot_mapping_parsing() {
    let m = Module::dynamic("AsmParserTest");
    let mapping = parse_source(&m);

    // Check we properly parse integer types.
    let (ty, read) =
        parse_type_at_beginning_with_slots(b"i32", &m, &mapping).expect("`i32` parses");
    assert!(ty.is_integer());
    assert_eq!(ty.primitive_size_in_bits(), TypeSize::fixed(32));
    assert_eq!(read, 3);

    // Check we properly parse integer types with exotic size.
    let (ty, read) =
        parse_type_at_beginning_with_slots(b"i13", &m, &mapping).expect("`i13` parses");
    assert!(ty.is_integer());
    assert_eq!(ty.primitive_size_in_bits(), TypeSize::fixed(13));
    assert_eq!(read, 3);

    // Check we properly parse floating point types.
    let (ty, read) =
        parse_type_at_beginning_with_slots(b"float", &m, &mapping).expect("`float` parses");
    assert_eq!(ty.kind(), TypeKind::Float);
    assert_eq!(read, 5);

    let (ty, read) =
        parse_type_at_beginning_with_slots(b"double", &m, &mapping).expect("`double` parses");
    assert_eq!(ty.kind(), TypeKind::Double);
    assert_eq!(read, 6);

    // Check we properly parse struct types.
    // Named struct.
    let (ty, read) =
        parse_type_at_beginning_with_slots(b"%st", &m, &mapping).expect("`%st` parses");
    assert_eq!(ty.kind(), TypeKind::Struct);
    assert_eq!(read, 3);

    // Check the details of the struct.
    let st = struct_of(ty);
    assert_eq!(st.field_count(), 2);
    assert_all_elements_are_i32(st);

    // Anonymous struct.
    let (ty, read) = parse_type_at_beginning_with_slots(b"%0", &m, &mapping).expect("`%0` parses");
    assert_eq!(ty.kind(), TypeKind::Struct);
    assert_eq!(read, 2);

    // Check the details of the struct.
    let st = struct_of(ty);
    assert_eq!(st.field_count(), 4);
    assert_all_elements_are_i32(st);

    // Check we properly parse vector types.
    let (ty, read) =
        parse_type_at_beginning_with_slots(b"<5 x i32>", &m, &mapping).expect("`<5 x i32>` parses");
    assert!(ty.is_vector());
    assert_eq!(read, 9);

    // Check the details of the vector.
    let vt = vector_of(ty);
    assert_eq!(vt.min_len().get(), 5);
    assert_eq!(vt.as_type().primitive_size_in_bits(), TypeSize::fixed(160));
    let ty = vt.element();
    assert!(ty.is_integer());
    assert_eq!(ty.primitive_size_in_bits(), TypeSize::fixed(32));

    // Opaque struct.
    let (ty, read) =
        parse_type_at_beginning_with_slots(b"%opaque", &m, &mapping).expect("`%opaque` parses");
    assert_eq!(ty.kind(), TypeKind::Struct);
    assert_eq!(read, 7);

    let st = struct_of(ty);
    assert!(st.is_opaque());

    // Check we properly parse pointer types.
    // One indirection.
    let (ty, read) =
        parse_type_at_beginning_with_slots(b"ptr", &m, &mapping).expect("`ptr` parses");
    assert!(ty.is_pointer());
    assert_eq!(read, 3);

    // Check that we reject types with garbage.
    let (ty, read) = parse_type_at_beginning_with_slots(b"i32 garbage", &m, &mapping)
        .expect("the prefix parses");
    assert!(ty.is_integer());
    assert_eq!(ty.primitive_size_in_bits(), TypeSize::fixed(32));
    // We go to the next token, i.e., we read "i32" + ' '.
    assert_eq!(read, 4);
}
