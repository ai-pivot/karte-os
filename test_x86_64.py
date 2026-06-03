#!/usr/bin/env python3
"""One-shot QEMU x86_64 interactive test using pty."""
import subprocess, time, sys, os, select, pty

cmd = sys.argv[1] if len(sys.argv) > 1 else "help"
timeout_secs = int(sys.argv[2]) if len(sys.argv) > 2 else 25

# Create a pseudo-terminal pair
master_fd, slave_fd = pty.openpty()
slave_name = os.ttyname(slave_fd)

proc = subprocess.Popen(
    ['qemu-system-x86_64', '-machine', 'pc', '-cpu', 'qemu64', '-m', '512M',
     '-cdrom', 'target/karte-os-x86_64.iso',
     '-drive', 'file=disk.img,format=raw,if=none,id=hd0',
     '-device', 'ich9-ahci,id=ahci',
     '-device', 'ide-hd,drive=hd0,bus=ahci.0',
     '-serial', slave_name,
     '-display', 'none', '-monitor', 'none', '-no-reboot'],
    stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
)

os.close(slave_fd)  # Close slave side, QEMU has it

output = b""
start = time.time()
cmd_sent = False

while time.time() - start < timeout_secs:
    r, _, _ = select.select([master_fd], [], [], 0.1)
    if r:
        try:
            chunk = os.read(master_fd, 8192)
            if not chunk:
                break
            output += chunk
            sys.stdout.buffer.write(chunk)
            sys.stdout.buffer.flush()
        except OSError:
            break
    
    # Look for shell prompt
    if b"$ " in output and not cmd_sent:
        time.sleep(0.5)
        os.write(master_fd, (cmd + "\r").encode())
        cmd_sent = True
        sys.stderr.write(f"\n[SENT: {cmd}]\n")
        sys.stderr.flush()
    
    # If we sent command and got another prompt, done
    if cmd_sent and output.count(b"$ ") >= 2:
        time.sleep(0.5)
        # Read remaining
        r, _, _ = select.select([master_fd], [], [], 0.5)
        if r:
            try:
                chunk = os.read(master_fd, 8192)
                output += chunk
                sys.stdout.buffer.write(chunk)
                sys.stdout.buffer.flush()
            except: pass
        break

os.close(master_fd)
proc.terminate()
try:
    proc.wait(timeout=3)
except:
    proc.kill()

print(f"\n\n=== Total: {len(output)} bytes, {time.time()-start:.1f}s ===", file=sys.stderr)
