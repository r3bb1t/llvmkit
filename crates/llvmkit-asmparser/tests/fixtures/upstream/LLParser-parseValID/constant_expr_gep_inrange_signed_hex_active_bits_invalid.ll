; Directly ports LLLexer.cpp hexadecimal APSInt active-bit truncation
; plus LLParser.cpp::LLParser::parseValID inrange non-empty validation.

@addr = external global i64

define ptr @const_gep_inrange_signed_hex_active_bits_invalid() {
  ret ptr getelementptr inrange(s0x0, s0x1) (i8, ptr @addr, i64 100)
}
