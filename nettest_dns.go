// nettest_dns.go — DNS resolution helper
package main

import (
	"net"
)

func netLookupHost(host string) ([]string, error) {
	return net.LookupHost(host)
}
