//! What `IrError::VerifierFailure` says its finding was *about*.
//!
//! No upstream counterpart as a fixture family: `Verifier::CheckFailed`
//! (`llvm/lib/IR/Verifier.cpp`) threads a `Value *` into its message and
//! prints it, so upstream has no context field to assert on separately from
//! the message — a `test/Verifier/*.ll` `CHECK` line matches the rendered
//! sentence, not a structured subject. `VerifierSubject` is llvmkit's way of
//! spelling which `Value` kind that pointer held, so these tests pin an
//! llvmkit representation choice.
//!
//! They exist because nothing read the field before. `VerifierFailure` used to
//! carry `function: Option<String>` / `block: Option<String>`, and every
//! consumer in the workspace matched it with `..` — thirteen files outside
//! `verifier.rs` and `error.rs` name the variant, and
//!
//! ```text
//! git grep -n -A6 "IrError::VerifierFailure {" 7ba7d1b -- 'crates/*/tests/*' \
//!     'crates/*/src/*' ':!crates/llvmkit-ir/src/verifier.rs' \
//!   | rg "^\S+[-:][0-9]+[-:]\s*(function|block):"
//! ```
//!
//! returns nothing. (Without the `verifier.rs` exclusion it returns ten hits —
//! the five construction sites' own field initialisers, which are writers, not
//! readers.) With no reader, two of the five sites could fill `function` with a
//! global variable's name and an ifunc's, and a third could disagree with the
//! others about whether the stored name carries its `@` sigil, all without a
//! test noticing.

use llvmkit_ir::{
    IntrinsicDescriptor, IntrinsicId, IrError, Linkage, NamedMetadataName, VerifierSubject,
    module_new,
};

/// The subject of the first verifier failure `m` reports.
fn subject_of(error: &IrError) -> &VerifierSubject {
    match error {
        IrError::VerifierFailure { subject, .. } => subject,
        other => panic!("expected a VerifierFailure, got {other:?}"),
    }
}

/// A global variable's finding names the global, not a function.
///
/// This is the shape that was wrong: `Verifier::fail_global` stored the
/// global's name in a field documented as "name of the function under
/// verification".
///
/// The rule is `Verifier::visitGlobalVariable`'s "Globals cannot contain
/// scalable types" check; `globals_basic.rs::scalable_vector_global_rejected`
/// covers the rule itself, and this covers the subject.
#[test]
fn a_global_variables_finding_names_the_global() -> Result<(), IrError> {
    let m = module_new!("subject")?;
    let i32_ty = m.i32_type();
    let scalable = m.scalable_vector_type(i32_ty.as_type(), 4);
    m.global_builder("s", scalable.as_type()).build()?;

    let error = m
        .verify_borrowed()
        .expect_err("a scalable global is invalid");
    assert_eq!(
        subject_of(&error),
        &VerifierSubject::GlobalVariable {
            name: "s".to_owned()
        }
    );
    Ok(())
}

/// An ifunc's finding names the ifunc, not a function.
///
/// `Verifier::visitGlobalIFunc`'s linkage check. `Linkage::Common` is outside
/// `is_valid_ifunc_linkage`'s set, and `GlobalIfuncBuilder::build` deliberately
/// accepts it so the upstream diagnostic stays reachable — see the comment on
/// `Verifier::visit_global_ifunc`.
#[test]
fn an_ifuncs_finding_names_the_ifunc() -> Result<(), IrError> {
    let m = module_new!("subject")?;
    let i32_ty = m.i32_type();
    let zero = i32_ty.const_int(0i32);
    let resolver = m.add_global("resolver", zero)?;
    m.ifunc_builder("indirect", i32_ty.as_type(), m.view(resolver))
        .linkage(Linkage::Common)
        .build()?;

    let error = m
        .verify_borrowed()
        .expect_err("common linkage is invalid for an ifunc");
    assert_eq!(
        subject_of(&error),
        &VerifierSubject::GlobalIfunc {
            name: "indirect".to_owned()
        }
    );
    Ok(())
}

/// A whole-function finding names the function and no block.
///
/// `Verifier::visitFunction`'s intrinsic-address-taken check. Storing the
/// intrinsic's pointer in a global initializer makes the use a
/// `ValueUse::GlobalField`, which `function_address_is_taken` reports as taken
/// because it is not a `CallBase`.
#[test]
fn a_whole_function_finding_names_the_function() -> Result<(), IrError> {
    let m = module_new!("subject")?;
    let i32_ty = m.i32_type();
    let descriptor = IntrinsicDescriptor::new(IntrinsicId::ABS, [i32_ty.as_type()])?;
    let intrinsic = m.get_or_insert_intrinsic_declaration(&descriptor)?;
    m.add_global_constant("slot", m.view(intrinsic).as_global_constant_ptr())?;

    let error = m
        .verify_borrowed()
        .expect_err("an intrinsic's address may not be taken");
    let VerifierSubject::Function { name } = subject_of(&error) else {
        panic!("expected a function subject, got {error:?}");
    };
    assert!(
        name.starts_with("llvm.abs."),
        "expected the intrinsic's own name, got {name:?}"
    );
    // The bug this file exists for: the field is a *function* name, so it must
    // not be a global's — the global that takes the address is `@slot`.
    assert_ne!(name, "slot");
    Ok(())
}

