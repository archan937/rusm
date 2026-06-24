// Calls a "black hole" service (receives, never replies) with a 50ms timeout and
// sends "timeout", "err:…", or "no-timeout" to a collector. Exercises CallTimeout.
package main

import rusm "github.com/archan937/rusm/packages/rusm-go"

func init() { rusm.Run(run) }
func main() {}

func run() {
	hole, ok := rusm.ParsePid(rusm.ReceiveString())
	if !ok {
		return
	}
	collector, ok := rusm.ParsePid(rusm.ReceiveString())
	if !ok {
		return
	}
	_, err := rusm.CallTimeout[string](hole, "echo", 50)
	if err != nil && err.Error() == "timeout" {
		rusm.SendBytes(collector, []byte("timeout"))
	} else if err != nil {
		rusm.SendBytes(collector, []byte("err:"+err.Error()))
	} else {
		rusm.SendBytes(collector, []byte("no-timeout"))
	}
}
