; INVALID-ACCESS-KIND: error: expected access kind (none, read, write, readwrite)
declare void @fn() memory(argmem: foo)
