# Regex execution and release frontier (Design 37)

This is the owning regex architecture contract. HF-329 in
[the v1.0 board](../../agent/V1_HANDOFF_FINDINGS.md) owns release acceptance;
implementation is not evidence of full conformance, linear complexity or a target
performance result. CPython source and official language documentation define the
observable contract for each supported Python version. Runtime execution never
uses host Python. Native and WASM have the same semantic obligations.

## One compilation and execution authority

`molt-runtime-regex/src/regex` is the single implementation. The former duplicate
runtime implementation was removed; [design 21e](21e_runtime_satellite_dedup_PLAN.md)
owns that completed migration. `ReNode` is parser input to compilation, not an
executable pattern payload. Compilation consumes it into an immutable typed
instruction `Program` in `CompiledPattern`; `PatternHandle` owns that program.
There is no runtime AST interpreter or alternate compatibility matcher.

The instruction set gives alternatives explicit ordered successors, records group
opening and closing, carries effective flags on predicates and assertions, and
represents conditional groups and backreferences directly. Captures remain indexed
by parsed group identity. Scoped flags affect compilation of their subtree; they
cannot be lost when its continuation forces backtracking.

The matcher uses an explicit choice stack and a trail of changed capture/repeat
slots. Backtracking restores the exact checkpoint of an alternative, including the
last closed group. `Match.lastindex` is the closing order recorded by execution,
not the highest participating group. Both direct and scanner result tuples carry
`(start, end, group_spans, lastindex)`; the Python facade and its independent
CPython-backed fixture consume that same boundary.

Fullmatch's end constraint and scanner `must_advance` are acceptance conditions.
If the first successful path violates either, the VM retries alternatives at that
same start. Rejecting the final result after the matcher has discarded alternatives
would produce false no-match answers. Search reuses one workspace across starts.

Lookaround has an explicit frame with its original position, capture checkpoint,
and alternative-stack barrier. A successful positive assertion retains captures
and discards its internal alternatives. A negative assertion restores its captures.
Lookbehind requires a fixed width and acceptance at its original position; nested
lookahead still sees the original subject end. Shortening the visible subject to
the lookbehind end would change nested assertion semantics.

Counted repeats use a compact counter and cursor, not an instruction expansion
proportional to the numeric bound. Their zero-width progress guard permits required
iterations and terminates optional stalled iterations. Generic repeat alternatives
and capture state participate in the same undo protocol as ordinary branches.

A structural fixed-width repeat plan handles nonbranching positive-width atoms,
concatenations, scoped flags and nested capture groups. It scans repetitions once
and keeps one retry cursor. Capture recipes recover the selected last iteration's
spans and closing order when the continuation retries a shorter/longer count. This
preserves simple-repeat throughput without one choice or capture copy per input
unit. It is determined by program structure, not pattern spelling or a builtin name.
Lazy repeats consume additional input only when their continuation demands it.

## Cost and failure contract

Compilation costs scale with source/program size; numeric repetition bounds are
stored, not unrolled. Adjacent literal assembly appends into one growing buffer;
literal runs occupy one instruction rather than an instruction per character.
Single-unit literals retain a compact allocation-free predicate. The production pattern retains the program and metadata,
not both program and AST. Literal/class decoding belongs to compilation. Anchors and categories use shared
closed types from parsing through execution, including per-category negation inside
a larger class. Non-CPython POSIX-class expansion is removed; nested `[` retains
CPython literal parsing and its warning.

Execution prepares a text index once per operation, or borrows byte input. Its
base workspace scales with capture and generic repeat slots. Choice, assertion and
undo storage grow with live backtracking history; allocation failure propagates as
`MemoryError` through direct execution, iteration, split and substitution. A depth
ceiling or allocation refusal must never return no-match. The matcher itself has
no recursive execution calls. Parser/compiler recursion is a separate admission
and failure obligation.

Fixed-width repeat cursor state is independent of input length, apart from the
prepared text index and fixed capture recipes. Generic backtracking can still take
exponential time; unanchored restart can still rescan input quadratically. Removing
recursion and allocation churn alone establishes neither linear execution nor
ReDoS protection. HF-329 remains open until those obligations are qualified.

