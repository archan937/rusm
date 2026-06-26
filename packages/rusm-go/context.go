package rusm

// The **host-only claims context** of the current bridge call, for a Go bridge's host code.

// bridgeContext holds the claims the Rust delegation shim forwarded for the call currently
// being dispatched. The generated runner installs it (SetContext) before each call; ordinary
// guest code never receives a forwarded context, so it stays nil → Context() returns empty.
var bridgeContext map[string]string

// Context returns the host-only claims context of the current bridge call — the tenant identity
// an auth hook established for the request (e.g. Context()["app_id"]), forwarded host-side to
// your bridge's host.go. Read it to make a bridge multi-tenant (act for client X vs Y) with no
// cooperation from the guest. Returns an empty map in ordinary guest code, which has no host
// context by design — so guest application code never learns the tenant.
func Context() map[string]string {
	if bridgeContext == nil {
		return map[string]string{}
	}
	return bridgeContext
}

// SetContext installs the forwarded claims context for the call about to be dispatched. The
// generated bridge runner calls it before each dispatch; application code must not.
func SetContext(c map[string]string) { bridgeContext = c }
