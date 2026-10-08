#!/bin/bash
# tools/llm/run_acceptance.sh — retry QEMU stdin-timing until llm generates.
# QEMU -nographic stdin delivery is timing-flaky; retry up to N times.
for i in 1 2 3; do
  (sleep 14; printf " llm\n"; sleep 300) | timeout 316 \
    qemu-system-riscv64 -machine virt -nographic -bios default \
    -kernel target/riscv64gc-unknown-none-elf/release/karte-os-kernel \
    -drive file=disk.img,format=raw,if=none,id=hd0 -device virtio-blk-device,drive=hd0 \
    > /tmp/llm_try$i.log 2>&1
  if grep -aq "LLM_OK" /tmp/llm_try$i.log; then
    echo "ATTEMPT $i: PASS"
    grep -a -A 1 '\$  llm' /tmp/llm_try$i.log | head -4
    exit 0
  fi
  echo "ATTEMPT $i: no LLM_OK ($(grep -acE 'LLM_START|fault' /tmp/llm_try$i.log) markers)"
done
echo "ALL ATTEMPTS FAILED"
exit 1
