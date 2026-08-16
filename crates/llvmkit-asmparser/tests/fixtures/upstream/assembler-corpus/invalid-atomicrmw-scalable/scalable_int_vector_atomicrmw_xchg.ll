













define <vscale x 2 x i16> @scalable_int_vector_atomicrmw_xchg(ptr %x, <vscale x 2 x i16> %val) {
; ERR1: :41: error: atomicrmw operand may not be scalable
  %atomic.xchg = atomicrmw xchg ptr %x, <vscale x 2 x i16> %val seq_cst
  ret <vscale x 2 x i16> %atomic.xchg
}
