# HF-685 slice 2: construction instruction attribution

## Phase 1: measurement prerequisite (2026-11-04)

The assigned outcome is an exact per-function construction anatomy followed by a
structural correction selected from that evidence. No runtime/compiler correction
is selected yet. Slice 1's five-pair macOS arm64 medians are 16,663 instructions
per plain construction, 28,048 per new-and-update, and 29,010 per dataclass
construction; its noisy spreads do not establish a speedup. Its sampled anatomy
left 41% of plain self samples in other runtime. These are historical diagnostic
measurements, not Linux observations or instruction attribution.

Live source ca1cdefc68c1 already has sealed class layouts, lazy instance dictionaries
and one scoped physical-field traversal (object/class_layout.rs and
object/field_storage.rs; public authority docs/design/foundation/49_object_field_ownership.md).
Do not replace these with another layout, cache, registry or dictionary policy.
The source and slice-1 report were checked before writing this design.

Live molt-gate is x86_64 Linux on the inventoried WSL distribution. Its checkout
is clean and behind origin/main by nine commits; it is a donor, never reset.
The probe found perf but no valgrind. The only phase-1 tracked source change is
adding valgrind to tools/trunk_loop.py:LINUX_SYSTEM_PACKAGES. This feeds the
existing WSL_GUEST_PACKAGES projection and approved missing-package installer.
No installer, host registry, privilege bypass or new proof cell is introduced.
The inventory already carries install_system_packages=true, owner-approved
2026-11-04. Run the canonical provision command after review; retain its receipt
and exact valgrind version. Existing tests/tools/test_trunk_provision.py exercises
the package projection and missing-only install boundary. Run that authority.

## Attribution protocol and interpretation

Use exact source ca1cdefc68c1 in an owned remote scratch checkout/build, without
moving the donor checkout. Record Git identity, compiler/runtime profile, toolchain,
program hashes and executable hashes. Reuse canonical programs from
tools/perf_scoreboard_primitives.py for plain_construction, new_and_update and
dataclass_construction; retain their stdout and compare with CPython 3.12.16.
Collect Callgrind Ir at N=0 and N=K, with matching binary and environment. Normalize
each function's signed difference by actual operations (new_and_update uses N/10).
Use sufficient K to dominate startup, retaining negative differences rather than
clamping them. Collect both exclusive costs and inclusive call-edge costs and
dynamic calls. Preserve unmapped symbols; they cannot support a guessed mechanism.
Inline functions belong to their machine-code owner unless debug attribution
proves otherwise. Inclusive rows overlap and must never be summed as mechanisms.
Check self-cost totals against the profile event summary; inspect recursive edges
before interpreting inclusive totals. Rank the top 15 by exclusive delta, showing
their inclusive delta beside it, plus a separate inclusive ranking as needed to
identify expensive call chains. Partition exclusive costs by actual mechanism:
allocation, binding, init dispatch, attribute store, dict materialization,
refcount, GC tracking, hashing, runtime entry and guards. Preserve loop/control
and unclassified costs rather than forcing them into a runtime category.

Callgrind counts modeled instruction reads, not hardware retired instructions.
It is the requested attribution oracle, not a replacement for the separate
five-pair macOS hardware-counter A/B. Do not compare its ratios with a hardware
ratchet. The Valgrind manual describes exclusive and inclusive costs and warns
that callgrind_annotate does not detect recursive cycles:
https://valgrind.org/docs/manual/cl-manual.html . Its format authority defines
cost rows, compressed identities and inclusive call-edge rows:
https://valgrind.org/docs/manual/cl-format.html . No upstream code is copied.

## Phase 2 gate

Before structural implementation, append the actual three attribution tables,
source-grounded dominant mechanism, chosen correction, invariants, affected
producer/consumer/backend cells, alternatives and independent semantic witnesses.
Have two sequential fresh read-only Codex reviewers read only this file, resolve
their findings here, and commit the reviewed design before structural code.
If dominance belongs to HF-205, HF-198, HF-226 or HF-79, retain that ownership and
limit implementation to the measured HF-685 portion. An unsettled semantic
decision returns to the parent. Do not claim attribution or a fix before evidence.

Required final proof: construction semantics against CPython 3.12 on native,
LLVM and WASM, covering missing init, overridden new, slots, dict identity/update,
vars, pickle and subclasses; five paired base/head hardware instruction slopes
against exact CPython 3.12.16 on all three primitives; head Callgrind tables;
changed tests/direct readers, affected crate checks, static-only, and at most
one canonical precheck in the runner's plain environment. Parent owns landing,
ledger/ratchet and retirement. No reserved path is in this worker's write scope.

## Review record

Phase-1 reviewer: separate `codex exec -s read-only -m gpt-6.1-sol`, high,
given only this path and the implementation-readiness role. It read no other
file. Findings and resolutions (review-bootstrap.md):

1. Exact target: tools/cross_hosts.toml entry molt-gate, SSH adpena@bat00,
   WSL distribution molt-gate. Command from the assigned checkout using its
   selected-image Python: `python -B tools/trunk_loop.py provision --host molt-gate`.
2. Canonical Primitive.program() reads N from `sys.argv[1]`, after compilation.
   Capture `valgrind --tool=callgrind --callgrind-out-file=OUT BINARY 0` and
   the same command with `BINARY K`. Record N, divisor and N/divisor with each
   profile; no recompile or empty-loop subtraction.
3. Use K=10,000 actual operations initially (N=100,000 for new_and_update).
   Check the total normalized slope at 2K: relative difference must be <=1%.
   A failure invalidates the anatomy as steady-state evidence; inspect actual
   changed call paths before adjusting K. Retain all captures and signed rows.

This resolves the prerequisite review. Runtime design remains gated on attribution
and its two independent reviews; no runtime optimization is approved by this phase.
