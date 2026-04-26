//! Calling conventions. Mirrors `llvm/include/llvm/IR/CallingConv.h`.
//!
//! Upstream defines `using ID = unsigned` plus an open `enum` of well-known
//! values (`CallingConv.h`). The Rust port therefore uses a `u32`
//! newtype with associated constants — that's the only shape that supports
//! arbitrary numeric IDs (which LLVM IR permits up to `MaxID = 1023`)
//! without losing the readable names.
//!
//! The constants and discriminants match the upstream `CallingConv` enum
//! exactly, so a downstream parser can round-trip via `from_raw` / `as_raw`.

use core::fmt;

/// LLVM calling convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(transparent)]
pub struct CallingConv(u32);

#[allow(non_upper_case_globals)] // mirrors LLVM's mixed-case names verbatim
impl CallingConv {
    /// Default calling convention compatible with C. Supports varargs.
    pub const C: Self = Self(0);

    pub const Fast: Self = Self(8);
    pub const Cold: Self = Self(9);
    pub const GHC: Self = Self(10);
    pub const HiPE: Self = Self(11);
    pub const AnyReg: Self = Self(13);
    pub const PreserveMost: Self = Self(14);
    pub const PreserveAll: Self = Self(15);
    pub const Swift: Self = Self(16);
    pub const CXX_FAST_TLS: Self = Self(17);
    pub const Tail: Self = Self(18);
    pub const CFGuard_Check: Self = Self(19);
    pub const SwiftTail: Self = Self(20);
    pub const PreserveNone: Self = Self(21);

    /// Start of the target-specific block (`FirstTargetCC`).
    pub const FIRST_TARGET: Self = Self(64);

    pub const X86_StdCall: Self = Self(64);
    pub const X86_FastCall: Self = Self(65);
    pub const ARM_APCS: Self = Self(66);
    pub const ARM_AAPCS: Self = Self(67);
    pub const ARM_AAPCS_VFP: Self = Self(68);
    pub const MSP430_INTR: Self = Self(69);
    pub const X86_ThisCall: Self = Self(70);
    pub const PTX_Kernel: Self = Self(71);
    pub const PTX_Device: Self = Self(72);
    pub const SPIR_FUNC: Self = Self(75);
    pub const SPIR_KERNEL: Self = Self(76);
    pub const Intel_OCL_BI: Self = Self(77);
    pub const X86_64_SysV: Self = Self(78);
    pub const Win64: Self = Self(79);
    pub const X86_VectorCall: Self = Self(80);
    /// Reserved for HHVM (deprecated upstream).
    pub const DUMMY_HHVM: Self = Self(81);
    /// Reserved for HHVM_C (deprecated upstream).
    pub const DUMMY_HHVM_C: Self = Self(82);
    pub const X86_INTR: Self = Self(83);
    pub const AVR_INTR: Self = Self(84);
    pub const AVR_SIGNAL: Self = Self(85);
    pub const AVR_BUILTIN: Self = Self(86);
    pub const AMDGPU_VS: Self = Self(87);
    pub const AMDGPU_GS: Self = Self(88);
    pub const AMDGPU_PS: Self = Self(89);
    pub const AMDGPU_CS: Self = Self(90);
    pub const AMDGPU_KERNEL: Self = Self(91);
    pub const X86_RegCall: Self = Self(92);
    pub const AMDGPU_HS: Self = Self(93);
    pub const MSP430_BUILTIN: Self = Self(94);
    pub const AMDGPU_LS: Self = Self(95);
    pub const AMDGPU_ES: Self = Self(96);
    pub const AArch64_VectorCall: Self = Self(97);
    pub const AArch64_SVE_VectorCall: Self = Self(98);
    pub const WASM_EmscriptenInvoke: Self = Self(99);
    pub const AMDGPU_Gfx: Self = Self(100);
    pub const M68k_INTR: Self = Self(101);
    pub const AArch64_SME_ABI_Support_Routines_PreserveMost_From_X0: Self = Self(102);
    pub const AArch64_SME_ABI_Support_Routines_PreserveMost_From_X2: Self = Self(103);
    pub const AMDGPU_CS_Chain: Self = Self(104);
    pub const AMDGPU_CS_ChainPreserve: Self = Self(105);
    pub const M68k_RTD: Self = Self(106);
    pub const GRAAL: Self = Self(107);
    pub const ARM64EC_Thunk_X64: Self = Self(108);
    pub const ARM64EC_Thunk_Native: Self = Self(109);
    pub const RISCV_VectorCall: Self = Self(110);
    pub const AArch64_SME_ABI_Support_Routines_PreserveMost_From_X1: Self = Self(111);
    pub const RISCV_VLSCall_32: Self = Self(112);
    pub const RISCV_VLSCall_64: Self = Self(113);
    pub const RISCV_VLSCall_128: Self = Self(114);
    pub const RISCV_VLSCall_256: Self = Self(115);
    pub const RISCV_VLSCall_512: Self = Self(116);
    pub const RISCV_VLSCall_1024: Self = Self(117);
    pub const RISCV_VLSCall_2048: Self = Self(118);
    pub const RISCV_VLSCall_4096: Self = Self(119);
    pub const RISCV_VLSCall_8192: Self = Self(120);
    pub const RISCV_VLSCall_16384: Self = Self(121);
    pub const RISCV_VLSCall_32768: Self = Self(122);
    pub const RISCV_VLSCall_65536: Self = Self(123);
    pub const AMDGPU_Gfx_WholeWave: Self = Self(124);
    pub const CHERIoT_CompartmentCall: Self = Self(125);
    pub const CHERIoT_CompartmentCallee: Self = Self(126);
    pub const CHERIoT_LibraryCall: Self = Self(127);

