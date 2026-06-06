# Round-10 baton: DropInsertion native activation — the `loop_break_if_false`
# reachable-empty-block native-codegen blocker (Blocker B)

## Status after round-9
Round-9 LANDED the structured-CF reconstruction fix (Blocker A from the round-9
task): `lower_to_simple_ir`'s loop-region external-reentry guard was INCOMPLETE
— it checked only `[cond, header_chain, body_entry]` for external predecessors,
missing body_set members. It is now the complete **single-entry-region**
invariant: the loop HEADER is the only block allowed a predecessor outside the
region; any other in-region block with an external predecessor declines
structured reconstruction (fail-closed → generic block-by-block lowering, which
labels every block). This healed `typing._typing_strip_wrapping_parens`
(missing-label-62 dangling jump) on BOTH the native (`label_blocks[&target]`
"no entry found for key" panic) and WASM ("unknown jump label" warning /
fallthrough miscompile) lanes — they share `lower_to_simple_ir`. Landed
DORMANT-SAFE (native drops stay gated OFF); it is a pure lowering-correctness fix
beneficial on the live WASM/LLVM lanes regardless of the native flip.

Commit: <FILL IN AT COMMIT TIME — round-9 sha>.

Native drop insertion is STILL GATED OFF (`target_uses_tir_drop_insertion`
NativeCranelift => false) — the flip is blocked by Blocker B below, a SECOND
drops-caused class that round-9's bisect proved is INDEPENDENT of the round-9
lowering fix.

## The blocker (Blocker B — drops-CAUSED, native codegen, NOT lowering)
With native drops temp-wired ON, `tests/differential/memory/rc_sites_loop_break.py`
fails to build: a Cranelift codegen panic at
`cranelift-codegen-0.131.0/src/unreachable_code.rs:29:57`:
`called Option::unwrap() on a None value`. Cranelift's unreachable-code pass does
`pos.func.layout.last_inst(block).unwrap()` for every block the domtree marks
REACHABLE — so this fires when a block is reachable (has a predecessor / a jump
targets it) but EMPTY (no instructions, not even a terminator). The native
function_compiler created a block, gave it a predecessor (`reachable_blocks` +
a `jump`/`brif` into it), but never filled it.

### Repro (the failing memory test)
```python
def concat_break(limit):
    s = ""
    i = 0
    while True:
        s = s + "z"          # heap accumulator, dead-at-back-edge → drop site
        i = i + 1
        if i >= limit:
            break            # break edge; carried `s` live to post-loop use
    return len(s)
```
`while True:` + an `if …: break` — the loop exit is the break, not the header
condition (a "double-break"-flavored shape; cf. the iter_consume baton's
"cranelift unreachable_code.rs:29 panic on double-break loops" note — this is
that class, surfaced by drops).

### Bisect — Blocker B is PRE-EXISTING with drops, NOT round-9's fix
Built `rc_sites_loop_break.py` with native drops temp-wired ON under BOTH guards:
- ORIGINAL `lower_to_simple` guard (drops ON): `unreachable_code.rs:29` panic.
- ROUND-9 fixed guard (drops ON): SAME `unreachable_code.rs:29` panic.
So the lowering fix neither caused nor cures it. The lowering is LABEL-CLEAN here
(`MOLT_TIR_WARN_INVALID_LABELS=1` prints NO "label validation failed" for
`concat_break`) — confirming the dangling block is created in NATIVE CODEGEN,
downstream of TIR→SimpleIR.

`alias_reassign_slice.py` is NOT this class: it builds+runs byte-identically with
drops ON under the round-9 guard (RSS 1MiB, `79899`). Its earlier "failures" in
the `molt diff` sweep were host-memory-pressure SIGKILLs of the CLI's *backend
rebuild* (`memory_guard: SIGKILL … no RSS violation observed … host signal
source`), not correctness — re-verify any sweep failure in isolation with
`MOLT_BACKEND_DAEMON=0` before attributing it to drops.

### Localization (where to look in round-10)
- `MOLT_DEBUG_LOOP_CFG=1` on the native build prints, for `concat_break`:
  `LOOP_CFG rc_sites_loop_break__concat_break op42 loop_break_if_false
   loop=block9 body=block10 after=block11`, then the panic. So the structured
  `loop_break_if_false` path (the `while True:`/`if break` exit) is the codegen
  shape that leaves a reachable-empty block.
