// A guest written in plain Go with the ergonomic rusm-go API (the Go twin of the
// rs-guest fixture). The whole component shell is `init`+empty `main`; the body is
// normal Go. On run it learns who to answer (the first message, a decimal pid),
// labels itself, and replies — the Go counterpart of rs-guest's run.
package main

import rusm "github.com/archan937/rusm/packages/rusm-go"

func init() { rusm.Run(run) }
func main() {}

func run() {
	replyTo, ok := rusm.ParsePid(rusm.ReceiveString())
	if !ok {
		return
	}
	rusm.SetLabel("go-guest")
	rusm.SendBytes(replyTo, []byte("hello from "+rusm.Self().String()))
}
