; Excerpted verbatim from llvm/test/Assembler/pr119818.ll (the `; CHECK` block
; that file carries is `opt -S` output, not AsmWriter's, so it is not reproduced
; here — what the fixture pins for llvmkit is that the module parses at all).

@vm_exec_core.insns_address_table = internal constant [2 x ptr] [ptr blockaddress(@vm_exec_core, %0), ptr blockaddress(@vm_exec_core, %block)], align 16

define void @vm_exec_core() {
entry:
  br label %block

block:
  br label %0

0:
  ret void
}
