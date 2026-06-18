; Directly ports llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID
; ForwardRefBlockAddresses RAUW through constant aggregate users.

declare void @sink([1 x ptr])

define void @uses_forward_aggregate_blockaddress() {
entry:
; CHECK: call void @sink([1 x ptr] [ptr blockaddress(@f, %entry)])
  call void @sink([1 x ptr] [ptr blockaddress(@f, %entry)])
  ret void
}

define void @f() {
entry:
  ret void
}