The development-only component measurement lives with the Rust regex tests and
is ignored in ordinary correctness runs. It times absent-literal search, simple
repeat and captured repeat over identical long subjects, using the same profile,
target directory and timer boundary before/after a change. It is not a release
performance threshold. Target runtime/startup, allocation/peak-memory, binary-size
and compile-cost acceptance require representative native and WASM measurements.
Benchmarking, fuzzing, CPython oracles and source-generation audits run on the
development side; emitted programs pay only for their required matching machinery.

## Remaining v1.0 obligations

* Prove CPython matching priority, captures, lastindex/lastgroup, empty-match
  scanning, assertion atomicity, replacement/split semantics and error behavior
  across text/bytes, native/WASM, supported Python versions and runtime profiles.
  Existing public differential carriers and pure Rust vectors own the first
  continuation/capture family; the complete CPython regex corpus and adversarial
  grammar-generated cases remain broader acceptance.
* Provide bounded regular execution and bounded memory without weakening match
  priority or captures. Thompson/Pike execution, bounded backtracking and prefilters
  are possible algorithms over the same semantic program. Algorithm admission must
  be explicit at compilation. Backreferences and other nonregular behavior require
  a documented resource-failure policy; failed matching is not feature detection.
  A second engine with an independently drifting parser/flag/capture model is not
  an acceptable shortcut.
* Replace Rust-scalar/lossy pattern and subject preparation with canonical Python
  code points from runtime-core text storage, including raw surrogates. Captured
  slicing already uses that authority; this does not prove matcher/parser storage.
* Generate version-pinned CPython Unicode category and case data, including special
  IGNORECASE equivalences, ASCII/bytes/locale rules and Python-version changes such
  as empty-subject `\B`. Ambient host Unicode tables cannot define target semantics.
* Complete atomic-group and possessive-quantifier parsing/execution and faithfully
  reject invalid syntax. Do not add non-CPython regex dialect extensions as an
  ecosystem workaround. CPython-version admission also owns bound limits, error
  locations, named groups and width analysis.
* Qualify pathological matching and long-input behavior, CPU and peak-memory cost,
  startup, compiled-pattern size, emitted binary size and build time. Set budgets
  from reproducible measurements, not aspirational multipliers or crate marketing.
* Consider build-time constant-pattern compilation and deduplication only after the
  semantic program and version/target identity are stable. Persisted programs need
  explicit format, endianness, version, alignment and validation contracts. A lazy
  DFA is not a ready-made serialized fully compiled DFA. Optional accelerators must
  earn their dependency, code-size and compile-time cost in each target/profile.

## Primary precedents and decisions

[CPython 3.12.15 `sre_lib.h`](https://github.com/python/cpython/blob/v3.12.15/Modules/_sre/sre_lib.h)
provides the reference for explicit contexts, ordered retries, mark restoration,
assertions, fullmatch/must-advance acceptance and `REPEAT_ONE`. Molt uses Rust-owned
state and capture recipes rather than copying C stack macros; observable behavior
must be proved independently against CPython.

[`regex-automata` PikeVM](https://github.com/rust-lang/regex/blob/master/regex-automata/src/nfa/thompson/pikevm.rs)
and its [bounded backtracker](https://github.com/rust-lang/regex/blob/master/regex-automata/src/nfa/thompson/backtrack.rs)
show prioritized Thompson states, capture workspace and explicit bounded visitation.
They establish algorithm precedents, not automatic Python compatibility. Any use of
these crates must account for target-versioned Unicode, Python captures and bounds,
bytes/locale semantics, assertions and nonregular syntax before claiming parity.

[RE2's syntax contract](https://github.com/google/re2/blob/main/doc/syntax.txt)
makes its omitted nonregular constructs explicit. Molt must preserve its declared
Python semantics rather than silently narrowing them to simplify an algorithm.
[CPython's `re` documentation](https://docs.python.org/3/library/re.html) records
public version changes; pinned source is the behavioral oracle.

This contract replaces the historical June design's speculative dependency sizes,
speed multipliers, unsupported regex dialect additions and obsolete duplicate-engine
phase plan. Future optimizations extend this authority and its tests instead of
creating a parallel roadmap or claiming an unimplemented tier has shipped.
