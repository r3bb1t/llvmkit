; Direct translation of llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID
; `kw_blockaddress` accepted shape.

define void @f() {
entry:
  ret void
}

@addr = global ptr blockaddress(@f, %entry)
