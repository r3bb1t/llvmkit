; Excerpted from llvm/test/Bitcode/blockaddress-addrspace.ll return-fwddecl-good.ll.

define ptr addrspace(2) @take_as2() {
entry:
; CHECK: ret ptr addrspace(2) blockaddress(@fwddecl_as2, %bb)
  ret ptr addrspace(2) blockaddress(@fwddecl_as2, %bb)
}

define void @fwddecl_as2() addrspace(2) {
entry:
  unreachable
bb:
  ret void
}
