BITS 64
; fd_test.asm — test fd allocation using KarteOS syscall ABI
; KarteOS ABI via int 0x80:
;   2 = write(fd, buf, len)      args: rdi=fd, rsi=buf, rdx=len
;   1 = exit(code)               args: rdi=code
;  10 = open(path, path_len, flags)  args: rdi=path, rsi=path_len, rdx=flags
;  11 = close(fd)                args: rdi=fd

section .text
global _start
_start:
    ; write(2, "HI\n", 3)  — KarteOS syscall 2
    mov rax, 2
    mov rdi, 2
    lea rsi, [rel .hi]
    mov rdx, 3
    int 0x80

    ; open("/t1.txt", 7, O_WRONLY|O_CREAT|O_TRUNC=0x301)
    ; KarteOS syscall 10 = open(path, path_len, flags)
    mov rax, 10
    lea rdi, [rel .p1]
    mov rsi, 7         ; path_len
    mov rdx, 0x301     ; O_WRONLY(1) | O_CREAT(0x100) | O_TRUNC(0x200)
    int 0x80
    push rax           ; fd1

    ; print fd1
    add rax, 0x30
    lea rsi, [rel .buf]
    mov [rsi], al
    mov rax, 2
    mov rdi, 2
    mov rdx, 2
    int 0x80

    ; open("/t2.txt", 7, 0x301)
    mov rax, 10
    lea rdi, [rel .p2]
    mov rsi, 7
    mov rdx, 0x301
    int 0x80
    push rax           ; fd2

    ; print fd2
    add rax, 0x30
    lea rsi, [rel .buf]
    mov [rsi], al
    mov rax, 2
    mov rdi, 2
    mov rdx, 2
    int 0x80

    ; open("/t3.txt", 7, 0x301) — no close, should be fd=5
    mov rax, 10
    lea rdi, [rel .p3]
    mov rsi, 7
    mov rdx, 0x301
    int 0x80
    push rax

    add rax, 0x30
    lea rsi, [rel .buf]
    mov [rsi], al
    mov rax, 2
    mov rdi, 2
    mov rdx, 2
    int 0x80

    ; close fd1
    pop rax            ; fd3
    pop rbx            ; fd2
    pop rcx            ; fd1
    mov rax, 11        ; close
    mov rdi, rcx
    int 0x80

    ; open("/t4.txt", 7, 0x301) — should reuse lowest free fd
    mov rax, 10
    lea rdi, [rel .p4]
    mov rsi, 7
    mov rdx, 0x301
    int 0x80

    add rax, 0x30
    lea rsi, [rel .buf]
    mov [rsi], al
    mov rax, 2
    mov rdi, 2
    mov rdx, 2
    int 0x80

    ; print OK
    mov rax, 2
    mov rdi, 2
    lea rsi, [rel .ok]
    mov rdx, 3
    int 0x80

    ; exit(0) — KarteOS syscall 1
    mov rax, 1
    xor rdi, rdi
    int 0x80

.hi: db "HI", 10
.p1: db "/t1.txt", 0
.p2: db "/t2.txt", 0
.p3: db "/t3.txt", 0
.p4: db "/t4.txt", 0
.ok: db "OK", 10

section .data progbits write align=16
.buf: db "? ", 10
