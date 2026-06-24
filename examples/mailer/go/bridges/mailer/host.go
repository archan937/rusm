// bridges/mailer/host.go — Go bridge host for the mailer example.
// rusm build generates _runner.go and go.mod (if absent); TinyGo compiles the whole
// bridges/mailer/ package to wasm/bridge-mailer.wasm.
// Set RESEND_API_KEY in the environment or .env before serving.
package main

import (
	"bytes"
	"encoding/json"
	"net/http"
	"os"
)

var (
	apiKey = os.Getenv("RESEND_API_KEY")
	client = &http.Client{}
)

// Send delivers the email via Resend. The generated dispatcher calls this with the WIT
// `message` record serialised into a json.RawMessage — unmarshal into your struct.
func Send(raw json.RawMessage) bool {
	var msg struct {
		To      string `json:"to"`
		Subject string `json:"subject"`
		Body    string `json:"body"`
	}
	if err := json.Unmarshal(raw, &msg); err != nil {
		return false
	}
	payload, _ := json.Marshal(map[string]string{
		"from":    "noreply@example.com",
		"to":      msg.To,
		"subject": msg.Subject,
		"html":    msg.Body,
	})
	req, _ := http.NewRequest("POST", "https://api.resend.com/emails", bytes.NewReader(payload))
	req.Header.Set("Authorization", "Bearer "+apiKey)
	req.Header.Set("Content-Type", "application/json")
	resp, err := client.Do(req)
	if err != nil {
		return false
	}
	resp.Body.Close()
	return resp.StatusCode < 300
}
