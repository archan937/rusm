package rusm

import (
	"go.bytecodealliance.org/cm"

	"github.com/archan937/rusm/packages/rusm-go/internal/wit/rusm/runtime/actor"
)

// Stream is a back-pressured byte stream to or from another process — the same
// primitive as the host's, surfaced ergonomically. Read suspends until a chunk
// arrives; ok is false at end-of-stream. Write applies natural back-pressure: it
// suspends while the reader's buffer is full and reports false once the reader is gone.
type Stream struct {
	handle actor.StreamID
}

// OpenStream opens a stream to a pid; ok is false if the target is already gone. The
// read end is delivered to the target (via AcceptStream); the write end is this value.
func OpenStream(to Pid) (s Stream, ok bool) {
	o := actor.StreamOpen(actor.Pid(to))
	if o.None() {
		return Stream{}, false
	}
	return Stream{handle: *o.Some()}, true
}

// AcceptStream blocks until an incoming stream arrives and takes it for reading.
func AcceptStream() Stream { return Stream{handle: actor.StreamAccept()} }

// Write sends one chunk, suspending under back-pressure; false once the reader is gone.
func (s Stream) Write(chunk []byte) bool { return actor.StreamWrite(s.handle, cm.ToList(chunk)) }

// Read returns the next chunk; ok is false at end-of-stream (the writer closed/dropped).
func (s Stream) Read() (chunk []byte, ok bool) {
	o := actor.StreamRead(s.handle)
	if o.None() {
		return nil, false
	}
	return o.Some().Slice(), true
}

// Close closes the write end, signalling end-of-stream to the reader.
func (s Stream) Close() { actor.StreamClose(s.handle) }
