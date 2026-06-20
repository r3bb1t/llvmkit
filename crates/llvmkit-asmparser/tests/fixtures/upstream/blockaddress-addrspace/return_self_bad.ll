; Excerpted verbatim from llvm/test/Bitcode/blockaddress-addrspace.ll return-self-bad.ll.

target datalayout = "P2"
define ptr addrspace(2) @take_self_bad() addrspace(1) {
L1:
  br label %L2
L2:
  ret ptr addrspace(2) blockaddress(@take_self_bad, %L3)
  ; CHECK: [[#@LINE-1]]:24: error: constant expression type mismatch: got type 'ptr addrspace(1)' but expected 'ptr addrspace(2)'
L3:
  unreachable
}
