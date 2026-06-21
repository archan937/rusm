// Canonical source: bridges/actor/guest.go — the actor bridge's Go guest binding (the
// Erlang Process core: Pid, Send/Receive, Spawn, Monitor, the registry). Synced into rusm-go
// (packages/rusm-go/actor.go) by `make sync-bridges`; edit this file, not the copy. Package
// infrastructure (Run, the opt helpers, pids) stays in rusm.go. `bridge_guest_in_sync`
// guards drift.

package rusm

import (
	"bytes"
	"encoding/json"
	"errors"
	"strconv"
	"strings"

	"go.bytecodealliance.org/cm"

	"github.com/archan937/rusm/packages/rusm-go/internal/wit/rusm/runtime/actor"
)

// Pid identifies a process (Erlang's pid).
type Pid uint64

// String renders the pid as a decimal — the form it takes on the message wire.
func (p Pid) String() string { return strconv.FormatUint(uint64(p), 10) }

// ParsePid parses a decimal pid (the wire form); ok is false if s is not a pid.
func ParsePid(s string) (pid Pid, ok bool) {
	n, err := strconv.ParseUint(strings.TrimSpace(s), 10, 64)
	if err != nil {
		return 0, false
	}
	return Pid(n), true
}

// Self returns this process's own pid (Erlang's self()).
func Self() Pid { return Pid(actor.OwnPid()) }

// SendBytes sends raw bytes to a pid (silently dropped if it is gone).
func SendBytes(to Pid, msg []byte) { actor.Send(actor.Pid(to), cm.ToList(msg)) }

// Send JSON-encodes msg and sends it — the wire shared with rusm-ts and rusm-rs.
func Send(to Pid, msg any) error {
	b, err := json.Marshal(msg)
	if err != nil {
		return err
	}
	SendBytes(to, b)
	return nil
}

// stash holds messages the RPC client set aside while awaiting a reply, so the app's
// own Receive still sees them (the guest is single-threaded — one mailbox).
var stash [][]byte

// stashMessage sets a message aside for the app's own Receive (used by the wire client).
func stashMessage(raw []byte) { stash = append(stash, raw) }

// ReceiveBytes blocks until the next message arrives and returns its raw bytes (FIFO),
// draining any mail the wire client set aside first.
func ReceiveBytes() []byte {
	if len(stash) > 0 {
		raw := stash[0]
		stash = stash[1:]
		return raw
	}
	return actor.Receive().Slice()
}

// ReceiveBytesTimeout is ReceiveBytes with a deadline: ok is false after timeoutMs
// with no message (Erlang's `receive … after`). Stashed mail returns immediately.
func ReceiveBytesTimeout(timeoutMs uint64) (msg []byte, ok bool) {
	if len(stash) > 0 {
		raw := stash[0]
		stash = stash[1:]
		return raw, true
	}
	o := actor.ReceiveTimeout(timeoutMs)
	if o.None() {
		return nil, false
	}
	return o.Some().Slice(), true
}

// Receive blocks for the next message and decodes it as JSON into a value of type T.
func Receive[T any]() (T, error) {
	var v T
	err := json.Unmarshal(ReceiveBytes(), &v)
	return v, err
}

// ReceiveString blocks for the next message and returns it as a string.
func ReceiveString() string { return string(ReceiveBytes()) }

// Spawn starts a registered component by name and returns its pid (capability-gated).
func Spawn(component string) (Pid, error) {
	r := actor.Spawn(component)
	if r.IsErr() {
		return 0, errors.New(*r.Err())
	}
	return Pid(*r.OK()), nil
}

// SpawnFrom spawns a dynamic JS instance of a registered runner template, loading its
// bundle at runtime from source — "inline:<js>" (the bundle itself), "kv:<bucket>/<key>"
// (the node store), or "url:"/"http(s)://…" (fetched). The JS runs under the template's
// declared capability profile (the guest chooses the code, never the capabilities); gated
// by the spawn capability plus the source's I/O capability (storage / network).
func SpawnFrom(component, source string) (Pid, error) {
	r := actor.SpawnFrom(component, source)
	if r.IsErr() {
		return 0, errors.New(*r.Err())
	}
	return Pid(*r.OK()), nil
}

// Monitor watches target: when it dies, this process receives a __down message — the
// basis for a Supervisor.
func Monitor(target Pid) { actor.Monitor(actor.Pid(target)) }

// DownPid parses a monitor __down message ({"__down":"<pid>","reason":...}) into the
// dead process's Pid; ok is false for an ordinary message. The single source for __down
// decoding in this SDK — a fast prefix check keeps ordinary messages from being parsed.
func DownPid(msg []byte) (pid Pid, ok bool) {
	if !bytes.HasPrefix(msg, []byte(`{"__down":"`)) {
		return 0, false
	}
	var v struct {
		Down string `json:"__down"`
	}
	if json.Unmarshal(msg, &v) != nil {
		return 0, false
	}
	return ParsePid(v.Down)
}

// Register registers this process under name in the node registry.
func Register(name string) bool { return actor.Register(name) }

// Whereis looks up a registered name; ok is false if it is unregistered.
func Whereis(name string) (pid Pid, ok bool) {
	o := actor.Whereis(name)
	if o.None() {
		return 0, false
	}
	return Pid(*o.Some()), true
}

// Unregister releases a registered name.
func Unregister(name string) bool { return actor.Unregister(name) }

// SetLabel sets this process's human-readable label (shown in introspection).
func SetLabel(label string) { actor.SetLabel(label) }

// List returns every live pid (subject to capability).
func List() []Pid { return pids(actor.ListProcesses()) }

// IsAlive reports whether a pid is still alive.
func IsAlive(p Pid) bool { return actor.IsAlive(actor.Pid(p)) }

// Kill terminates a pid (subject to capability); false if it was already gone.
func Kill(p Pid) bool { return actor.Kill(actor.Pid(p)) }

// ProcessInfo is a snapshot of a process (Erlang's Process.info/1).
type ProcessInfo struct {
	Pid          Pid
	Links        uint32
	Monitors     uint32
	Names        []string
	Label        string
	MailboxDepth uint32
	TrapExit     bool
}

// Info returns a snapshot of a process; ok is false if it is gone.
func Info(target Pid) (info ProcessInfo, ok bool) {
	o := actor.Info(actor.Pid(target))
	if o.None() {
		return ProcessInfo{}, false
	}
	i := o.Some()
	label := ""
	if !i.Label.None() {
		label = i.Label.Value()
	}
	return ProcessInfo{
		Pid:          Pid(i.Pid),
		Links:        i.Links,
		Monitors:     i.Monitors,
		Names:        i.Names.Slice(),
		Label:        label,
		MailboxDepth: i.MailboxDepth,
		TrapExit:     i.TrapExit,
	}, true
}
