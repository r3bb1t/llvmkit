; Module-level slice the Session-2 parser handles end-to-end:
; target directives, named + numbered struct types, plain globals,
; a declaration, and a tiny function body.

target datalayout = "e-m:e-i64:64-f80:128-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"

source_filename = "parser_demo.ll"

%pair = type { i32, ptr }

@g  = global i32 7
@k  = constant i64 42
@0  = global i32 0

declare i32 @printf(ptr, ...)

define i32 @main() {
entry:
  %slot = alloca i32, align 4
  store i32 0, ptr %slot, align 4
  %v    = load i32, ptr %slot, align 4
  ret i32 %v
}
