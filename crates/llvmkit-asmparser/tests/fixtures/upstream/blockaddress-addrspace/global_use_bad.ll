; Excerpted verbatim from llvm/test/Bitcode/blockaddress-addrspace.ll global-use-bad.ll.

define void @fn() addrspace(1) {
  unreachable
bb:
  ret void
}
@global1 = constant ptr addrspace(2) blockaddress(@fn, %bb)
; CHECK: [[#@LINE-1]]:38: error: constant expression type mismatch: got type 'ptr addrspace(1)' but expected 'ptr addrspace(2)'
