//! IIT type-signature encoding and the generated-table output.
//!
//! Ports `llvm/utils/TableGen/Basic/IntrinsicEmitter.cpp`.

use crate::*;
use std::fmt::Write as _;

pub(crate) fn compute_type_signature(
    ret_types: &[Value],
    param_types: &[Value],
) -> GenResult<Vec<u8>> {
    let all = ret_types
        .iter()
        .chain(param_types.iter())
        .map(type_from_value)
        .collect::<GenResult<Vec<_>>>()?;
    let mut overload_slots = Vec::new();
    let mut ac_idxs = Vec::with_capacity(all.len());
    for ty in &all {
        if ty.is_any() || matches!(ty, IntrType::VectorOfAnyPointersToElt { .. }) {
            ac_idxs.push(Some(overload_slots.len()));
            overload_slots.push(ty.clone());
        } else {
            ac_idxs.push(None);
        }
    }
    let any_positions = overload_slots
        .iter()
        .enumerate()
        .filter(|(_, ty)| ty.is_any())
        .map(|(idx, _)| idx)
        .collect::<Vec<_>>();
    let arg_codes = overload_slots
        .iter()
        .map(|ty| ty.arg_code())
        .collect::<GenResult<Vec<_>>>()?;

    let mut sig = Vec::new();
    match ret_types.len() {
        0 => sig.push(0),
        1 => {}
        n => {
            sig.push(21);
            sig.push(
                (n - 2)
                    .try_into()
                    .map_err(|_| GenError::new("too many return values"))?,
            );
        }
    }

    for (idx, ty) in all.iter().enumerate() {
        emit_type_sig(ty, idx, &ac_idxs, &any_positions, &arg_codes, &mut sig)?;
    }
    Ok(sig)
}

