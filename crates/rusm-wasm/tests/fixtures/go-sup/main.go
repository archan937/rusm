// A supervisor component (the Go twin of rs-sup): supervise the `flaky` child
// one-for-one and restart it when it dies — with restart intensity, giving up if more
// than 2 restarts happen within an hour. One kill restarts the child; a rapid burst of
// kills trips the limit and the supervisor itself exits.
package main

import (
	"time"

	rusm "github.com/archan937/rusm/packages/rusm-go"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	rusm.Supervisor{
		Strategy:    rusm.OneForOne,
		Children:    []string{"flaky"},
		MaxRestarts: 2,
		Within:      time.Hour,
	}.Run()
}
