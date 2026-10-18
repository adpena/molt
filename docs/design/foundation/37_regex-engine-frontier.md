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

The instruction set gives alternatives explicit ordered successors, records independent raw group
start/end marks, carries effective flags on predicates and assertions, and
represents conditional groups and backreferences directly. Captures remain indexed
by parsed group identity. Conditionals and backreferences validate the current
start against the previous end mark; opening a repeated group must not expose
a stale completed span. Invalid completed mark order is an internal error. Scoped flags affect compilation of their subtree; they
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
would produce false no-match answers. Search reuses one workspace across starts. Modes are a closed type inside the
engine; only the public boundary parses their textual protocol, and invalid modes
raise ValueError. Bounds clamp independently to physical length. CPython permits
an empty anchored match beyond endpos, while search/fullmatch/iteration fail.
The scanner owns its compiled pattern and commits cursor updates after execution;
its old post-exhaustion rescan is deleted.

Lookaround has an explicit frame with its original position, capture checkpoint,
and alternative-stack barrier. A successful positive assertion retains captures
and discards its internal alternatives. A negative assertion restores its captures.
Lookbehind requires a fixed width and acceptance at its original position; nested
lookahead still sees the original subject end. Shortening the visible subject to
the lookbehind end would change nested assertion semantics.

Counted repeats use a compact counter and optional-iteration start, not an
instruction expansion proportional to the numeric bound. Mandatory iterations
never update the optional progress guard. After an empty mandatory iteration, the
first optional iteration is still eligible; its own empty completion stops a
second optional iteration at that position. Generic repeat alternatives
and capture state participate in the same undo protocol as ordinary branches.

A structural fixed-width repeat plan handles nonbranching positive-width atoms,
concatenations, scoped flags and nested capture groups. It scans repetitions once
and keeps one retry cursor. Capture recipes recover the selected last iteration's
spans and closing order when the continuation retries a shorter/longer count. This
preserves simple-repeat throughput without one choice or capture copy per input
unit. It is determined by program structure, not pattern spelling or a builtin name.
Lazy repeats consume additional input only when their continuation demands it.

The parser owns VERBOSE lexical state for both text and bytes. It skips only
Python's six ASCII verbose whitespace characters and comments between tokens;
compound group headers, numeric references, braced counts and lazy/possessive
suffixes consume adjacent raw units. Scoped flag changes apply while parsing
their body and restore the enclosing lexical state afterwards. Incomplete braced
quantifiers remain literal source, and omitted lower bounds are admitted.
The former whole-pattern verbose rewrite and its intrinsic, fixture, exports
and generated ABI rows are deleted. Rewriting first destroyed token adjacency,
disabled scoped whitespace, changed source positions and copied the pattern.
Original source now crosses the compile boundary once.

Atomic groups and possessive repeats share the assertion engine's typed scope
stack and choice barriers. Atomic success discards every alternative created
inside the group. A possessive repeat commits each successful body iteration,
including mandatory iterations; a later failed mandatory iteration cannot retry
an earlier one. A failed optional iteration restores its partial captures and
continues with the preceding committed result. These are distinct protocols:
`(?:a|ab){2}+` fails on `abab`, while `(?>(?:a|ab){2})` matches `aba` in CPython.
The parser's closed repeat mode distinguishes greedy, lazy and possessive
execution. Fixed-width possessive plans scan without a retry cursor.

After a possessive iteration or atomic group commits, the VM compacts its undo
interval to the first old value per touched slot. All surviving outer choice and
scope checkpoints precede that interval. Capture values and closing order stay
live; an outer failure can still roll the entire scope back. Possessive history
therefore scales with touched capture/repeat slots and live body backtracking,
not completed iteration count. Compaction scans that bounded journal and lazily
allocates one epoch array indexed by the existing slots; patterns that never
compact allocate none. This does not bound an individual body search or generic
backtracking elsewhere in the pattern, or shrink the peak Vec capacity needed
by such a search. The shared inline scope stores the largest possessive state;
assertion-frame bytes and throughput remain part of target performance acceptance.

## Python code-point storage

