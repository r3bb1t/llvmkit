define void @f() {
entry:
  unreachable
}

@a = global ptr blockaddress(@f, %0)
