; Directly ports llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID
; `inrange` endpoints parsed as lltok::APSInt, including s0x/u0x tokens.

@addr = external global i64

define ptr @const_gep_inrange_hex_apsint() {
; CHECK: ret ptr getelementptr inrange(0, 1) (i8, ptr @addr, i64 100)
  ret ptr getelementptr inrange(s0x0, u0x1) (i8, ptr @addr, i64 100)
}
