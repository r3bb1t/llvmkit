; Excerpted verbatim from llvm/test/Bitcode/blockaddress-addrspace.ll return-self-good.ll.

target datalayout = "P2"
define ptr addrspace(0) @take_self_as0() addrspace(0) {
L1:
  br label %L2
L2:
  ret ptr addrspace(0) blockaddress(@take_self_as0, %L3)
L3:
  unreachable
}
define ptr addrspace(2) @take_self_prog_as() {
L1:
  br label %L2
L2:
  ret ptr addrspace(2) blockaddress(@take_self_prog_as, %L3)
L3:
  unreachable
}
define ptr addrspace(1) @take_self_as1() addrspace(1) {
L1:
  br label %L2
L2:
  ret ptr addrspace(1) blockaddress(@take_self_as1, %L3)
L3:
  unreachable
}
define ptr addrspace(2) @take_self_as2() addrspace(2) {
L1:
  br label %L2
L2:
  ret ptr addrspace(2) blockaddress(@take_self_as2, %L3)
L3:
  unreachable
}
