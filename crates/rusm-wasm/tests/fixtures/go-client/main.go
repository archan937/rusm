// A Go client: it reaches a service with the typed Call[R] over the JSON wire, the Go
// shape of rusm-ts's spawn<Svc>() client / rusm-rs's typed Client. On run it learns the
// service pid and a reply-to pid, calls two methods, and reports the results — proving
// Call encodes requests and decodes typed replies (with request-id matching).
package main

import (
	"fmt"

	rusm "github.com/archan937/rusm/packages/rusm-go"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	service, ok := rusm.ParsePid(rusm.ReceiveString())
	if !ok {
		return
	}
	replyTo, ok := rusm.ParsePid(rusm.ReceiveString())
	if !ok {
		return
	}

	sum, err := rusm.Call[int](service, "add", 2, 3)
	if err != nil {
		rusm.SendBytes(replyTo, []byte("err: "+err.Error()))
		return
	}
	greet, err := rusm.Call[string](service, "greet", "ada")
	if err != nil {
		rusm.SendBytes(replyTo, []byte("err: "+err.Error()))
		return
	}

	var counted []int
	for v := range rusm.CallStream[int](service, "count", 3) {
		counted = append(counted, v)
	}

	// A callback call: the service invokes our closure with 1..3 during the call.
	var ticks []int
	done, err := rusm.Call[string](service, "countup", 3, rusm.CB(func(i int) {
		ticks = append(ticks, i)
	}))
	if err != nil {
		rusm.SendBytes(replyTo, []byte("err: "+err.Error()))
		return
	}

	rusm.SendBytes(replyTo, []byte(fmt.Sprintf(
		"sum=%d greet=%s count=%v countup=%s ticks=%v", sum, greet, counted, done, ticks)))
}