- Lowering (`MOLT_DEBUG_LOWER_FUNC=rc_sites_loop_break__concat_break`):
  `LOWER_DEBUG_REGION bid=BlockId(2) cond_bid=BlockId(6) break_kind=BreakIfFalse
   body_entry=BlockId(9) exit_block=BlockId(7)` — the loop IS reconstructed
  structurally (single-entry, so round-9's guard correctly does NOT decline it),
  so the structured `loop_start`/`loop_break_if_false`/`loop_end` op stream is
  what native codegen consumes.
- Native handler: `function_compiler.rs` `"loop_break_if_false"` /
  `"loop_break"` / `"loop_end"` arms (~19500–20710). The drop phase inserts
  `DecRef` at the break boundary (the carried `s` released on the loop-exit
  edge); the suspect is the `after_block` (or a break-cleanup block) being marked
  `reachable_blocks.insert(after_block)` + `jump_block(after_block)` but left
  unfilled when the break-edge cleanup re-routes block-fill state
  (`is_block_filled` / `switch_to_block_with_rebind`). Diff the block-fill
  sequence drops-ON vs dormant on this op stream to find the block that gets a
  predecessor but no terminator.

### What round-10 must do (structural, per CLAUDE.md)
1. Root-cause WHICH block is left reachable-but-empty in the native
   `loop_break_if_false`+drops codegen (cranelift `last_inst==None`). Likely the
   loop after_block / a break-cleanup block whose fill is skipped when a DecRef
   is emitted on the break edge.
2. Fix the native codegen so every block it marks reachable is filled with at
   least a terminator (or is not marked reachable). This is a bug-CLASS fix, not
   a per-shape guard: any structured loop whose break edge carries a drop must
   close its blocks. Mirror to any sibling `loop_break_if_true` /
   `loop_break_if_exception` arm that shares the pattern.
3. Add a native end-to-end regression (the `concat_break` shape) and re-run the
   round-9 flip protocol (below).
4. Re-run the round-9 lowering fix's tests too (they must stay green): unit
   `loop_shared_preheader_latch_body_keeps_label_no_dangling`, diff
   `tests/differential/basic/loop_shared_preheader_latch_drops.py`.

## THE FLIP PROTOCOL (unchanged from rounds 6-9 — only flip when ZERO drops-caused)
temp-wire `target_uses_tir_drop_insertion` NativeCranelift => true →
- serial sweep `tests/differential/basic` + `tests/differential/memory` vs the
  calibration oracle (`python3 tests/molt_diff.py <dir> --jobs 4`; isolated
  re-verification of any fail with `MOLT_BACKEND_DAEMON=0` — the harness
  rebuild gets SIGKILLed under sibling load, masking the real RC; warm the CLI
  backend with ONE `molt build` first so per-test builds don't rebuild it);
- `bench_counter_words` == 97360, `bench_calls` ≈ dormant (~0.21s), full perf
  table no-regression (quiet window, medians);
- RSS plateaus: concat ≈9MiB, list(range) ≈17-24MiB, bigint ≈8MiB;
- ZERO drops-caused → FLIP NativeCranelift => true + full gates (cargo test both
  feature sets + runtime, clippy both forms, MOLT_VERIFY_ANALYSIS=1, design-20
  set byte-identical ×3 lanes, compliance serial, honesty guard + ratchet
  removals). Evidence tables in the commit body.
- If NEW drops-caused classes remain: STOP, triage precisely (round-11), land
  any reconstruction/codegen fix DORMANT-SAFE.

## A SEPARATE pre-existing WASM repr class (Blocker C — NOT native, NOT round-9)
The heavy-module WASM stress (`import typing/re/collections/json/dataclasses/
functools/itertools/textwrap`) fails WASM **structural validation** before
linking: `func 2398 failed to validate: type mismatch: expected i64 but nothing
on stack`. BISECTED: it fails IDENTICALLY on origin/main (c2dad9e89) with the
round-9 lowering fix REVERTED, so it is independent of round-9 and of the native
drop flip. This is another instance of the round-8 loop-phi-repr WASM class
(round-8 `c2dad9e89` fixed the `os._seconds_float_to_sec_nsec` instance; the
round-8 baton flagged "likely the first of several" — `func 2398` in the
collections/json/dataclasses surface is another). It blocks the WASM lane for
these modules regardless of the native work; it is a REPR/stack class
(LIR i64/DynBox), structurally distinct from Blocker B's native reachable-empty
block and from round-9's label class. Fix it in the WASM LIR repr derivation
(`lower_to_lir` / `repr_by_value` → `LirRepr`), not in CFG reconstruction.
Single-module WASM (`repro_typing`) is clean, so it is specific to a
function in the heavy-module set.

## Verified green at round-9 (with native drops dormant) — do not re-litigate
- WASM lane (drops ON): `repro_typing` + the round-9 differential regression
  build & run byte-identical to CPython via `node wasm/run_wasm.js`; no
  "unknown jump label" / "invalid labels" warnings — the round-9 label fix
  HEALED the WASM label class. (The heavy-module WASM stress hits the SEPARATE
  pre-existing repr Blocker C above, not a label issue.)
- Native with drops ON (round-9 fixed guard): `_typing_strip_wrapping_parens`
  repro, the round-9 diff regression, the heavy-module stress, and
  `alias_reassign_slice` ALL build label-clean + byte-identical. Native memory
  set with drops ON: 13/14 (only `rc_sites_loop_break` = Blocker B).
- DORMANT (production native, round-9 fixed guard): full `molt-backend` lib
  1023/0; `lower_to_simple` 54/0 (incl. the new
  `loop_shared_preheader_latch_body_keeps_label_no_dangling`); clippy
  `-D warnings` native + native+llvm clean; design-20 memory set 14/14
  byte-identical; the round-9 diff regression byte-identical. The round-9 fix is
  landed dormant-safe (native flip stays OFF).
- HOST hazard: load ran 9-42 with sibling agents; the CLI backend rebuild
  was SIGKILLed repeatedly under load (`memory_guard: SIGKILL … no RSS violation
  observed`). Warm the cache once in a quiet window; re-verify any sweep fail in
  isolation with `MOLT_BACKEND_DAEMON=0`. A prior round-9 incarnation died to
  server-side rate limiting and a recovery automation committed its staged work
  verbatim as a WIP, then rebased it onto origin/main — that is where the
  detached worktree's HEAD WIP commit + the `*latch*`/`*.wat` stray files came
  from (not a concurrent partner).
