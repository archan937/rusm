// A Go guest that loads JS at runtime: it spawns a dynamic instance of the "runner"
// template with an **inline** JS bundle (the plain-string source). The loaded JS runs on
// the js-runner under the template's declared profile and messages the collector — proving
// a guest in one language can spawn runtime-determined JS, isolated, via SpawnFrom.
package main

import rusm "github.com/archan937/rusm/packages/rusm-go"

func init() { rusm.Run(run) }
func main() {}

func run() {
	js := `module.exports.default = async function () {
		Process.send(Process.whereis("collector"), "ran from go");
	};`
	if _, err := rusm.SpawnFrom("runner", "inline:"+js); err != nil {
		if c, ok := rusm.Whereis("collector"); ok {
			rusm.SendBytes(c, []byte("err: "+err.Error()))
		}
	}
}
