; llvmkit-authored negative: LLVM 22.1.4 ships no test/Assembler fixture for
; callbr's *absence* of an address space. The rule is
; llvm/lib/AsmParser/LLParser.cpp::parseCallBr, whose `||` chain runs
; parseOptionalCallingConv -> parseOptionalReturnAttrs -> parseType with no
; parseOptionalProgramAddrSpace, and which resolves the callee with
; convertValIDToValue(PointerType::getUnqual(Context), ...).
define void @f() {
  callbr addrspace(1) void asm "", ""() to label %x []
x:
  ret void
}
