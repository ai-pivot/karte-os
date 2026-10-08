# Vector Extension (RVV 1.0) Context Design — M0-V

> Goal: let user programs use RISC-V V-extension instructions with correct,
> per-task vector context across traps and context switches. This is the
> hardware baseline for the M1 edge-LLM milestone (vectorized kernels).

## 1. Architectural state to preserve

| Piece | Size (VLEN=128) | Notes |
|-------|-----------------|-------|
| v0–v31 | 32 × 16 B = 512 B | vector registers, no direct SD — use `vse`/`vle` |
| vtype | 8 B | vill/vma/vta/vsew/vlmul |
| vl | 8 B | active vector length |
| vstart | 8 B | resume offset; normally 0 |
| sstatus.VS | bits 10:9 | Off=0 Initial=1 Clean=2 Dirty=3 |
| vxsat | 1 bit | saturation flag; folded into vtype save area (low risk to ignore in M0) |

## 2. Save policy: full save in TrapContext (lazy FPU-style save deferred)

**Decision (M0-V): full save.** Every user→kernel trap saves the full 536 B
vector state into the task's TrapContext; every trap_return restores it.
Rationale:
- Correctness first; the cost is 536 B of copies per trap (~1–2 % of trap cost).
- Precedent: x86_64 already does full FXSAVE/FXRSTOR (512 B) per switch.
- Lazy save (check sstatus.VS == Dirty, save only then) is the documented
  M1 optimization once vectorized kernels actually run hot.

The RISC-V spec auto-promotes VS Initial/Clean → Dirty on the first vector
instruction, so no kernel intervention is needed to *start* using vectors.
VS=Off (0) turns vector instructions into illegal-instruction traps; we
initialize user sstatus with VS=Initial so user code can use V immediately.

## 3. TrapContext layout (expansion)

Existing TrapContext = 36 usizes (x[0..32], sstatus, sepc, sscratch,
user_satp) = 288 B. **New layout appends a vector area**:

```text
offset  size   field
0       256    x[0..32]
256     8      sstatus
264     8      sepc
272     8      sscratch
280     8      user_satp
288     512    v[0..31]   (16-byte aligned start: 288 = 18×16 ✓)
800     8      vtype
808     8      vl
816     8      vstart
824     8      (reserved: vxsat)
total   832    (16-byte aligned)
```

`size_of::<TrapContext>()` = 832. Keep trap_entry.S offsets, main.rs and
sched::add_user_process in sync (same rule as the existing GOTCHA).

## 4. Save/restore sequences (trap_entry.S)

Save (only when arriving from U-mode and sstatus.VS != Off):

```asm
# a0 = TrapContext base
li   t0, 3 << 9           # ensure vector instructions are legal for save
csrs sstatus, t0          # VS = Dirty
vsetvli t0, zero, e64, m8, ta, ma
vse64.v v0,  (288)(a0)    # stores v0–v7  (LMUL=8 ⇒ 128 B per vse)
vse64.v v8,  (400)(a0)
vse64.v v16, (512)(a0)
vse64.v v24, (624)(a0)
csrr t0, vtype; sd t0, (800)(a0)
csrr t0, vl;    sd t0, (808)(a0)
csrr t0, vstart;sd t0, (816)(a0)
```

Restore (before sret to U-mode):

```asm
ld   t0, (800)(a0); csrw vtype, t0    # vtype write also resets vl
ld   t0, (808)(a0); csrw vl, t0
ld   t0, (816)(a0); csrw vstart, t0
vsetvli t0, zero, e64, m8, ta, ma
vle64.v v0,  (288)(a0)
vle64.v v8,  (400)(a0)
vle64.v v16, (512)(a0)
vle64.v v24, (624)(a0)
```

Notes:
- VS is part of sstatus, which is already saved/restored as a whole —
  the Dirty bit round-trips with the saved sstatus, no extra work.
- Kernel tasks never execute vector code in M0-V, so no kernel-side
  V state handling beyond the U-mode save/restore above.
- The `illegal_instruction` handler must NOT print (existing rule).

## 5. Testing plan

1. **Kernel round-trip test (M0-V②)**: in test_mode, set distinctive
   values in v0–v3, run the save sequence into a scratch buffer, scribble
   the registers, run the restore sequence, verify all lanes match.
2. **Cross-task isolation (M0-V③, real proof)**: two user processes each
   write a distinct pattern into v0, alternate via sleep/schedule, then
   read back and print — patterns must survive (the actual L211 test).
3. **Vector add user program (M0-V③)**: `-C target-feature=+v` program
   computing vadd over two arrays; outputs checksum.
