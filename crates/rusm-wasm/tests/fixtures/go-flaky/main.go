// A supervised child (the Go twin of rs-flaky): announce which pid we are to the
// registered collector, then block until killed. Each (re)start announces a fresh pid,
// so a test can watch the supervisor restart us.
package main

import rusm "github.com/archan937/rusm/packages/rusm-go"

func init() { rusm.Run(run) }
func main() {}

func run() {
	if collector, ok := rusm.Whereis("collector"); ok {
		rusm.SendBytes(collector, []byte("started:"+rusm.Self().String()))
	}
	for {
		rusm.ReceiveBytes() // wait to be killed
	}
}
