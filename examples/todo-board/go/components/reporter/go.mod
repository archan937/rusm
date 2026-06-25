module reporter

go 1.24

require (
	github.com/archan937/rusm/packages/rusm-go v0.6.0
	todoboard v0.0.0
)

require go.bytecodealliance.org/cm v0.3.0 // indirect
replace todoboard => ../../shared
