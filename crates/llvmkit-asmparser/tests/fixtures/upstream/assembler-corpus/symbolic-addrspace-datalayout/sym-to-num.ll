
















































target datalayout = "P11-p2(global):32:8-p8(stack):8:8-p11(code):8:8"
; CHECK: target datalayout = "P11-p2(global):32:8-p8(stack):8:8-p11(code):8:8"

; CHECK: @str = private addrspace(2) constant [4 x i8] c"str\00"
@str = private addrspace("global") constant [4 x i8] c"str\00"

define void @foo() {
  ; CHECK: %alloca = alloca i32, align 4, addrspace(8)
  %alloca = alloca i32, addrspace("stack")
  ret void
}

; CHECK: define void @bar() addrspace(11)
define void @bar() addrspace(11) {
  ; CHECK: call addrspace(11) void @foo()
  call addrspace("code") void @foo()
  ret void
}

