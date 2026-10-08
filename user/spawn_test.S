# user/spawn_test.S — Multi-process test for KarteOS
# Spawns a child process (hello) and verifies it runs, then verifies POSIX
# fork semantics (P1.2): the child resumes after its fork() ecall with a0=0.
#
# Syscall ABI:
#   a7 = syscall number
#   a0-a5 = arguments
#   a0 = return value
#
# SYS_SPAWN = 30: spawn(prog_id, 0) → child_pid or error
# SYS_GETPID = 5: getpid() → pid
# SYS_WAITPID = 31: waitpid(pid) → exit code (native poll loop)
# SYS_FORK = 34: fork() → parent: child_pid, child: 0
# 84: wait4(pid, &status, 0, 0) → child pid, *status = code<<8

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

    # ─── P1.2: POSIX fork semantics ───
    # fork() → parent gets child pid (>0), child gets 0 and resumes right
    # after the ecall (not at the ELF entry).
    li      a7, 34          # SYS_FORK
    ecall
    bnez    a0, fork_parent

fork_child:
    # Child path (a0 == 0): announce and exit with code 7.
    li      a7, 2
    li      a0, 1
    la      a1, child_msg
    li      a2, 22
    ecall
    li      a7, 1           # SYS_EXIT
    li      a0, 7           # exit code 7
    ecall

fork_parent:
    # Parent path (a0 = child pid): must be positive.
    blt     a0, zero, fork_failed
    mv      t2, a0          # save child pid

    # wait4(child_pid, &status, 0, 0) → child pid; *status = 7 << 8.
    # Status lives on the user stack (inherited, guaranteed-mapped page)
    # to avoid any .bss mapping questions.
    addi    a1, sp, -16
    mv      a0, t2
    li      a2, 0
    li      a3, 0
    li      a7, 84          # wait4 dedicated handler
    ecall
    blt     a0, zero, fork_failed

    # Verify *status == 0x700 (WIFEXITED | WEXITSTATUS(7)).
    lw      t3, -16(sp)
    li      t4, 0x700
    bne     t3, t4, fork_failed

    # Print "FORK: pass (exit=7)"
    li      a7, 2
    li      a0, 1
    la      a1, fork_ok_msg
    li      a2, 26
    ecall
    j       parent_done

fork_failed:
    # Print "FORK: FAILED"
    li      a7, 2
    li      a0, 1
    la      a1, fork_fail_msg
    li      a2, 14
    ecall

    # Exit with error
    li      a7, 1
    li      a0, 2           # code = 2
    ecall

parent_done:
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
child_msg:
    .string "Child: fork ok (a0=0)\n"
fork_ok_msg:
    .string "FORK: pass (exit=7)\n"
fork_fail_msg:
    .string "FORK: FAILED\n"
