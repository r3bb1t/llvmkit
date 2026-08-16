; DEFAULT-AFTER-LOC: error: default access kind must be specified first
declare void @fn() memory(argmem: read, write)
