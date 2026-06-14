#!/bin/bash
# Pre-create .xbot directory structure on disk image
DISK="${1:-disk.img}"
MOUNT="/tmp/karteos-mnt"
mkdir -p "$MOUNT"
mount -o loop "$DISK" "$MOUNT" 2>/dev/null
mkdir -p "$MOUNT/.xbot/logs" "$MOUNT/.xbot/agents" "$MOUNT/.xbot/sessions"
umount "$MOUNT" 2>/dev/null
rmdir "$MOUNT" 2>/dev/null
echo "[disk] .xbot directories created"
