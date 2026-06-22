// A tiny URL shortener in Go — routed handler actions over durable kv.
// POST /shorten stores a URL under a fresh code; GET /:code redirects to it.
package main

import (
	"strconv"
	"strings"

	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	links := rusm.OpenBucket("links")
	h := web.NewHandlers()

	// POST /shorten — the body is the long URL; store it under a fresh code.
	h.Handle("shorten", func(req web.Request, _ web.Params) web.Response {
		target := strings.TrimSpace(string(req.Body))
		if target == "" {
			return web.Bytes(400, []byte("send a URL in the body\n"))
		}
		keys, _ := links.List()
		code := strconv.Itoa(len(keys) + 1) // a simple sequential code
		_ = links.Set(code, []byte(target))
		return web.Bytes(201, []byte("/"+code+"\n"))
	})

	// GET /:code — look the code up and redirect to the URL.
	h.Handle("expand", func(_ web.Request, p web.Params) web.Response {
		if url, ok, _ := links.Get(p.Get("code")); ok {
			return web.Bytes(302, nil).Header("location", string(url))
		}
		return web.Bytes(404, []byte("not found\n"))
	})

	h.Serve()
}
