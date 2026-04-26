; ModuleID = 'demo'
source_filename = "demo.c"

@msg = private unnamed_addr constant [13 x i8] c"hello world\0A\00"

define i32 @main() {
entry:
  %0 = alloca i32, align 4
  store i32 0, ptr %0, align 4
  ret i32 0
}
