# user/spawn_test.S — Multi-process test for KarteOS
# Spawns a child process (hello) and verifies it runs.
#
# Syscall ABI:
#   a7 = syscall number
#   a0-a5 = arguments
#   a0 = return value
#
# SYS_SPAWN = 30: spawn(prog_id, 0) → child_pid or error
# SYS_GETPID = 5: getpid() → pid

.section .text
.globl _start

_start:
    # Print "Parent: spawning child..."
    li      a7, 2           # SYS_WRITE
    li      a0, 1           # fd = stdout
    la      a1, parent_msg
    li      a2, 25          # len
    ecall

    # Get parent PID
    li      a7, 5           # SYS_GETPID
    ecall
    mv      t0, a0          # save parent PID

    # Print parent PID
    li      a7, 2           # SYS_WRITE
    li      a0, 1
    la      a1, pid_msg
    li      a2, 12
    ecall

    # Spawn child process (prog_id=0 = hello)
    li      a7, 30          # SYS_SPAWN
    li      a0, 0           # prog_id = 0 (hello)
    li      a1, 0           # arg = 0
    ecall

    # Save child_pid for waitpid later
    mv      t1, a0          # child_pid = spawn() return

    # Check result
    blt     a0, zero, spawn_failed

    # Print "Parent: child spawned"
    li      a7, 2
    li      a0, 1
    la      a1, spawned_msg
    li      a2, 28
    ecall

    # Wait for child to exit (blocking wait)
    mv      a0, t1          # child_pid
    li      a7, 31          # SYS_WAITPID
    ecall

    # Print exit code
    mv      t1, a0
    li      a7, 2
    li      a0, 1
    la      a1, exit_msg
    li      a2, 20
    ecall

    # Print "Parent: done"
    li      a7, 2
    li      a0, 1
    la      a1, parent_done_msg
    li      a2, 18
    ecall

    # Exit
    li      a7, 1           # SYS_EXIT
    li      a0, 0           # code = 0
    ecall

spawn_failed:
    # Print "Parent: spawn failed!"
    li      a7, 2
    li      a0, 1
    la      a1, fail_msg
    li      a2, 24
    ecall

    # Exit with error
    li      a7, 1           # SYS_EXIT
    li      a0, 1           # code = 1
    ecall

    j       .

.section .rodata
parent_msg:
    .string "Parent: spawning child..."
pid_msg:
    .string "Parent pid: xx\n"
spawned_msg:
    .string "Parent: child spawned ok!\n"
parent_done_msg:
    .string "Parent: exiting now!\n"
fail_msg:
    .string "Parent: spawn failed!\n"
exit_msg:
    .string "Child exit code: xx\n"
