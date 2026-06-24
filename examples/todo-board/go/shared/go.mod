// The shared code for the Go todo board — the todos data layer and the store service
// contract, as two packages (todoboard/todos, todoboard/store) in one module the
// components import via a local `replace`. Never built directly; pulled in by the
// component crates.
module todoboard

go 1.24

require github.com/archan937/rusm/packages/rusm-go v0.5.0

require go.bytecodealliance.org/cm v0.3.0 // indirect