    /// Highest legal raw value (`MaxID = 1023`, `CallingConv.h`).
    pub const MAX: u32 = 1023;
}

impl CallingConv {
    /// Construct from the raw numeric ID. Returns `None` if `> Self::MAX`.
    #[inline]
    pub const fn from_raw(raw: u32) -> Option<Self> {
        if raw <= Self::MAX {
            Some(Self(raw))
        } else {
            None
        }
    }

    /// Raw numeric ID.
    #[inline]
    pub const fn as_raw(self) -> u32 {
        self.0
    }

    /// `true` iff the convention permits direct or indirect call-like
    /// dispatch. Mirrors `isCallableCC` (`CallingConv.h`).
    pub const fn is_callable(self) -> bool {
        match self.0 {
            // AMDGPU intrinsic-only:
            104 | 105 | 124 => false,
            // Hardware entry points:
            76 | 87..=91 | 93 | 95 | 96 => false,
            _ => true,
        }
    }

    /// Optional well-known mnemonic. Returns `None` for IDs whose textual
    /// form is parameterised (`riscv_vls_cc(<N>)`) or that LLVM's
    /// AsmWriter falls back to `cc <num>` for; the [`fmt::Display`] impl handles those.
    /// Strings match `printCallingConv` in `lib/IR/AsmWriter.cpp`.
    pub const fn name(self) -> Option<&'static str> {
        Some(match self.0 {
            0 => "ccc",
            8 => "fastcc",
            9 => "coldcc",
            10 => "ghccc",
            13 => "anyregcc",
            14 => "preserve_mostcc",
            15 => "preserve_allcc",
            16 => "swiftcc",
            17 => "cxx_fast_tlscc",
            18 => "tailcc",
            19 => "cfguard_checkcc",
            20 => "swifttailcc",
            21 => "preserve_nonecc",
            64 => "x86_stdcallcc",
            65 => "x86_fastcallcc",
            66 => "arm_apcscc",
            67 => "arm_aapcscc",
            68 => "arm_aapcs_vfpcc",
            69 => "msp430_intrcc",
            70 => "x86_thiscallcc",
            71 => "ptx_kernel",
            72 => "ptx_device",
            75 => "spir_func",
            76 => "spir_kernel",
            77 => "intel_ocl_bicc",
            78 => "x86_64_sysvcc",
            79 => "win64cc",
            80 => "x86_vectorcallcc",
            81 => "hhvmcc",
            82 => "hhvm_ccc",
            83 => "x86_intrcc",
            84 => "avr_intrcc",
            85 => "avr_signalcc",
            87 => "amdgpu_vs",
            88 => "amdgpu_gs",
            89 => "amdgpu_ps",
            90 => "amdgpu_cs",
            91 => "amdgpu_kernel",
            92 => "x86_regcallcc",
            93 => "amdgpu_hs",
            95 => "amdgpu_ls",
            96 => "amdgpu_es",
            97 => "aarch64_vector_pcs",
            98 => "aarch64_sve_vector_pcs",
            100 => "amdgpu_gfx",
            102 => "aarch64_sme_preservemost_from_x0",
            103 => "aarch64_sme_preservemost_from_x2",
            104 => "amdgpu_cs_chain",
            105 => "amdgpu_cs_chain_preserve",
            106 => "m68k_rtdcc",
            107 => "graalcc",
            110 => "riscv_vector_cc",
            111 => "aarch64_sme_preservemost_from_x1",
            124 => "amdgpu_gfx_whole_wave",
            125 => "cheriot_compartmentcallcc",
            126 => "cheriot_compartmentcalleecc",
            127 => "cheriot_librarycallcc",
            _ => return None,
        })
    }

    /// VLEN parameter for a `RISCV_VLSCall_<N>` convention, else `None`.
    /// Mirrors the `CC_VLS_CASE` macro in `AsmWriter.cpp`.
    pub const fn riscv_vls_vlen(self) -> Option<u32> {
        match self.0 {
            112 => Some(32),
            113 => Some(64),
            114 => Some(128),
            115 => Some(256),
            116 => Some(512),
            117 => Some(1024),
            118 => Some(2048),
            119 => Some(4096),
            120 => Some(8192),
            121 => Some(16384),
            122 => Some(32768),
            123 => Some(65536),
            _ => None,
        }
    }
}

