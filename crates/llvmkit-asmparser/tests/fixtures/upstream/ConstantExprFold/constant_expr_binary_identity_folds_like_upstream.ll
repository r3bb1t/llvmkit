@A = global i64 0

@add = global ptr inttoptr (i64 add (i64 ptrtoint (ptr @A to i64), i64 0) to ptr) ; X + 0 == X
@sub = global ptr inttoptr (i64 sub (i64 ptrtoint (ptr @A to i64), i64 0) to ptr) ; X - 0 == X
@xor = global ptr inttoptr (i64 xor (i64 ptrtoint (ptr @A to i64), i64 0) to ptr) ; X ^ 0 == X
