




























define <vscale x 2 x half> @scalable_fp_vector_atomicrmw_fadd(ptr %x, <vscale x 2 x half> %val) {
; ERR3: :41: error: atomicrmw operand may not be scalable
  %atomic.fadd = atomicrmw fadd ptr %x, <vscale x 2 x half> %val seq_cst
  ret <vscale x 2 x half> %atomic.fadd
}

