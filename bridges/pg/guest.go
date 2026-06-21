// Canonical source: bridges/pg/guest.go — the pg bridge's Go guest binding.
// Synced into rusm-go (packages/rusm-go/pg.go) by `make sync-bridges`; edit this
// file, not the copy. The `bridge_guest_in_sync` test fails the build on drift.

package rusm

import (
	"github.com/archan937/rusm/packages/rusm-go/internal/wit/rusm/runtime/pg"
)

// RegisterTag joins this process to a process-group tag (Erlang's pg); released on exit.
func RegisterTag(tag string) { pg.RegisterTag(tag) }

// UnregisterTag leaves a process-group tag this process holds.
func UnregisterTag(tag string) { pg.UnregisterTag(tag) }

// WhereisTag returns the live members of a process-group tag.
func WhereisTag(tag string) []Pid { return pids(pg.WhereisTag(tag)) }

// KillTag terminates every live member of a process-group tag and returns how many
// were killed (gated by process-control; 0 if denied or empty).
func KillTag(tag string) uint32 { return pg.KillTag(tag) }
