
; PARSE-ERROR-4: error: expected ')'
@g = external global i8
define ptr @test() {
  ret ptr getelementptr inrange(42, 123 (i8, ptr @g, i64 8)
}
