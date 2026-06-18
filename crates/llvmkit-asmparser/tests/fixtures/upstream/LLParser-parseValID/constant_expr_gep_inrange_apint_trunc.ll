; Direct translation of llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID
; constant getelementptr inrange APInt extOrTrunc-to-index-width branch.

@addr = external global i64

define ptr @const_gep_inrange_apint_trunc() {
; CHECK: ret ptr getelementptr inrange(0, 1) (i8, ptr @addr, i64 100)
  ret ptr getelementptr inrange(18446744073709551616, 18446744073709551617) (i8, ptr @addr, i64 100)
}
