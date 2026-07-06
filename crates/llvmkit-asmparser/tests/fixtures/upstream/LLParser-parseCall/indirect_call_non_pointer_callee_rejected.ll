; Mirrors llvm/lib/AsmParser/LLParser.cpp::PerFunctionState::getVal's type
; check at the callee position: upstream llvm-as rejects this module with
; "'%x' defined with type 'i32' but expected 'ptr'". llvmkit surfaces the
; same rule when converting the parsed callee value to a pointer. No
; upstream lit coverage of the diagnostic exists at 22.1.4; the rule
; shape is the anchor (D11).

define void @caller(i32 %x) {
entry:
  call void (i32) %x(i32 1)
  ret void
}
