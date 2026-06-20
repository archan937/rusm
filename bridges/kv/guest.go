// Canonical source: bridges/kv/guest.go — the kv bridge's Go guest binding.
// Synced into rusm-go (packages/rusm-go/kv.go) by `make sync-bridges`; edit this
// file, not the copy. The `bridge_guest_in_sync` test fails the build on drift.

package rusm

import (
	"errors"

	"go.bytecodealliance.org/cm"

	"github.com/archan937/rusm/packages/rusm-go/internal/wit/rusm/runtime/kv"
)

// Bucket is a handle to one namespace of durable key-value storage — bytes in, bytes
// out, backed by the node's embedded store (no external daemon). Gated by the storage
// capability (default-deny): each op returns an error if storage is denied or
// unconfigured. The host application owns the store's file; a guest names buckets + keys.
type Bucket struct {
	name string
}

// OpenBucket returns a handle to the named storage bucket.
func OpenBucket(name string) Bucket { return Bucket{name: name} }

// Get returns the value for key; ok is false if the key is absent.
func (b Bucket) Get(key string) (value []byte, ok bool, err error) {
	r := kv.Get(b.name, key)
	if r.IsErr() {
		return nil, false, errors.New(*r.Err())
	}
	o := r.OK()
	if o.None() {
		return nil, false, nil
	}
	return o.Some().Slice(), true, nil
}

// Set stores value under key.
func (b Bucket) Set(key string, value []byte) error {
	r := kv.Set(b.name, key, cm.ToList(value))
	if r.IsErr() {
		return errors.New(*r.Err())
	}
	return nil
}

// Delete removes key; deleted is false if it was absent.
func (b Bucket) Delete(key string) (deleted bool, err error) {
	r := kv.Delete(b.name, key)
	if r.IsErr() {
		return false, errors.New(*r.Err())
	}
	return *r.OK(), nil
}

// Exists reports whether key is present.
func (b Bucket) Exists(key string) (bool, error) {
	r := kv.Exists(b.name, key)
	if r.IsErr() {
		return false, errors.New(*r.Err())
	}
	return *r.OK(), nil
}

// List returns every key in the bucket.
func (b Bucket) List() ([]string, error) {
	r := kv.List(b.name)
	if r.IsErr() {
		return nil, errors.New(*r.Err())
	}
	return r.OK().Slice(), nil
}
