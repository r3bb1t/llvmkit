declare void @f()

define void @g() {
  tail void @f()
  ret void
}
