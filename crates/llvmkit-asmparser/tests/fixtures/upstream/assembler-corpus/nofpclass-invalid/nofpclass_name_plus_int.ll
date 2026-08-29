; NAME-PLUS-INT: error: expected nofpclass test mask
define void @nofpclass_name_plus_int(float nofpclass(nan 42) %x) {
  ret void
}

