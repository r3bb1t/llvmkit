//! The intrinsic emitter.
//!
//! Ports `llvm/utils/TableGen/Basic/`, the TableGen backend that turns the
//! parsed records into tables. Upstream keeps this beside the `llvm-tblgen`
//! tool rather than in the TableGen library; llvmkit keeps both in one crate,
//! and this module is the seam.

pub(crate) mod code_gen_intrinsics;
pub(crate) mod intrinsic_emitter;
