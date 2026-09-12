













target datalayout = "A1-G2-P3"
; CHECK: target datalayout = "A1-G2-P3"

; CHECK: @str = private addrspace(2) constant [4 x i8] c"str\00"
@str = private addrspace("G") constant [4 x i8] c"str\00"

define void @foo() {
  ; CHECK: %alloca = alloca i32, align 4, addrspace(1)
  %alloca = alloca i32, addrspace("A")
  ret void
}

; CHECK: define void @bar() addrspace(3) {
define void @bar() addrspace("P") {
  ; CHECK: call addrspace(3) void @foo()
  call addrspace("P") void @foo()
  ret void
}