pub(crate) fn compute_sample_overloads(
    ret_types: &[Value],
    param_types: &[Value],
) -> GenResult<Option<Vec<SampleTypeOut>>> {
    let all = ret_types
        .iter()
        .chain(param_types.iter())
        .map(type_from_value)
        .collect::<GenResult<Vec<_>>>()?;
    let mut overload_slots = Vec::new();
    for ty in &all {
        if ty.is_any() || matches!(ty, IntrType::VectorOfAnyPointersToElt { .. }) {
            overload_slots.push(ty.clone());
        }
    }
    if overload_slots.is_empty() {
        return Ok(None);
    }

    let any_positions = overload_slots
        .iter()
        .enumerate()
        .filter(|(_, ty)| ty.is_any())
        .map(|(idx, _)| idx)
        .collect::<Vec<_>>();
    let mut constraints = vec![SampleConstraint::default(); overload_slots.len()];
    for ty in &all {
        collect_sample_constraints(ty, &any_positions, &mut constraints)?;
    }

    let mut samples = overload_slots
        .iter()
        .zip(&constraints)
        .map(|(ty, constraint)| sample_for_overload_slot(ty, *constraint))
        .collect::<GenResult<Vec<_>>>()?;

    for (idx, ty) in overload_slots.iter().enumerate() {
        if let IntrType::VectorOfAnyPointersToElt { index } = ty {
            let reference = overload_slot_for_any_index(&any_positions, *index)?;
            let lanes = sample_vector_lanes(&samples[reference])?;
            samples[idx] = SampleTypeOut::FixedVector {
                lanes,
                element: Box::new(SampleTypeOut::Pointer(0)),
            };
        }
    }

    Ok(Some(samples))
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SampleConstraint {
    pub(crate) requires_vector: bool,
    pub(crate) lane_multiple: u32,
}

pub(crate) fn collect_sample_constraints(
    ty: &IntrType,
    any_positions: &[usize],
    constraints: &mut [SampleConstraint],
) -> GenResult<()> {
    match ty {
        IntrType::SameVecWidth { element, .. } => {
            collect_sample_constraints(element, any_positions, constraints)
        }
        IntrType::OneNthElements { index, n } => {
            constrain_vector_slot(any_positions, constraints, *index, *n)
        }
        IntrType::VectorOfAnyPointersToElt { index } => {
            constrain_vector_slot(any_positions, constraints, *index, 1)
        }
        IntrType::Match { index, kind } => match kind {
            MatchKind::VecElement | MatchKind::VecOfBitcastsToInt => {
                constrain_vector_slot(any_positions, constraints, *index, 1)
            }
            MatchKind::Subdivide2 => constrain_vector_slot(any_positions, constraints, *index, 2),
            MatchKind::Subdivide4 => constrain_vector_slot(any_positions, constraints, *index, 4),
            MatchKind::Argument | MatchKind::Extend | MatchKind::Trunc => Ok(()),
        },
        IntrType::Fixed(_)
        | IntrType::Pointer(_)
        | IntrType::Any
        | IntrType::AnyInt
        | IntrType::AnyFloat
        | IntrType::AnyVector
        | IntrType::AnyPointer => Ok(()),
    }
}

pub(crate) fn constrain_vector_slot(
    any_positions: &[usize],
    constraints: &mut [SampleConstraint],
    any_index: u32,
    lane_multiple: u32,
) -> GenResult<()> {
    let slot = overload_slot_for_any_index(any_positions, any_index)?;
    let Some(constraint) = constraints.get_mut(slot) else {
        return Err(GenError::new("sample overload slot is out of range"));
    };
    constraint.requires_vector = true;
    constraint.lane_multiple = lcm_nonzero(constraint.lane_multiple.max(1), lane_multiple.max(1))?;
    Ok(())
}

pub(crate) fn overload_slot_for_any_index(
    any_positions: &[usize],
    any_index: u32,
) -> GenResult<usize> {
    let index = usize::try_from(any_index)
        .map_err(|_| GenError::new("sample overload index exceeds usize"))?;
    any_positions
        .get(index)
        .copied()
        .ok_or_else(|| GenError::new(format!("sample overload index {any_index} is out of range")))
}

pub(crate) fn sample_for_overload_slot(
    ty: &IntrType,
    constraint: SampleConstraint,
) -> GenResult<SampleTypeOut> {
    let lanes = constrained_sample_lanes(constraint.lane_multiple.max(1))?;
    match ty {
        IntrType::Any => {
            if constraint.requires_vector {
                Ok(sample_int_vector(lanes))
            } else {
                Ok(SampleTypeOut::Int(32))
            }
        }
        IntrType::AnyInt => {
            if constraint.requires_vector {
                Ok(sample_int_vector(lanes))
            } else {
                Ok(SampleTypeOut::Int(32))
            }
        }
        IntrType::AnyFloat => {
            if constraint.requires_vector {
                Ok(SampleTypeOut::FixedVector {
                    lanes,
                    element: Box::new(SampleTypeOut::Float("f32")),
                })
            } else {
                Ok(SampleTypeOut::Float("f32"))
            }
        }
        IntrType::AnyVector => Ok(sample_int_vector(lanes)),
        IntrType::AnyPointer => {
            if constraint.requires_vector {
                Ok(SampleTypeOut::FixedVector {
                    lanes,
                    element: Box::new(SampleTypeOut::Pointer(0)),
                })
            } else {
                Ok(SampleTypeOut::Pointer(0))
            }
        }
        IntrType::VectorOfAnyPointersToElt { .. } => Ok(SampleTypeOut::FixedVector {
            lanes,
            element: Box::new(SampleTypeOut::Pointer(0)),
        }),
        IntrType::Fixed(_)
        | IntrType::Pointer(_)
        | IntrType::Match { .. }
        | IntrType::SameVecWidth { .. }
        | IntrType::OneNthElements { .. } => Err(GenError::new(
            "non-overload intrinsic type cannot produce a sample overload",
        )),
    }
}

pub(crate) fn sample_int_vector(lanes: u32) -> SampleTypeOut {
    SampleTypeOut::FixedVector {
        lanes,
        element: Box::new(SampleTypeOut::Int(32)),
    }
}

pub(crate) fn sample_vector_lanes(sample: &SampleTypeOut) -> GenResult<u32> {
    match sample {
        SampleTypeOut::FixedVector { lanes, .. } => Ok(*lanes),
        _ => Err(GenError::new(
            "vector-of-pointers sample reference is not a vector",
        )),
    }
}

pub(crate) fn constrained_sample_lanes(lane_multiple: u32) -> GenResult<u32> {
    lcm_nonzero(4, lane_multiple.max(1))
}

pub(crate) fn lcm_nonzero(a: u32, b: u32) -> GenResult<u32> {
    if a == 0 || b == 0 {
        return Err(GenError::new("sample vector lane multiple must be nonzero"));
    }
    a.checked_div(gcd(a, b))
        .and_then(|quotient| quotient.checked_mul(b))
        .ok_or_else(|| GenError::new("sample vector lane multiple overflowed"))
}

pub(crate) fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

pub(crate) fn emit_type_sig(
    ty: &IntrType,
    all_index: usize,
    ac_idxs: &[Option<usize>],
    any_positions: &[usize],
    arg_codes: &[u8],
    out: &mut Vec<u8>,
) -> GenResult<()> {
    match ty {
        IntrType::Fixed(fixed) => emit_fixed_sig(fixed, out),
        IntrType::Pointer(addr) => {
            if *addr == 0 {
                out.push(14);
            } else {
                out.push(24);
                out.push(
                    (*addr)
                        .try_into()
                        .map_err(|_| GenError::new("address space exceeds u8"))?,
                );
            }
            Ok(())
        }
        IntrType::Any
        | IntrType::AnyInt
        | IntrType::AnyFloat
        | IntrType::AnyVector
        | IntrType::AnyPointer => {
            let ac =
                ac_idxs[all_index].ok_or_else(|| GenError::new("missing overload AC index"))?;
            out.push(15);
            out.push(((ac as u8) << 3) | ty.arg_code()?);
            Ok(())
        }
        IntrType::Match { index, kind } => {
            out.push(kind.iit_code());
            let mapped = any_positions.get(*index as usize).copied().ok_or_else(|| {
                GenError::new(format!("match type index {index} has no overload slot"))
            })?;
            out.push(((mapped as u8) << 3) | 7);
            Ok(())
        }
        IntrType::SameVecWidth { index, element } => {
            out.push(28);
            let mapped = any_positions.get(*index as usize).copied().ok_or_else(|| {
                GenError::new(format!("same-width index {index} has no overload slot"))
            })?;
            out.push(((mapped as u8) << 3) | arg_codes[mapped]);
            emit_type_sig(element, all_index, ac_idxs, any_positions, arg_codes, out)
        }
        IntrType::OneNthElements { index, n } => {
            out.push(27);
            let mapped = any_positions.get(*index as usize).copied().ok_or_else(|| {
                GenError::new(format!("one-nth index {index} has no overload slot"))
            })?;
            out.push(mapped as u8);
            out.push(
                (*n).try_into()
                    .map_err(|_| GenError::new("one-nth n exceeds u8"))?,
            );
            Ok(())
        }
        IntrType::VectorOfAnyPointersToElt { index } => {
            out.push(29);
            let ac =
                ac_idxs[all_index].ok_or_else(|| GenError::new("missing next-arg AC index"))?;
            let mapped = any_positions.get(*index as usize).copied().ok_or_else(|| {
                GenError::new(format!("vec-of-ptrs index {index} has no overload slot"))
            })?;
            out.push(ac as u8);
            out.push(mapped as u8);
            Ok(())
        }
    }
}

pub(crate) fn emit_fixed_sig(fixed: &FixedType, out: &mut Vec<u8>) -> GenResult<()> {
    if fixed.scalable {
        out.push(35);
    }
    if let Some(lanes) = fixed.lanes {
        out.push(vector_iit(lanes)?);
        let elem = fixed
            .element
            .as_ref()
            .ok_or_else(|| GenError::new("vector type missing element"))?;
        emit_fixed_sig(elem, out)?;
        return Ok(());
    }
    out.push(match fixed.name.as_str() {
        "isVoid" => 0,
        "vararg" => 26,
        "i1" => 1,
        "i8" => 2,
        "i16" => 3,
        "i32" => 4,
        "i64" => 5,
        "f16" => 6,
        "f32" => 7,
        "f64" => 8,
        "x86mmx" => 17,
        "token" => 18,
        "MetadataVT" => 19,
        "OtherVT" => 20,
        "i128" => 30,
        "f128" => 33,
        "bf16" => 40,
        "x86amx" => 42,
        "ppcf128" => 43,
        "externref" => 45,
        "funcref" => 46,
        "i2" => 47,
        "i4" => 48,
        "aarch64svcount" => 49,
        "exnref" => CUSTOM_IIT_WASM_EXNREF,
        other => {
            return Err(GenError::new(format!(
                "no IIT encoding for fixed type `{other}`"
            )));
        }
    });
    Ok(())
}

pub(crate) fn vector_iit(lanes: u32) -> GenResult<u8> {
    match lanes {
        1 => Ok(25),
        2 => Ok(9),
        3 => Ok(44),
        4 => Ok(10),
        6 => Ok(50),
        8 => Ok(11),
        10 => Ok(51),
        16 => Ok(12),
        32 => Ok(13),
        64 => Ok(16),
        128 => Ok(39),
        256 => Ok(41),
        512 => Ok(31),
        1024 => Ok(32),
        2048 => Ok(52),
        4096 => Ok(53),
        other => Err(GenError::new(format!(
            "no IIT vector encoding for v{other}"
        ))),
    }
}

#[derive(Debug, Clone)]
pub(crate) enum IntrType {
    Fixed(FixedType),
    Pointer(u32),
    Any,
    AnyInt,
    AnyFloat,
    AnyVector,
    AnyPointer,
    Match { index: u32, kind: MatchKind },
    SameVecWidth { index: u32, element: Box<IntrType> },
    OneNthElements { index: u32, n: u32 },
    VectorOfAnyPointersToElt { index: u32 },
}

impl IntrType {
    pub(crate) fn is_any(&self) -> bool {
        matches!(
            self,
            IntrType::Any
                | IntrType::AnyInt
                | IntrType::AnyFloat
                | IntrType::AnyVector
                | IntrType::AnyPointer
        )
    }

    pub(crate) fn arg_code(&self) -> GenResult<u8> {
        match self {
            IntrType::Any => Ok(0),
            IntrType::AnyInt => Ok(1),
            IntrType::AnyFloat => Ok(2),
            IntrType::AnyVector => Ok(3),
            IntrType::AnyPointer => Ok(4),
            IntrType::VectorOfAnyPointersToElt { .. } => Ok(4),
            other => Err(GenError::new(format!("type {other:?} has no ArgKind"))),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FixedType {
    pub(crate) name: String,
    pub(crate) lanes: Option<u32>,
    pub(crate) scalable: bool,
    pub(crate) element: Option<Box<FixedType>>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum MatchKind {
    Argument,
    Extend,
    Trunc,
    VecElement,
    Subdivide2,
    Subdivide4,
    VecOfBitcastsToInt,
}

impl MatchKind {
    pub(crate) fn iit_code(self) -> u8 {
        match self {
            MatchKind::Argument => 15,
            MatchKind::Extend => 22,
            MatchKind::Trunc => 23,
            MatchKind::VecElement => 34,
            MatchKind::Subdivide2 => 36,
            MatchKind::Subdivide4 => 37,
            MatchKind::VecOfBitcastsToInt => 38,
        }
    }
}

pub(crate) fn type_from_value(value: &Value) -> GenResult<IntrType> {
    let record_rc = as_record(value)?;
    let record = record_rc.as_ref();
    let name = record.name.as_deref();
    if name == Some("llvm_vararg_ty") {
        return Ok(IntrType::Fixed(FixedType {
            name: "vararg".to_owned(),
            lanes: None,
            scalable: false,
            element: None,
        }));
    }
    if record.classes.contains("LLVMAnyPointerType") {
        return Ok(IntrType::AnyPointer);
    }
    if record.classes.contains("LLVMAnyType") {
        let vt = record_field_record(record, "VT")?;
        return match vt.name.as_deref().unwrap_or("") {
            "Any" => Ok(IntrType::Any),
            "iAny" => Ok(IntrType::AnyInt),
            "fAny" => Ok(IntrType::AnyFloat),
            "vAny" => Ok(IntrType::AnyVector),
            "pAny" => Ok(IntrType::AnyPointer),
            other => Err(GenError::new(format!("unknown any ValueType `{other}`"))),
        };
    }
    if record.classes.contains("LLVMQualPointerType") {
        return Ok(IntrType::Pointer(
            int_field(record, "addrspace")
                .unwrap_or_else(|_| int_field(record, "Number").unwrap_or(0)) as u32,
        ));
    }
    if record.classes.contains("LLVMScalarOrSameVectorWidth") {
        let index = int_field(record, "idx").or_else(|_| int_field(record, "Number"))? as u32;
        let element = record_field_record(record, "elty")
            .or_else(|_| record_field_record(record, "eltty"))
            .or_else(|_| record_field_record(record, "T"))?;
        return Ok(IntrType::SameVecWidth {
            index,
            element: Box::new(type_from_value(&Value::Record(element))?),
        });
    }
    if record.classes.contains("LLVMOneNthElementsVectorType") {
        return Ok(IntrType::OneNthElements {
            index: int_field(record, "idx")? as u32,
            n: int_field(record, "n")? as u32,
        });
    }
    if record.classes.contains("LLVMVectorOfAnyPointersToElt") {
        return Ok(IntrType::VectorOfAnyPointersToElt {
            index: int_field(record, "num").or_else(|_| int_field(record, "Number"))? as u32,
        });
    }
    if record.classes.contains("LLVMVectorElementType") {
        return Ok(IntrType::Match {
            index: int_field(record, "num").or_else(|_| int_field(record, "Number"))? as u32,
            kind: MatchKind::VecElement,
        });
    }
    if record.classes.contains("LLVMExtendedType") {
        return Ok(IntrType::Match {
            index: int_field(record, "num").or_else(|_| int_field(record, "Number"))? as u32,
            kind: MatchKind::Extend,
        });
    }
    if record.classes.contains("LLVMTruncatedType") {
        return Ok(IntrType::Match {
            index: int_field(record, "num").or_else(|_| int_field(record, "Number"))? as u32,
            kind: MatchKind::Trunc,
        });
    }
    if record.classes.contains("LLVMSubdivide2VectorType") {
        return Ok(IntrType::Match {
            index: int_field(record, "num").or_else(|_| int_field(record, "Number"))? as u32,
            kind: MatchKind::Subdivide2,
        });
    }
    if record.classes.contains("LLVMSubdivide4VectorType") {
        return Ok(IntrType::Match {
            index: int_field(record, "num").or_else(|_| int_field(record, "Number"))? as u32,
            kind: MatchKind::Subdivide4,
        });
    }
    if record.classes.contains("LLVMVectorOfBitcastsToInt") {
        return Ok(IntrType::Match {
            index: int_field(record, "num").or_else(|_| int_field(record, "Number"))? as u32,
            kind: MatchKind::VecOfBitcastsToInt,
        });
    }
    if record.classes.contains("LLVMMatchType") {
        return Ok(IntrType::Match {
            index: int_field(record, "num").or_else(|_| int_field(record, "Number"))? as u32,
            kind: MatchKind::Argument,
        });
    }
    if record.classes.contains("LLVMType") {
        let vt = record_field_record(record, "VT")?;
        return Ok(IntrType::Fixed(fixed_type_from_value_type(&vt)?));
    }
    Err(GenError::new(format!(
        "record {:?} is not an LLVMType; classes={:?}",
        record.name, record.classes
    )))
}

pub(crate) fn fixed_type_from_value_type(record: &RecordValue) -> GenResult<FixedType> {
    let name = record
        .name
        .clone()
        .ok_or_else(|| GenError::new("anonymous ValueType"))?;
    if field_bool(record, "isVector")?.unwrap_or(false) {
        let element = record_field_record(record, "ElementType")?;
        Ok(FixedType {
            name,
            lanes: Some(int_field(record, "nElem")? as u32),
            scalable: field_bool(record, "isScalable")?.unwrap_or(false),
            element: Some(Box::new(fixed_type_from_value_type(&element)?)),
        })
    } else {
        Ok(FixedType {
            name,
            lanes: None,
            scalable: false,
            element: None,
        })
    }
}

#[derive(Debug)]
pub(crate) struct TargetSetOut {
    pub(crate) prefix: String,
    pub(crate) offset: usize,
    pub(crate) count: usize,
}

pub(crate) fn build_target_sets(intrinsics: &[IntrinsicOut]) -> Vec<TargetSetOut> {
    let mut sets = Vec::new();
    if intrinsics.is_empty() {
        return sets;
    }
    let mut current = intrinsics[0].target_prefix.clone();
    let mut offset = 0usize;
    for (idx, intrinsic) in intrinsics.iter().enumerate() {
        if intrinsic.target_prefix != current {
            sets.push(TargetSetOut {
                prefix: current,
                offset,
                count: idx - offset,
            });
            current = intrinsic.target_prefix.clone();
            offset = idx;
        }
    }
    sets.push(TargetSetOut {
        prefix: current,
        offset,
        count: intrinsics.len() - offset,
    });
    sets
}

pub(crate) fn render_generated(
    intrinsics: &[IntrinsicOut],
    target_sets: &[TargetSetOut],
) -> GenResult<String> {
    let (iit_table, long_table, iit_indices) = build_iit_tables(intrinsics)?;
    let mut out = String::new();
    out.push_str(GENERATED_HEADER);
    writeln!(
        out,
        "pub(crate) const NUM_INTRINSICS: u32 = {};",
        intrinsics.len() + 1
    )
    .unwrap();
    writeln!(
        out,
        "pub(crate) const IIT_WASM_EXNREF: u8 = {CUSTOM_IIT_WASM_EXNREF};\n"
    )
    .unwrap();

    writeln!(
        out,
        "pub(crate) static INTRINSIC_TARGET_SETS: &[IntrinsicTargetSet] = &["
    )
    .unwrap();
    for set in target_sets {
        writeln!(
            out,
            "    IntrinsicTargetSet {{ prefix: {:?}, offset: {}, count: {} }},",
            set.prefix, set.offset, set.count
        )
        .unwrap();
    }
    writeln!(out, "];\n").unwrap();

    writeln!(
        out,
        "pub(crate) static INTRINSIC_RECORDS: &[IntrinsicRecord] = &["
    )
    .unwrap();
    for (idx, intrinsic) in intrinsics.iter().enumerate() {
        writeln!(out, "    IntrinsicRecord {{").unwrap();
        writeln!(out, "        enum_name: {:?},", intrinsic.enum_name).unwrap();
        writeln!(out, "        base_name: {:?},", intrinsic.name).unwrap();
        writeln!(out, "        target_prefix: {:?},", intrinsic.target_prefix).unwrap();
        writeln!(out, "        is_overloaded: {},", intrinsic.overloaded).unwrap();
        writeln!(out, "        iit_table_index: {},", iit_indices[idx]).unwrap();
        writeln!(
            out,
            "        fn_attrs: {},",
            render_fn_attrs(intrinsic.fn_attrs)
        )
        .unwrap();
        writeln!(
            out,
            "        arg_attrs: {},",
            render_arg_attrs(&intrinsic.arg_attrs)
        )
        .unwrap();
        writeln!(
            out,
            "        memory_effects: MemoryEffects::create_from_int_value({}),",
            intrinsic.memory_effects
        )
        .unwrap();
        writeln!(
            out,
            "        clang_builtin: {},",
            render_option(&intrinsic.clang_builtin)
        )
        .unwrap();
        writeln!(
            out,
            "        ms_builtin: {},",
            render_option(&intrinsic.ms_builtin)
        )
        .unwrap();
        writeln!(
            out,
            "        pretty_print: {},",
            render_pretty_print(&intrinsic.pretty_print)
        )
        .unwrap();
        writeln!(out, "    }},").unwrap();
    }
    writeln!(out, "];\n").unwrap();

    writeln!(out, "pub(crate) static IIT_TABLE: &[u16] = &[").unwrap();
    for chunk in iit_table.chunks(8) {
        write!(out, "   ").unwrap();
        for value in chunk {
            write!(out, " 0x{value:04x},").unwrap();
        }
        writeln!(out).unwrap();
    }
    writeln!(out, "];\n").unwrap();

    writeln!(out, "pub(crate) static IIT_LONG_ENCODING_TABLE: &[u8] = &[").unwrap();
    for chunk in long_table.chunks(16) {
        write!(out, "   ").unwrap();
        for value in chunk {
            write!(out, " {value},").unwrap();
        }
        writeln!(out).unwrap();
    }
    writeln!(out, "];\n").unwrap();

    let semantic_ids = semantic_ids(intrinsics)?;
    for (name, raw) in &semantic_ids {
        writeln!(out, "pub(crate) const SEMANTIC_{name}: u32 = {raw};").unwrap();
    }
    writeln!(out).unwrap();
    writeln!(
        out,
        "pub(crate) static SAMPLE_OVERLOADS: &[IntrinsicSampleOverload] = &["
    )
    .unwrap();
    for (idx, intrinsic) in intrinsics.iter().enumerate() {
        if intrinsic.sample_overloads.is_empty() {
            continue;
        }
        writeln!(
            out,
            "    IntrinsicSampleOverload {{ raw_id: {}, overloads: {} }},",
            idx + 1,
            render_sample_types(&intrinsic.sample_overloads)
        )
        .unwrap();
    }
    writeln!(out, "];\n").unwrap();
    Ok(out)
}

pub(crate) fn build_iit_tables(
    intrinsics: &[IntrinsicOut],
) -> GenResult<(Vec<u16>, Vec<u8>, Vec<u32>)> {
    let mut fixed = Vec::new();
    let mut long_table = Vec::<u8>::new();
    let mut long_offsets = BTreeMap::<Vec<u8>, usize>::new();
    let mut indices = Vec::new();
    for intrinsic in intrinsics {
        let idx = fixed.len() as u32;
        indices.push(idx);
        if let Some(encoded) = encode_fixed(&intrinsic.type_sig) {
            fixed.push(encoded);
        } else {
            let offset = if let Some(offset) = long_offsets.get(&intrinsic.type_sig) {
                *offset
            } else {
                let offset = long_table.len();
                long_table.extend_from_slice(&intrinsic.type_sig);
                long_table.push(0);
                long_offsets.insert(intrinsic.type_sig.clone(), offset);
                offset
            };
            if offset > 0x7fff {
                return Err(GenError::new(
                    "IIT long encoding table offset exceeds 15 bits",
                ));
            }
            fixed.push(0x8000 | (offset as u16));
        }
    }
    long_table.push(255);
    Ok((fixed, long_table, indices))
}

pub(crate) fn encode_fixed(sig: &[u8]) -> Option<u16> {
    if sig.len() > 8 || sig.iter().any(|byte| *byte > 15) {
        return None;
    }
    let mut result = 0u32;
    for byte in sig.iter().rev() {
        result = (result << 4) | u32::from(*byte);
    }
    if result & 0x7fff == result {
        Some(result as u16)
    } else {
        None
    }
}

pub(crate) fn semantic_ids(intrinsics: &[IntrinsicOut]) -> GenResult<Vec<(String, usize)>> {
    let required = [
        ("ABS", "abs"),
        ("BITREVERSE", "bitreverse"),
        ("BSWAP", "bswap"),
        ("CTLZ", "ctlz"),
        ("CTTZ", "cttz"),
        ("CTPOP", "ctpop"),
        ("FSHL", "fshl"),
        ("FSHR", "fshr"),
        ("UADD_SAT", "uadd_sat"),
        ("USUB_SAT", "usub_sat"),
        ("SADD_SAT", "sadd_sat"),
        ("SSUB_SAT", "ssub_sat"),
        ("UMIN", "umin"),
        ("UMAX", "umax"),
        ("SMIN", "smin"),
        ("SMAX", "smax"),
        ("VECTOR_REDUCE_ADD", "vector_reduce_add"),
        ("PTRMASK", "ptrmask"),
        ("LIFETIME_START", "lifetime_start"),
        ("LIFETIME_END", "lifetime_end"),
        ("MEMCPY", "memcpy"),
        ("MEMMOVE", "memmove"),
        ("MEMSET", "memset"),
        ("ASSUME", "assume"),
        ("EXPECT", "expect"),
        ("TRAP", "trap"),
        ("DONOTHING", "donothing"),
        ("READCYCLECOUNTER", "readcyclecounter"),
        ("READ_REGISTER", "read_register"),
        ("WRITE_REGISTER", "write_register"),
        ("VSCALE", "vscale"),
    ];
    let mut out = Vec::new();
    for (const_name, enum_name) in required {
        let idx = intrinsics
            .iter()
            .position(|intrinsic| intrinsic.enum_name == enum_name)
            .ok_or_else(|| {
                GenError::new(format!("missing required semantic intrinsic `{enum_name}`"))
            })?;
        out.push((const_name.to_owned(), idx + 1));
    }
    Ok(out)
}

pub(crate) fn render_fn_attrs(attrs: FnAttrsOut) -> String {
    format!(
        "IntrinsicFnAttrs {{ no_unwind: {}, no_return: {}, no_callback: {}, no_sync: {}, no_free: {}, will_return: {}, cold: {}, no_duplicate: {}, no_merge: {}, commutative: {}, convergent: {}, speculatable: {}, strict_fp: {}, no_create_undef_or_poison: {}, has_side_effects: {} }}",
        attrs.no_unwind,
        attrs.no_return,
        attrs.no_callback,
        attrs.no_sync,
        attrs.no_free,
        attrs.will_return,
        attrs.cold,
        attrs.no_duplicate,
        attrs.no_merge,
        attrs.commutative,
        attrs.convergent,
        attrs.speculatable,
        attrs.strict_fp,
        attrs.no_create_undef_or_poison,
        attrs.has_side_effects,
    )
}

pub(crate) fn render_arg_attrs(attrs: &[IndexedAttrOut]) -> String {
    if attrs.is_empty() {
        return "&[]".to_owned();
    }
    let mut out = String::from("&[");
    for attr in attrs {
        write!(
            out,
            "IntrinsicIndexedAttr {{ index: {}, attr: {} }}, ",
            attr.index,
            render_arg_attr(&attr.attr)
        )
        .unwrap();
    }
    out.push(']');
    out
}

pub(crate) fn render_arg_attr(attr: &ArgAttrOut) -> String {
    match attr {
        ArgAttrOut::NoCapture => "IntrinsicArgAttr::NoCapture".to_owned(),
        ArgAttrOut::NoAlias => "IntrinsicArgAttr::NoAlias".to_owned(),
        ArgAttrOut::NoUndef => "IntrinsicArgAttr::NoUndef".to_owned(),
        ArgAttrOut::NonNull => "IntrinsicArgAttr::NonNull".to_owned(),
        ArgAttrOut::Returned => "IntrinsicArgAttr::Returned".to_owned(),
        ArgAttrOut::ReadOnly => "IntrinsicArgAttr::ReadOnly".to_owned(),
        ArgAttrOut::WriteOnly => "IntrinsicArgAttr::WriteOnly".to_owned(),
        ArgAttrOut::ReadNone => "IntrinsicArgAttr::ReadNone".to_owned(),
        ArgAttrOut::ImmArg => "IntrinsicArgAttr::ImmArg".to_owned(),
        ArgAttrOut::Alignment(value) => format!("IntrinsicArgAttr::Alignment({value})"),
        ArgAttrOut::Dereferenceable(value) => format!("IntrinsicArgAttr::Dereferenceable({value})"),
        ArgAttrOut::Range(lower, upper) => {
            format!("IntrinsicArgAttr::Range {{ lower: {lower}, upper: {upper} }}")
        }
    }
}

pub(crate) fn render_pretty_print(args: &[PrettyPrintOut]) -> String {
    if args.is_empty() {
        return "&[]".to_owned();
    }
    let mut out = String::from("&[");
    for arg in args {
        write!(
            out,
            "PrettyPrintArg {{ arg_index: {}, name: {:?}, printer: {:?} }}, ",
            arg.arg_index, arg.name, arg.printer
        )
        .unwrap();
    }
    out.push(']');
    out
}

pub(crate) fn render_option(value: &Option<String>) -> String {
    match value {
        Some(value) => format!("Some({value:?})"),
        None => "None".to_owned(),
    }
}

pub(crate) fn render_sample_types(samples: &[SampleTypeOut]) -> String {
    if samples.is_empty() {
        return "&[]".to_owned();
    }
    let mut out = String::from("&[");
    for sample in samples {
        write!(out, "{}, ", render_sample_type(sample)).unwrap();
    }
    out.push(']');
    out
}

pub(crate) fn render_sample_type(sample: &SampleTypeOut) -> String {
    match sample {
        SampleTypeOut::Int(bits) => format!("IntrinsicSampleType::Int({bits})"),
        SampleTypeOut::Float(name) => format!("IntrinsicSampleType::Float({name:?})"),
        SampleTypeOut::Pointer(addr_space) => format!("IntrinsicSampleType::Pointer({addr_space})"),
        SampleTypeOut::FixedVector { lanes, element } => format!(
            "IntrinsicSampleType::FixedVector {{ lanes: {lanes}, element: &{} }}",
            render_sample_type(element)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::test_record;

    /// Mirrors `llvm/utils/TableGen/Basic/IntrinsicEmitter.cpp::encodePacked`
    /// and the fixed/long IIT table split in `EmitGenerator`: fixed encodings
    /// pack decoder-order nibbles and reject entries that need the long table.
    #[test]
    fn fixed_iit_encoding_packs_nibbles_in_decoder_order() {
        assert_eq!(encode_fixed(&[4, 4, 4]), Some(0x444));
        assert_eq!(encode_fixed(&[21, 0, 4]), None);
    }

    /// Mirrors `llvm/include/llvm/IR/Intrinsics.td::LLVMVectorOfAnyPointersToElt`
    /// and `llvm/lib/IR/Intrinsics.cpp::matchIntrinsicType`: sample overloads
    /// for tied vector-pointer operands preserve the referenced vector shape.
    #[test]
    fn vector_of_any_pointers_to_elt_gets_sample_overload() {
        let any_vector = test_record(
            "llvm_anyvector_ty",
            &["LLVMAnyType"],
            &[("VT", test_record("vAny", &[], &[]))],
        );
        let pointer_vector = test_record(
            "llvm_anyptr_vector_ty",
            &["LLVMVectorOfAnyPointersToElt"],
            &[("Number", Value::Int(0))],
        );

        let samples = compute_sample_overloads(&[], &[any_vector, pointer_vector]).unwrap();

        assert_eq!(
            samples,
            Some(vec![
                SampleTypeOut::FixedVector {
                    lanes: 4,
                    element: Box::new(SampleTypeOut::Int(32)),
                },
                SampleTypeOut::FixedVector {
                    lanes: 4,
                    element: Box::new(SampleTypeOut::Pointer(0)),
                },
            ])
        );
    }

    /// Mirrors `llvm/include/llvm/IR/Intrinsics.td::LLVMOneNthElementsVectorType`
    /// and `llvm/lib/IR/Intrinsics.cpp::matchIntrinsicType`: sample overloads
    /// choose a source vector with a lane count divisible by the requested split.
    #[test]
    fn one_nth_vector_reference_gets_divisible_sample_overload() {
        let any_vector = test_record(
            "llvm_anyvector_ty",
            &["LLVMAnyType"],
            &[("VT", test_record("vAny", &[], &[]))],
        );
        let one_third_vector = test_record(
            "llvm_v3_ty",
            &["LLVMOneNthElementsVectorType"],
            &[("idx", Value::Int(0)), ("n", Value::Int(3))],
        );

        let samples = compute_sample_overloads(&[one_third_vector], &[any_vector]).unwrap();

        assert_eq!(
            samples,
            Some(vec![SampleTypeOut::FixedVector {
                lanes: 12,
                element: Box::new(SampleTypeOut::Int(32)),
            }])
        );
    }
}
