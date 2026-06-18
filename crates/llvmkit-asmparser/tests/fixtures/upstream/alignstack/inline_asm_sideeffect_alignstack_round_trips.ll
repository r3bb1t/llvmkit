; Excerpted from llvm/test/Assembler/alignstack.ll.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

define void @test2() nounwind {
; CHECK: test2
; CHECK: sideeffect
; CHECK: alignstack
	tail call void asm sideeffect alignstack "mov", "~{dirflag},~{fpsr},~{flags}"() nounwind
	ret void
; CHECK: ret
}
