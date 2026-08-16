




















define <vscale x 2 x ptr> @scalable_ptr_vector_atomicrmw_xchg(ptr %x, <vscale x 2 x ptr> %val) {
; ERR2: :41: error: atomicrmw operand may not be scalable
  %atomic.xchg = atomicrmw xchg ptr %x, <vscale x 2 x ptr> %val seq_cst
  ret <vscale x 2 x ptr> %atomic.xchg
}
