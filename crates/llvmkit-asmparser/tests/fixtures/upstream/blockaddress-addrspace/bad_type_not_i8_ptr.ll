; Excerpted verbatim from llvm/test/Bitcode/blockaddress-addrspace.ll bad-type-not-i8-ptr.ll.

@global = constant ptr blockaddress(@unknown_fn, %bb)
; CHECK: [[#@LINE-1]]:37: error: expected function name in blockaddress
