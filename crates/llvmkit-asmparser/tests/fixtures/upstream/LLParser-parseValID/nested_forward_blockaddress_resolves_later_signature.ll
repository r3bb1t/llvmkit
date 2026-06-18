; Direct translation of llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID
; forward `kw_blockaddress` placeholder resolution inside an aggregate initializer.

@addrs = global [1 x ptr] [ptr blockaddress(@f, %entry)]

define i32 @f(i32 %x) {
entry:
  ret i32 %x
}
