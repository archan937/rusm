// The store service as an isolated, supervised process: it runs store.Serve(), the
// receive→dispatch→reply loop around the service's operations (defined once in the shared
// todoboard/store package). Spawned on demand by the reporter and reached through the
// typed store.Client; it runs under its own manifest-declared profile whoever spawns it.
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"todoboard/store"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	store.Serve()
}
