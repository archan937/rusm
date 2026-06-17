// A Go service: a package of typed handlers registered with the FnN adapters and run
// with Serve — the Go shape of a rusm-ts service / a rusm-rs #[service]. It answers the
// same JSON wire as the other guests, so a Rust or TS client (or the host, in the test)
// calls it unchanged.
package main

import rusm "github.com/archan937/rusm/packages/rusm-go"

func init() { rusm.Run(run) }
func main() {}

func run() {
	svc := rusm.NewService()
	svc.Handle("add", rusm.Fn2(func(a, b int) (int, error) { return a + b, nil }))
	svc.Handle("greet", rusm.Fn1(func(name string) (string, error) { return "hi " + name, nil }))
	svc.HandleStream("count", func(req rusm.Request, out rusm.Sink) error {
		n, err := rusm.Arg[int](req, 0)
		if err != nil {
			return err
		}
		for i := 1; i <= n; i++ {
			if !out.Send(i) {
				break // reader gone
			}
		}
		return nil
	})
	// countup(n, callback): invoke the caller's callback with 1..n, then reply "done".
	svc.Handle("countup", func(req rusm.Request) (any, error) {
		n, err := rusm.Arg[int](req, 0)
		if err != nil {
			return nil, err
		}
		cb := rusm.CallbackArg(req, 1)
		for i := 1; i <= n; i++ {
			cb.Call(i)
		}
		return "done", nil
	})
	svc.Serve()
}
