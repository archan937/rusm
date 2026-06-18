// The todo HTTP API in Go: buffered #-action handlers (no main, no router; routes live in
// rusm.toml's [serve.routes]). Each request runs in a fresh, isolated instance. It reads/
// writes the durable todo list, publishes every change to the feed's subscribers, and
// serves the explanatory web page at GET /. The data layer lives in the shared todos package.
package main

import (
	_ "embed"
	"encoding/json"
	"log/slog"
	"strconv"
	"strings"

	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
	"todoboard/todos"
)

//go:embed page.html
var page []byte

func init() { rusm.Run(run) }
func main() {}

// cors adds the headers every response carries (so a browser app on another origin works).
func cors(resp web.Response) web.Response {
	return resp.
		Header("access-control-allow-origin", "*").
		Header("access-control-allow-methods", "GET, POST, PATCH, DELETE, OPTIONS").
		Header("access-control-allow-headers", "content-type")
}

func jsonResp(status int, v any) web.Response {
	body, err := json.Marshal(v)
	if err != nil {
		return cors(web.Bytes(500, []byte(`{"error":"encode"}`)))
	}
	return cors(web.Bytes(status, body).Header("content-type", "application/json"))
}

func errResp(status int, message string) web.Response {
	return jsonResp(status, map[string]string{"error": message})
}

func idParam(p web.Params) (uint64, bool) {
	id, err := strconv.ParseUint(p.Get("id"), 10, 64)
	return id, err == nil
}

func run() {
	h := web.NewHandlers()

	h.Handle("home", func(_ web.Request, _ web.Params) web.Response {
		return web.Bytes(200, page).Header("content-type", "text/html; charset=utf-8")
	})

	h.Handle("list", func(_ web.Request, _ web.Params) web.Response {
		return jsonResp(200, todos.List())
	})

	h.Handle("create", func(req web.Request, _ web.Params) web.Response {
		var body struct {
			Text string `json:"text"`
		}
		if json.Unmarshal(req.Body, &body) != nil {
			return errResp(400, "invalid body")
		}
		text := strings.TrimSpace(body.Text)
		if text == "" {
			return errResp(400, "text is required")
		}
		todo := todos.Create(text)
		slog.Info("api: created", "id", todo.ID, "text", todo.Text)
		return jsonResp(201, todo)
	})

	h.Handle("toggle", func(_ web.Request, p web.Params) web.Response {
		id, ok := idParam(p)
		if !ok {
			return errResp(400, "bad id")
		}
		todo := todos.SetDone(id)
		if todo == nil {
			return errResp(404, "no such todo")
		}
		slog.Info("api: toggled", "id", id, "done", todo.Done)
		return jsonResp(200, todo)
	})

	h.Handle("remove", func(_ web.Request, p web.Params) web.Response {
		id, ok := idParam(p)
		if !ok {
			return errResp(400, "bad id")
		}
		if !todos.Delete(id) {
			return errResp(404, "no such todo")
		}
		slog.Info("api: deleted", "id", id)
		return cors(web.Bytes(204, nil))
	})

	h.Handle("preflight", func(_ web.Request, _ web.Params) web.Response {
		return cors(web.Bytes(204, nil))
	})

	h.Serve()
}