/// A body finding names the function *and* the block it is in.
///
/// `MissingTerminator`, the same shape
/// `verifier_basic.rs::verify_function_with_empty_block_fails_missing_terminator`
/// covers for the rule.
#[test]
fn a_body_finding_names_the_function_and_its_block() -> Result<(), IrError> {
    let m = module_new!("subject")?;
    let void = m.void_type();
    let fn_ty = m.function_type_no_parameters(void);
    let f = m.add_function_dyn("empty", fn_ty, Linkage::External)?;
    let _entry = m.view(f).append_basic_block(&m, "entry");

    let error = m
        .verify_borrowed()
        .expect_err("an unterminated block is invalid");
    assert_eq!(
        subject_of(&error),
        &VerifierSubject::Block {
            function: "empty".to_owned(),
            block: Some("entry".to_owned()),
        }
    );
    Ok(())
}

/// A module-flag finding has no named subject.
///
/// Ports the setup of `verifier_module_flags.rs::
/// incorrect_number_of_operands_in_module_flag`, which itself ports
/// `llvm/test/Verifier/module-flags-1.ll`'s `!0 = !{i32 1}`.
#[test]
fn a_module_flag_finding_has_no_named_subject() -> Result<(), IrError> {
    let m = module_new!("subject")?;
    let one = m.metadata_constant(m.i32_type().const_int(1i32))?;
    let tuple = m.metadata_tuple([one])?;
    let flags = m.get_or_insert_named_metadata(NamedMetadataName::ModuleFlags);
    m.named_metadata_add_operand(flags, tuple)?;

    let error = m
        .verify_borrowed()
        .expect_err("a one-operand module flag is invalid");
    assert_eq!(subject_of(&error), &VerifierSubject::WholeModule);
    Ok(())
}

/// No subject stores a sigil, and every subject renders one.
///
/// The stored name used to carry `@` at three construction sites and not at
/// the fourth, so one field held two spellings of one concept. The sigil is
/// now applied by `Display` alone, which is checkable in one place.
#[test]
fn subjects_store_bare_names_and_render_sigils() {
    let subjects = [
        VerifierSubject::WholeModule,
        VerifierSubject::GlobalVariable {
            name: "g".to_owned(),
        },
        VerifierSubject::GlobalIfunc {
            name: "i".to_owned(),
        },
        VerifierSubject::Function {
            name: "f".to_owned(),
        },
        VerifierSubject::Block {
            function: "f".to_owned(),
            block: Some("bb".to_owned()),
        },
        VerifierSubject::Block {
            function: "f".to_owned(),
            block: None,
        },
    ];

    let rendered: Vec<String> = subjects.iter().map(ToString::to_string).collect();
    assert_eq!(
        rendered,
        [
            "module",
            "global @g",
            "ifunc @i",
            "function @f",
            "function @f, block %bb",
            "function @f, unnamed block",
        ]
    );
}

/// Every stored name in a real verifier failure is bare.
///
/// The sweep the previous test cannot do: it checks constructed values, this
/// checks what `verifier.rs` actually stores. A site that bakes `@` into the
/// name again fails here.
#[test]
fn no_verifier_failure_stores_a_sigil() -> Result<(), IrError> {
    fn stored_names(subject: &VerifierSubject) -> Vec<&str> {
        match subject {
            VerifierSubject::WholeModule => Vec::new(),
            VerifierSubject::GlobalVariable { name }
            | VerifierSubject::GlobalIfunc { name }
            | VerifierSubject::Function { name } => vec![name.as_str()],
            VerifierSubject::Block { function, block } => {
                let mut names = vec![function.as_str()];
                names.extend(block.as_deref());
                names
            }
        }
    }

    let scalable_global = {
        let m = module_new!("subject")?;
        let scalable = m.scalable_vector_type(m.i32_type().as_type(), 4);
        m.global_builder("s", scalable.as_type()).build()?;
        m.verify_borrowed().expect_err("invalid")
    };
    let bad_ifunc = {
        let m = module_new!("subject")?;
        let i32_ty = m.i32_type();
        let resolver = m.add_global("resolver", i32_ty.const_int(0i32))?;
        m.ifunc_builder("indirect", i32_ty.as_type(), m.view(resolver))
            .linkage(Linkage::Common)
            .build()?;
        m.verify_borrowed().expect_err("invalid")
    };
    let unterminated = {
        let m = module_new!("subject")?;
        let fn_ty = m.function_type_no_parameters(m.void_type());
        let f = m.add_function_dyn("empty", fn_ty, Linkage::External)?;
        let _entry = m.view(f).append_basic_block(&m, "entry");
        m.verify_borrowed().expect_err("invalid")
    };
    let address_taken = {
        let m = module_new!("subject")?;
        let descriptor = IntrinsicDescriptor::new(IntrinsicId::ABS, [m.i32_type().as_type()])?;
        let intrinsic = m.get_or_insert_intrinsic_declaration(&descriptor)?;
        m.add_global_constant("slot", m.view(intrinsic).as_global_constant_ptr())?;
        m.verify_borrowed().expect_err("invalid")
    };

    for error in [scalable_global, bad_ifunc, unterminated, address_taken] {
        for name in stored_names(subject_of(&error)) {
            assert!(
                !name.starts_with('@') && !name.starts_with('%'),
                "stored name {name:?} carries a sigil; Display supplies it"
            );
        }
    }
    Ok(())
}