Pattern parsing, literal and class operands, fixed-width analysis, direct
matching and scanner indexes use Python code points (`u32`), including lone and
adjacent surrogates. Runtime-core's `text_storage::code_points` decodes admitted
surrogatepass storage once; no Rust `String`, lossy UTF-8 conversion or surrogate
pair normalization sits on that boundary. A supplementary scalar occupies one
position; an adjacent high/low surrogate pair occupies two. Byte patterns map
raw bytes directly to units and byte subjects remain borrowed. Effective flags
and the compiled pattern's text/byte identity still determine semantics.

The parser transfers owned literal/class buffers to the typed program. It no
longer serializes units as strings for the compiler to decode and copy again.
Single-unit predicates retain their allocation-free inline operand, including
inside structural repeat plans. Text preparation reserves fallibly from the byte
upper bound; each prepared unit has the same four-byte size as the former Rust
`char`. Direct execution prepares once, and a scanner retains one index for its
lifetime. Split, substitution, expansion and escape preserve the same raw units;
substitution appends spans without allocating temporary segment strings.
Validated group identifiers, exception messages and the ASCII mode protocol
retain their strict UTF-8 boundaries. The two replaced owned-String FFI bridges
and their generated test stubs are deleted.

This follows CPython's code-point length and offset contract in
[`sre.c` getstring/state_init](https://github.com/python/cpython/blob/v3.12.15/Modules/_sre/sre.c)
and [PEP 393](https://peps.python.org/pep-0393/), while reusing Molt's existing
storage owner rather than copying CPython's physical Unicode object layout.
Representation correctness does not qualify Rust-derived category/case tables,
locale behavior, parser escape admission or all target/version/profile cells.
The existing CPython differential carrier owns the raw-unit identity, offsets,
scanning and replacement regressions; the release board owns qualification.

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
undo storage grow with live backtracking history; their allocation failure propagates as
`MemoryError` through direct execution, iteration, split and substitution. A depth
ceiling or allocation refusal must never return no-match. The matcher itself has
no recursive execution calls. Parser/compiler recursion is a separate admission
and failure obligation. This is a VM-workspace guarantee: parser vectors and some split/substitution, group and replacement
buffers still allocate infallibly. Full operation
OOM handling remains required. The VM currently has no signal safepoint; adding
CPython-compatible interruption must release borrowed byte views and scanner locks
before handlers run, then resume the same execution if a handler returns. It
cannot be implemented by restarting matching or imposing a silent step budget.

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
  continuation/capture family. The canonical `re_parity.py` program groups matching,
  flags, captures, scanning, substitution, imports and buffer custody by contract.
  It retains the 27 former carriers' scenario inputs and 200 assertions, with
  byte-exact CPython stdout after grouping. Observable subject classes stay at module
  scope, and raw multiline patterns retain their exact token bytes. The suspicious
  nested-set warning retains its category, message and compile operation; its source
  filename and line follow the consolidated file. Broader stderr/source-location
  conformance remains an acceptance obligation.

  One executable per target owns this semantic family; storage and codec consumers
  keep their distinct programs. This cuts the owning cohort from 58 to six product
  builds without deleting scenarios or adding a test dispatcher. Future regex
  carriers still join through the canonical filename selector. Full CPython regex
  corpus and adversarial grammar-generated cases remain broader acceptance.
* Provide bounded regular execution and bounded memory without weakening match
  priority or captures. Thompson/Pike execution, bounded backtracking and prefilters
  require separate semantic proofs over the same program. Nullable-loop captures
  distinguish states with the same program/input position; counted loops add
  counter state. A plain Pike `(pc, position)` deduplication is unsound here.
  Selective failure memoization is a candidate that preserves match priority when
  its key includes the required loop-progress and counter state. Algorithm admission must
  be explicit at compilation. Backreferences and other nonregular behavior require
  a documented resource-failure policy; failed matching is not feature detection.
  Signal delivery, raising/nonraising handlers and scanner reentry are explicit
  acceptance cells. A second engine with an independently drifting parser/flag/capture model is not
  an acceptable shortcut.
* Qualify the canonical Python code-point pattern/subject migration across every
  required target/version/profile cell, including raw-surrogate matching,
  captured slicing, scanning and replacement consumers. The shared representation
  alone does not prove those execution cells.
* Generate version-pinned CPython Unicode category and case data, including special
  IGNORECASE equivalences, ASCII/bytes/locale rules and Python-version changes such
  as empty-subject `\B`. Ambient host Unicode tables cannot define target semantics.
* Qualify atomic-group and possessive-quantifier parsing/execution through nested
  assertions, capture rollback, iteration, substitution and split on both targets.
  Faithfully reject invalid syntax; CPython-version admission also owns bound
  limits, error locations, named groups and width analysis. Do not add non-CPython
  regex dialect extensions as an ecosystem workaround.
* Qualify pathological matching and long-input behavior, CPU and peak-memory cost,
  startup, compiled-pattern size, emitted binary size and build time. Set budgets
  from reproducible measurements, not aspirational multipliers or crate marketing.
* Consider build-time constant-pattern compilation and deduplication only after the
  semantic program and version/target identity are stable. Persisted programs need
  explicit format, endianness, version, alignment and validation contracts. A lazy
  DFA is not a ready-made serialized fully compiled DFA. Optional accelerators must
  earn their dependency, code-size and compile-time cost in each target/profile.

## Primary precedents and decisions

[CPython 3.12.15 `test_re.py`](https://github.com/python/cpython/blob/v3.12.15/Lib/test/test_re.py)
groups regex scenarios in one semantic suite. Molt follows that grouping while
using its existing differential runner and independent CPython execution. Test
compilation and linking are development costs; ordinary binaries acquire no
validation machinery. Compiled conformance and the actual build-phase timings
must qualify the grouped carrier; a reference-only preservation check cannot.

[CPython 3.12.15 `sre_lib.h`](https://github.com/python/cpython/blob/v3.12.15/Modules/_sre/sre_lib.h)
provides the reference for explicit contexts, ordered retries, mark restoration,
assertions, fullmatch/must-advance acceptance, `REPEAT_ONE`, `ATOMIC_GROUP` and
the per-body `POSSESSIVE_REPEAT` calls. Molt uses Rust-owned
state and capture recipes rather than copying C stack macros; observable behavior
must be proved independently against CPython.

[`regex-automata` PikeVM](https://github.com/rust-lang/regex/blob/master/regex-automata/src/nfa/thompson/pikevm.rs)
and its [bounded backtracker](https://github.com/rust-lang/regex/blob/master/regex-automata/src/nfa/thompson/backtrack.rs)
show prioritized Thompson states, capture workspace and explicit bounded visitation.
They establish algorithm precedents, not automatic Python compatibility. Any use of
these crates must account for target-versioned Unicode, Python captures and bounds,
bytes/locale semantics, assertions and nonregular syntax before claiming parity.

[Ruby 3.2's regex memoization](https://www.ruby-lang.org/en/news/2022/12/25/ruby-3-2-0-released/)
and [Davis et al., S&P 2021](https://www3.cs.stonybrook.edu/~dongyoon/papers/SP-21-Memoization.pdf)
provide a failure-memoization precedent and its memory trade. They do not prove a
Molt admission class or CPython nullable-loop capture equivalence. A memoized
executor must be compared with the nonmemoized semantic authority and CPython;
no unverified timeout or default step budget is introduced by this migration.

[RE2's syntax contract](https://github.com/google/re2/blob/main/doc/syntax.txt)
makes its omitted nonregular constructs explicit. Molt must preserve its declared
Python semantics rather than silently narrowing them to simplify an algorithm.
[CPython's `re` documentation](https://docs.python.org/3/library/re.html) records
public version changes; pinned source is the behavioral oracle.

This contract replaces the historical June design's speculative dependency sizes,
speed multipliers, unsupported regex dialect additions and obsolete duplicate-engine
phase plan. Future optimizations extend this authority and its tests instead of
creating a parallel roadmap or claiming an unimplemented tier has shipped.
