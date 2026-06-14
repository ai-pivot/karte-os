#!/bin/bash
set -e
cd /home/user/src/karte-os

echo "=== Installing Go 1.25.5 ==="
curl -sLo /tmp/go1.25.5.tar.gz https://go.dev/dl/go1.25.5.linux-amd64.tar.gz
sudo rm -rf /usr/local/go1.25 /usr/local/go
sudo tar -C /usr/local -xzf /tmp/go1.25.5.tar.gz
sudo mv /usr/local/go /usr/local/go1.25
rm /tmp/go1.25.5.tar.gz

echo "=== Go version ==="
/usr/local/go1.25/bin/go version

echo "=== Compiling hello-go for RISC-V ==="
cat > /tmp/hello.go << 'GOEOF'
package main
import "fmt"
func main() { fmt.Println("Hello, World!") }
GOEOF

GOOS=linux GOARCH=riscv64 /usr/local/go1.25/bin/go build \
    -ldflags="-s -w" \
    -o tools/disk_root/hello-go \
    /tmp/hello.go

echo "=== Deploying to disk ==="
tools/mkdisk.sh put tools/disk_root/hello-go

echo "=== Done ==="
echo "Run: ./scripts/boot-riscv64.sh"
echo "Then: hello-go"
