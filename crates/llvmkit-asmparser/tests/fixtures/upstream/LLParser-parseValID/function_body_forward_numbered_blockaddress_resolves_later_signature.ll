; Directly ports llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID
; ForwardRefBlockAddresses path for blockaddress naming a later numbered function.

define ptr @uses_forward_numbered_blockaddress() {
entry:
; CHECK: ret ptr blockaddress(@0, %entry)
  ret ptr blockaddress(@0, %entry)
}

define void @0() {
entry:
  ret void
}
