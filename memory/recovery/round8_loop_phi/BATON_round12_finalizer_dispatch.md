# Round-12 baton: native drop flip blocked on EXACTLY ONE class — DecRef-without-finalizer-dispatch

## Verdict of the round-11 flip session (2026-06-06, the quiet-window sweep the
## round-11 baton demanded — now COMPLETE)

The full `tests/differential/basic` sweep (818 tests at base `ddb3c4329`,
flip temp-wired) ran to completion across three daemon incarnations plus a
597-test resume. Combined with a 37-test dormant-rebuild adjudication
(single-variable: same base, stash the one-line flip, rebuild, rerun), the
final census is:

| class | count | flip-relevant? |
|---|---|---|
| PASS | 684 | — |
| expectations-tracked known-bad (fail on dormant too) | 95 seen (of 146 native dims; rest live in other lanes/moved files) | NO |
| NEW-pre-existing, untracked (fail dormant AND flipped) | 35 | NO — task #78 (suite-honesty at scale; async_*×13 + file_*×8 + pep562-suspect attr/hash ×4 + misc) |
| **DROPS-CAUSED (pass dormant, fail flipped)** | **2** | **YES — this baton** |
| skipped | 2 | no |

Keystone confirmations that REMAIN green on flipped native:
* `bench_counter_words` = **97360** (round-8 loop-phi class dead)
* `loop_while_true_break_drops` byte-identical (round-10 polarity class dead,
  fix on main as `4f6c0a07e`)
* the memory corpus (RSS-bounded lane) was green in earlier rounds and the
  resume sweep added no memory-lane regressions.

## The ONE remaining class: `__del__` never fires under drop-inserted native RC

`finalizer_exit_semantics.py` and `finalizer_resurrection_once.py`:
pass dormant, fail flipped. The divergence transcripts (flipped binary,
safe_run, vs CPython 3.14):

```
finalizer_exit_semantics:   molt []                      cpython [1]
finalizer_resurrection_once:
    molt    after_first  [] 0      cpython after_first  ['del'] 1
    molt    after_second [] 0      cpython after_second ['del'] 0
```

Read: this is NOT mis-timing, NOT double-finalization, NOT resurrection
bookkeeping. **The finalizer never runs at all.** The object's memory is
released (no leak signal, RSS clean) but `__del__` dispatch is skipped.

Hypothesis (to verify first thing in round-12): the TIR-drop `DecRef`
lowering on native frees through a release path WITHOUT the
finalizer-dispatch hook, while the dormant tracked-RC release
(`drain_cleanup_tracked` / tracked release helper) goes through the
finalizer-aware path. If so the fix is structural and small: route the
drop-inserted DecRef lowering through the SAME finalizer-aware release the
tracked path uses (single release authority — no second source of truth for
"what happens at refcount zero").

**LLVM-lane probe VERDICT (run, recorded): LLVM ALSO SKIPS THE FINALIZERS.**
Both tests print the same `[]` / `after_first [] 0` divergence on LLVM with
drops live (`/tmp/claude-501/molt_dev_detached/flip_llvm_probe/run.log`).
The class is therefore **drop-pass-wide and LIVE IN PRODUCTION TODAY**: LLVM
and WASM (drops active on both since their activation) silently never run
`__del__`. Nobody noticed because the calibrated 150 contained zero
finalizer differentials. This is not merely the flip blocker — it is a
standing P0 parity hole on the shipping drop lanes, and round-12 fixes the
SHARED layer.

The good news pair:
* dormant NATIVE passes both tests → a finalizer-aware release path EXISTS
  in the runtime (the tracked-RC release helper calls it);
* the dormant adjudication proved drop PLACEMENT is correct on 816/818 —
  the bug is exclusively in what happens at refcount-zero on the drop path.

So the fix point is precise: every lowering of the TIR `DecRef` op (native
drop path, LLVM, WASM — and Luau's no-op needs an explicit justification)
must release through the ONE finalizer-aware authority the tracked path
already uses (`__del__` dispatch, resurrection-once bit, exception-in-del
swallow semantics). No second release semantics anywhere.

## What round-12 must do
1. Read the LLVM probe verdict (path above) to fix the right layer(s).
2. Find the native DecRef lowering for drop-inserted functions
   (`function_compiler.rs`, the `drop_inserted` paths) and the tracked-RC
   release helper; unify on the finalizer-aware release. NO parallel release
   semantics — one authority.
3. Extend the finalizer differential matrix while in there: `__del__` via
   del-statement, scope-exit, reassignment, cycle-free chains, resurrection
   (once-semantics), exceptions raised inside `__del__` (printed-to-stderr
   parity), `__del__` ordering vs module teardown. CPython 3.12/3.13/3.14.
4. Re-run THE FLIP PROTOCOL (this is now cheap): temp-flip, the 2 finalizer
   tests + the 37-list + keystones; then the full-gate + perf + RSS battery
   from the round-11 baton; commit the real flip; MM RUNG 1 complete; queue
   Perceus P1 (doc 27).

## Operational learnings this session (all now tool-enforced)
* Background long-runs died THREE WAYS in one day: harness-detach (exit 144,
  empty block-buffered log), sandbox teardown (reaps setsid daemons spawned
  from sandboxed calls), and a group-kill ~80 min into an unsandboxed run
  (suspected `molt clean --kill-processes` from the partner session's
  end-of-arc cleanup — molt_diff matches its worker patterns). Countermeasure
  LANDED `bc8603973`: `molt_dev.py detached-run / detached-verify` (state dir
  pid/sid/cmd.json/run.log/rc; two-step verify protocol; never-kills).
  Monitors themselves die to 144 — wakeup-polling detached-verify is the
  proven loop.
* `molt_diff --warm-cache` is currently a 2-4× SLOWDOWN: the warm phase
  builds with the `micro` profile and every hashlib/tempfile-importing
  generated test hits the #70 loud refusal (task #75). Run sweeps WITHOUT it
  until #75 lands.
* The resume pattern works: parse `[PASS]/[FAIL]` verdict lines from the dead
  run's log, `--files-from` the complement. Sweep state is reconstructable
  from logs alone.

## Artifacts (all on this host)
* flipped-sweep verdicts: `/tmp/flip_resume.json` (+ first-segment log
  `/tmp/flip_sweep_stdout.log`), failures `/tmp/flip_resume_failures.txt`
* dormant adjudication: `/tmp/flip_dormant.json`
* the 37-test list: `/tmp/flip_37.txt`
* finalizer diffs: `/tmp/claude-501/molt_dev_detached/flip_finalizer_diag/run.log`
* LLVM probe: `/tmp/claude-501/molt_dev_detached/flip_llvm_probe/run.log`
* worktree `/tmp/wt_flip` left DORMANT-SAFE (flip line reverted; no
  uncommitted changes beyond this baton).
