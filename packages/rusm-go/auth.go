package rusm

import (
	"encoding/json"
	"strings"
)

// Serving **auth hook** types for a Go host (`auth/<name>/host.go`). An auth hook validates an
// incoming request host-side and returns claims (the tenant a multi-tenant bridge then acts
// for) or a denial. The operator writes:
//
//	func Authenticate(req rusm.AuthRequest) rusm.AuthVerdict {
//	    if ok, appID := verify(req.Header("authorization")); ok {
//	        return rusm.Allow(map[string]string{"app_id": appID})
//	    }
//	    return rusm.Deny()
//	}
//
// `rusm build` compiles this into a resident dispatch runner; a `[[serve]] authentication =
// "<name>"` listener runs it before every request (claims seed the request's host-only context,
// a denial is `401`). It is host code: guest application components never see it.

// AuthRequest is what a Go auth hook is shown about an incoming request. The host fills it
// (method/path/query/headers); the hook reads it. Field tags match the host's JSON wire.
type AuthRequest struct {
	Method  string     `json:"method"`
	Path    string     `json:"path"`
	Query   string     `json:"query"`
	Headers [][]string `json:"headers"`
}

// Header returns the first header whose name equals name (ASCII case-insensitive), or "".
func (r AuthRequest) Header(name string) string {
	for _, h := range r.Headers {
		if len(h) == 2 && strings.EqualFold(h[0], name) {
			return h[1]
		}
	}
	return ""
}

// QueryParam returns the value of query parameter name (first occurrence), or "". A browser
// can't set Authorization on a WebSocket, so the token often arrives here. Values are undecoded.
func (r AuthRequest) QueryParam(name string) string {
	for _, pair := range strings.Split(r.Query, "&") {
		if k, v, ok := strings.Cut(pair, "="); ok && k == name {
			return v
		}
	}
	return ""
}

// AuthVerdict is an auth hook's decision. Build it with Allow or Deny — never by hand. It
// marshals to the host's wire: `{"allow": {…}}` permits (seeding those claims); anything
// else — including the zero value — denies. So a hook that forgets to return, or returns the
// zero AuthVerdict, fails closed.
type AuthVerdict struct {
	allow map[string]string
}

// MarshalJSON emits `{"allow": {claims}}` for an allow, or `{}` for a deny (the host reads any
// non-`allow` shape as a denial — fail-closed).
func (v AuthVerdict) MarshalJSON() ([]byte, error) {
	if v.allow == nil {
		return []byte("{}"), nil
	}
	return json.Marshal(struct {
		Allow map[string]string `json:"allow"`
	}{Allow: v.allow})
}

// Allow authenticates the request, attaching claims (e.g. {"app_id": "acme"}) to its host-only
// context — reaching every bridge the handler's call graph hits.
func Allow(claims map[string]string) AuthVerdict {
	if claims == nil {
		claims = map[string]string{}
	}
	return AuthVerdict{allow: claims}
}

// Deny rejects the request: the serving bridge replies 401 and never spawns a handler.
func Deny() AuthVerdict { return AuthVerdict{} }
