//! In-crate home for raw-phi tests that block arguments cannot express.
//!
//! When the raw phi builders (`build_*_phi`, the open-phi `add_incoming` /
//! `finish` mutators, and the untyped `phi_add_incoming_from_value`) went
//! `pub(crate)`, integration tests under `tests/` could no longer author a phi
//! fed from a `switch` / `invoke` / `callbr` predecessor, build a deliberately
//! malformed phi to check the verifier rejects it, exercise the raw phi
//! typestate (`Open` → `Closed` → `finish`, phi-head placement, `PhiKind`
//! rediscovery), or drive the untyped add path. Those tests live here, in the
//! crate, where the now-internal builders are reachable. Everything block
//! arguments *can* express stays an integration test on the public API.
//!
//! These submodules were relocated verbatim from the `tests/` files named
//! after each one (paths changed `llvmkit_ir::` → `crate::`); their bodies and
//! assertions are unchanged.

mod analysis_switch;
mod constant_folding;
mod fmf;
mod medium;
mod typestate;
mod verifier_basic;
mod zero_incoming_phi;
