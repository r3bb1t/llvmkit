; Directly ports llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID
; ForwardRefBlockAddresses path for blockaddress naming a later-defined function.

define ptr @uses_forward_blockaddress() {
entry:
; CHECK: ret ptr blockaddress(@f, %entry)
  ret ptr blockaddress(@f, %entry)
}

define void @f() {
entry:
  ret void
}
