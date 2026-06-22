// A Go client whose typed Call must not be fooled by a colliding-ref request arriving during
// the wait (the wire ref-collision regression — the Go twin of the rusm-rs/rpc.js fix). It
// calls a noisy echoer that sends a same-ref request just before its real reply, and reports
// the decoded result — proving Call sets the request aside and returns the genuine reply.
package main

import rusm "github.com/archan937/rusm/packages/rusm-go"

func init() { rusm.Run(run) }
func main() {}

func run() {
	echoer, ok := rusm.ParsePid(rusm.ReceiveString())
	if !ok {
		return
	}
	collector, ok := rusm.ParsePid(rusm.ReceiveString())
	if !ok {
		return
	}
	r, err := rusm.Call[string](echoer, "echo", "hi")
	if err != nil {
		rusm.SendBytes(collector, []byte("err:"+err.Error()))
		return
	}
	rusm.SendBytes(collector, []byte("got:"+r))
}
