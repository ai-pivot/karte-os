// Minimal DNS test program — resolves api.deepseek.com via UDP to 10.0.2.3:53
package main

import (
	"encoding/binary"
	"fmt"
	"net"
	"os"
	"time"
)

func main() {
	fmt.Println("[dns-test] Starting DNS resolution test")

	// Method 1: Use Go's net resolver
	fmt.Println("[dns-test] Attempting net.LookupHost...")
	ips, err := net.LookupHost("api.deepseek.com")
	if err != nil {
		fmt.Printf("[dns-test] LookupHost failed: %v\n", err)
	} else {
		fmt.Printf("[dns-test] LookupHost success: %v\n", ips)
		os.Exit(0)
	}

	// Method 2: Manual DNS query via UDP
	fmt.Println("[dns-test] Attempting manual UDP DNS query...")
	conn, err := net.DialTimeout("udp", "10.0.2.3:53", 5*time.Second)
	if err != nil {
		fmt.Printf("[dns-test] Dial failed: %v\n", err)
		os.Exit(1)
	}
	defer conn.Close()

	// Build minimal DNS query for api.deepseek.com
	query := buildDNSQuery("api.deepseek.com")
	fmt.Printf("[dns-test] Sending %d bytes to 10.0.2.3:53\n", len(query))

	conn.SetDeadline(time.Now().Add(5 * time.Second))
	_, err = conn.Write(query)
	if err != nil {
		fmt.Printf("[dns-test] Write failed: %v\n", err)
		os.Exit(1)
	}
	fmt.Println("[dns-test] Write succeeded!")

	buf := make([]byte, 512)
	n, err := conn.Read(buf)
	if err != nil {
		fmt.Printf("[dns-test] Read failed: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("[dns-test] Got %d bytes response!\n", n)
	fmt.Println("[dns-test] SUCCESS")
}

func buildDNSQuery(name string) []byte {
	// DNS header: ID=0x1234, flags=0x0100 (standard query, recursion desired)
	header := []byte{0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00}

	// Encode domain name
	var nameBytes []byte
	labels := splitLabels(name)
	for _, label := range labels {
		nameBytes = append(nameBytes, byte(len(label)))
		nameBytes = append(nameBytes, []byte(label)...)
	}
	nameBytes = append(nameBytes, 0) // root terminator

	// Type A (1), Class IN (1)
	suffix := []byte{0x00, 0x01, 0x00, 0x01}

	query := append(header, nameBytes...)
	query = append(query, suffix...)
	return query
}

func splitLabels(name string) []string {
	var labels []string
	start := 0
	for i := 0; i < len(name); i++ {
		if name[i] == '.' {
			labels = append(labels, name[start:i])
			start = i + 1
		}
	}
	labels = append(labels, name[start:])
	return labels
}

// Ensure binary encoding import is used
var _ = binary.BigEndian
