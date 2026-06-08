package main

import (
	"fmt"
	"os"
	"runtime"
	"sync"
	"time"
)

func main() {
	fmt.Fprintf(os.Stderr, "[minclone] GOMAXPROCS=%d NumCPU=%d\n", runtime.GOMAXPROCS(0), runtime.NumCPU())
	fmt.Fprintf(os.Stderr, "[minclone] start\n")

	// Test 1: basic goroutine
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		fmt.Fprintf(os.Stderr, "[minclone] goroutine 1 running on tid=%d\n", syscallGettid())
		wg.Done()
	}()
	wg.Wait()
	fmt.Fprintf(os.Stderr, "[minclone] goroutine 1 done\n")

	// Test 2: multiple goroutines
	for i := 0; i < 4; i++ {
		wg.Add(1)
		go func(id int) {
			fmt.Fprintf(os.Stderr, "[minclone] goroutine %d running\n", id)
			wg.Done()
		}(i)
	}
	wg.Wait()
	fmt.Fprintf(os.Stderr, "[minclone] all goroutines done\n")

	// Test 3: write to stdout
	fmt.Print("HELLO FROM STDOUT\n")
	fmt.Fprintf(os.Stderr, "[minclone] wrote to stdout\n")

	// Test 4: timer
	fmt.Fprintf(os.Stderr, "[minclone] sleeping 100ms...\n")
	time.Sleep(100 * time.Millisecond)
	fmt.Fprintf(os.Stderr, "[minclone] slept 100ms\n")

	fmt.Print("DONE\n")
	fmt.Fprintf(os.Stderr, "[minclone] exit\n")
}

func syscallGettid() int {
	return os.Getpid() // approximation
}
