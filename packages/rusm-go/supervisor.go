package rusm

import (
	"time"

	"go.bytecodealliance.org/cm"

	"github.com/archan937/rusm/packages/rusm-go/internal/wit/rusm/runtime/actor"
)

// Strategy is how a Supervisor reacts when one child dies.
type Strategy int

const (
	// OneForOne restarts only the child that died.
	OneForOne Strategy = iota
	// OneForAll restarts all children (terminating the survivors first).
	OneForAll
	// RestForOne restarts the dead child and every child started after it.
	RestForOne
)

// Supervisor supervises named child components — a thin facade over the host's single
// native supervisor (the supervise ABI), so the restart logic lives in exactly one
// place. Build it as a struct literal and Run it as the process body:
//
//	rusm.Supervisor{
//		Strategy:    rusm.OneForOne,
//		Children:    []string{"worker", "logger"},
//		MaxRestarts: 2,
//		Within:      time.Hour, // give up if more than 2 restarts happen in the window
//	}.Run()
type Supervisor struct {
	Strategy Strategy
	// Children are component names (as registered) to spawn and supervise.
	Children []string
	// MaxRestarts is the restart budget before the supervisor gives up; 0 = unlimited.
	MaxRestarts uint32
	// Within bounds MaxRestarts to a sliding window (Erlang's restart intensity); 0
	// counts MaxRestarts over the supervisor's whole lifetime.
	Within time.Duration
}

// Run hands the children to the host's native supervisor and parks as its owner: the
// host spawns, monitors, and restarts them under one implementation and links the
// supervisor to this process. It returns only if supervision is denied (e.g. the spawn
// capability is missing); otherwise it blocks — the link tears this process down when
// the supervisor gives up, and tears the children down if this process is killed.
func (s Supervisor) Run() {
	r := actor.Supervise(s.strategy(), cm.ToList(s.Children), s.MaxRestarts, s.withinMs())
	if r.IsErr() {
		return // supervision denied — nothing to own
	}
	for {
		ReceiveBytes() // park until the link takes us down
	}
}

func (s Supervisor) strategy() actor.SuperviseStrategy {
	switch s.Strategy {
	case OneForAll:
		return actor.SuperviseStrategyOneForAll
	case RestForOne:
		return actor.SuperviseStrategyRestForOne
	default:
		return actor.SuperviseStrategyOneForOne
	}
}

func (s Supervisor) withinMs() uint32 { return uint32(s.Within / time.Millisecond) }
