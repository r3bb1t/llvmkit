

































target datalayout = "A1-G2-P3"
; ALLOCA-IN-GLOBALS: target datalayout = "A1-G2-P3"

define void @foo() {
  ; ALLOCA-IN-GLOBALS: %alloca = alloca i32, align 4, addrspace(2){{$}}
  ; ALLOCA-IN-GLOBALS: %alloca2 = alloca i32, align 4, addrspace(1){{$}}
  ; ALLOCA-IN-GLOBALS: %alloca3 = alloca i32, align 4{{$}}
  ; ALLOCA-IN-GLOBALS: %alloca4 = alloca i32, align 4, addrspace(3){{$}}
  %alloca = alloca i32, addrspace("G")
  %alloca2 = alloca i32, addrspace("A")
  %alloca3 = alloca i32
  %alloca4 = alloca i32, addrspace("P")
  ret void
}

