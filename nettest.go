// nettest — minimal DNS + HTTP test for KarteOS
package main

import (
	"crypto/tls"
	"fmt"
	"io"
	"net/http"
	"os"
	"strings"
	"time"
)

func main() {
	fmt.Println("nettest: starting")

	// Step 1: DNS lookup
	fmt.Println("nettest: resolving api.deepseek.com...")
	ips, err := lookupDNS("api.deepseek.com")
	if err != nil {
		fmt.Printf("nettest: DNS FAILED: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("nettest: DNS OK: %v\n", ips)

	// Step 2: HTTPS request (skip TLS verify — we don't have CA certs)
	fmt.Println("nettest: HTTPS POST to api.deepseek.com...")
	body := strings.NewReader(`{"model":"deepseek-chat","messages":[{"role":"user","content":"say hi"}],"max_tokens":5}`)
	req, err := http.NewRequest("POST", "https://api.deepseek.com/chat/completions", body)
	if err != nil {
		fmt.Printf("nettest: NewRequest FAILED: %v\n", err)
		os.Exit(1)
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+os.Getenv("DEEPSEEK_API_KEY"))

	client := &http.Client{
		Timeout: 30 * time.Second,
		Transport: &http.Transport{
			TLSClientConfig: &tls.Config{InsecureSkipVerify: true},
		},
	}
	resp, err := client.Do(req)
	if err != nil {
		fmt.Printf("nettest: HTTP FAILED: %v\n", err)
		os.Exit(1)
	}
	defer resp.Body.Close()
	respBody, _ := io.ReadAll(resp.Body)
	respStr := string(respBody)
	if len(respStr) > 200 {
		respStr = respStr[:200]
	}
	fmt.Printf("nettest: HTTP OK status=%d body=%s\n", resp.StatusCode, respStr)
	fmt.Println("nettest: ALL PASSED")
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}

// DNS lookup helper
func lookupDNS(host string) ([]string, error) {
	return netLookupHost(host)
}
