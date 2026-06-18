; Direct translation of llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID
; forward `kw_blockaddress` placeholder resolution.

@addr = global ptr blockaddress(@f, %entry)

define i32 @f(i32 %x) {
entry:
  ret i32 %x
}