impl fmt::Display for CallingConv {
    /// Print the canonical IR name; for `RISCV_VLSCall_<N>` emit
    /// `riscv_vls_cc(<N>)`; otherwise fall back to `cc <num>` like the
    /// default branch in AsmWriter (`AsmWriter.cpp`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(n) = self.riscv_vls_vlen() {
            return write!(f, "riscv_vls_cc({n})");
        }
        match self.name() {
            Some(s) => f.write_str(s),
            None => write!(f, "cc {}", self.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_c() {
        assert_eq!(CallingConv::default(), CallingConv::C);
    }

    #[test]
    fn round_trip_known() {
        for cc in [
            CallingConv::C,
            CallingConv::Fast,
            CallingConv::SwiftTail,
            CallingConv::AMDGPU_KERNEL,
            CallingConv::CHERIoT_LibraryCall,
        ] {
            assert_eq!(CallingConv::from_raw(cc.as_raw()), Some(cc));
            assert!(cc.name().is_some());
        }
    }

    #[test]
    fn rejects_out_of_range() {
        assert_eq!(CallingConv::from_raw(1024), None);
        assert!(CallingConv::from_raw(1023).is_some());
    }

    #[test]
    fn callable_partition() {
        assert!(CallingConv::C.is_callable());
        assert!(CallingConv::Fast.is_callable());
        assert!(!CallingConv::SPIR_KERNEL.is_callable());
        assert!(!CallingConv::AMDGPU_KERNEL.is_callable());
        assert!(!CallingConv::AMDGPU_CS_Chain.is_callable());
        assert!(!CallingConv::AMDGPU_Gfx_WholeWave.is_callable());
    }

    #[test]
    fn display_named_and_numeric() {
        assert_eq!(format!("{}", CallingConv::C), "ccc");
        assert_eq!(format!("{}", CallingConv::Fast), "fastcc");
        // 12 is unassigned (was WebKit_JS, removed):
        let unknown = CallingConv::from_raw(12).unwrap();
        assert_eq!(format!("{unknown}"), "cc 12");
    }

    #[test]
    fn display_riscv_vls_parameterised() {
        assert_eq!(
            format!("{}", CallingConv::RISCV_VLSCall_512),
            "riscv_vls_cc(512)"
        );
    }

    #[test]
    fn unsupported_named_falls_back_to_numeric() {
        // HiPE has an enum slot but no AsmWriter mnemonic.
        assert_eq!(format!("{}", CallingConv::HiPE), "cc 11");
        assert!(CallingConv::HiPE.name().is_none());
    }
}
