<!--
Foundation design 37 — Regex Engine Frontier. The end-state design for molt's `re`
module engine. Architect: read-only research-granted agent, 2026-06-06. DESIGN ONLY;
no implementation landed. Doc number 37 reserved by the supervisor in the doc-29
remapping note (29 §header: "its 'Doc 32 Regex' -> slot 37"). Do not renumber.

All file:line anchors verified against the live worktree at HEAD commit
8679b065540d90a03e3253e24638da39643b42bf (branch main, 2026-06-06). The doc-29
SUBSYSTEM 6 audit and the doc-16 regex rows were written at 951938075; this doc
RE-AUDITS the engine at HEAD and flags FOUR divergences from those audits inline
(they are materially STALE — see §0.1).

Research provenance (RESEARCH GRANT, standing): the Rust `regex` crate 1.12.3 +
`regex-automata` (BurntSushi, "Regex engine internals as a library",
https://burntsushi.net/regex-internals/), `fancy-regex` 0.18.0 (MIT; hybrid
delegate-or-backtrack), Google RE2 ("a principled approach", swtch.com/~rsc/regexp/),
CPython `re` docs (docs.python.org/3/library/re.html — PSF, semantics reference only),
CPython `_sre`/`sre_compile`/`sre_parse` internals, and the Python 3.7/3.11/3.13
`re` changelog. Cited inline. License discipline: study + reimplement; PSF-licensed
CPython is a semantics oracle only; `regex`/`regex-automata`/`fancy-regex`/`memchr`/
`aho-corasick` are MIT-OR-Apache-2.0 (direct use permitted); RE2 is BSD-3 (ideas, no
code ingest needed). No GPL code ingested.
-->

# Regex Engine Frontier (Design 37)

**Document status:** Implementation-ready frontier design. **`re`-subsystem root doc.**
**Scope:** The complete `re`-module story for molt across all targets (native
macOS/Linux, WASM-browser, WASI, Luau) and all profiles. Defines the **two-tier
engine** (Tier-F = linear-time finite automata for the regex-crate-expressible
subset; Tier-B = full CPython-`sre`-semantics backtracker for the rest), the
**sound AST classifier** that routes between them, the **AOT compile-time pattern
compilation** superpower, the parity-vs-UNLEASHED boundary (§1.4, house mandate),
the semantics-fidelity divergence table with its pinning tests, the per-target
matrix, the benchmark gates, and the phased build plan with deletions.

**The one-paragraph thesis.** molt does **not** today ship "a pure-Python regex
engine" (as doc-29 SUBSYSTEM 6 and doc-16 claim). It ships a **5,708-line hand-rolled
recursive-descent parser + naïve recursive-backtracking NFA matcher in Rust**
(`runtime/molt-runtime-regex/src/regex.rs`), duplicated verbatim as a second compiled
copy in `runtime/molt-runtime/src/builtins/regex.rs`. That engine is feature-complete
on paper (backrefs, lookaround, conditionals, scoped flags, named groups, sub/split/
finditer) but is **structurally unsound in three load-bearing ways**: (1) no
linear-time guarantee — it is exponential-blowup backtracking with a `MAX_RECURSION_DEPTH
= 5000` cap that **silently returns "no match" (`None`) on deep inputs**, a silent
correctness divergence and a ReDoS vector; (2) capture-under-nested-repetition is
acknowledged-fragile in its own apology comments (`regex.rs:2383-2553`); (3)
IGNORECASE / `\w` / `\Z` use ad-hoc `char::to_lowercase()` and `is_alphabetic()`
instead of `sre`'s casefold + Unicode-word tables, producing observable divergences.
The end-state replaces it with a **Tier-F engine built on the `regex`/`regex-automata`
crate** (already at version 1.12.3 in `Cargo.lock`, zero new transitive deps for the
ML/arrow build closure) for the linear subset, and a **principled Rust port of the
`_sre` bytecode VM** as Tier-B for backrefs/lookaround/conditionals/atomic-groups —
because no off-the-shelf crate matches `sre` observable semantics (`fancy-regex`
targets Oniguruma, §3-c). A sound AST classifier picks the tier; the default tier is
**byte-identical to CPython 3.12+**; the optional UNLEASHED tier is RE2-style
guaranteed-linear (rejects backref/lookaround at compile, immune to catastrophic
backtracking). The pure-Python `re/__init__.py` wrapper is **demoted to a thin object
layer** and the two homegrown Rust engines are **deleted**.

---

## 0. Method, scores, refusals

House conventions (docs 26–33): every claim carries a `file:line` anchor verified at
HEAD `8679b0655`; gaps are scored **IMPORTANCE × GAP** on the 1–3 scale; refused
designs are stated with the reason; research provenance is inline.

### 0.1 FOUR stale claims in the prior audits (re-audited at HEAD)

The doc-29 SUBSYSTEM 6 audit (`docs/design/foundation/29_domain-critical-portfolio.md:193-221`)
and the doc-16 rows (`16_cpython-surface-stdlib-gpu-gap-audit.md:35,178,355`) are the
inputs this doc was commissioned against. Re-auditing at HEAD, four of their
load-bearing claims are **false now** and the design must not inherit them:

- **STALE-1 — "engine is entirely in Python" / "pure Python with Rust-backed literal
  and lookaround fast paths" (29:197-199).** FALSE. `re/__init__.py:10-24` imports
  `molt_re_compile`, `molt_re_execute`, `molt_re_finditer_collect`, `molt_re_split`,
  `molt_re_sub`, `molt_re_sub_callable`, `molt_re_pattern_info` and routes ALL match
  execution into Rust. The Python file is a 478-line object wrapper (`Match`,
  `Pattern`, module functions, cache) with **zero match logic**. The engine —
  parser, IR, matcher — is `regex.rs:957-3666` (Rust).

- **STALE-2 — "Lookahead, named groups, flag scoping, backreferences all raise
  NotImplementedError; host fallback disabled" (16:178,355; 29:201).** FALSE. The
  Rust parser builds `ReNode::Look`, `ReNode::Backref`, `ReNode::ScopedFlags`,
  `ReNode::Conditional`, and named-group `group_names` (`regex.rs:1016-1033`,
  `:1571-1577`, `:1422-1583`), and the matcher executes all of them
  (`regex.rs:2316-2376`, `:2834-2896`). There is **no `NotImplementedError`** site in
  `re/__init__.py`. The "host fallback disabled / SENTINEL_FALLBACK=-2" mechanism
  (29:201) refers to the **vestigial lookaround helpers** `molt_re_positive_lookahead`
  et al. (`regex.rs:223-462`) — a *dead* descriptor protocol from the pre-Rust-engine
  era that the live `molt_re_execute` path never calls. **The real gaps are different**
  (see §2): silent ReDoS truncation, capture fragility, casefold divergence, and the
  missing 3.11 atomic-group/possessive surface.

- **STALE-3 — "no NFA/DFA compiled engine, no regex crate" (29:199); "the regex crate
  is already in the Rust ecosystem … adding regex crate to molt-runtime-regex/Cargo.toml
  is the only structural dependency" (29:217-219).** Half-true and half-false. There is
  no NFA/DFA engine — correct. But the prior audit did not know that `regex` **1.12.3**,
  `regex-automata`, `regex-syntax`, `aho-corasick`, and `memchr 2.8.0` are **already in
  `Cargo.lock`** (`Cargo.lock:4369,4381,4398,35,2844`), pulled by `arrow-csv`,
  `arrow-string`, `bindgen`, and `criterion`. They are NOT yet in the lean
  `stdlib_standard` runtime closure (the satellite `molt-runtime-regex/Cargo.toml`
  depends only on `molt-runtime-core` + `molt-obj-model`), but the crate is vendored,
  version-pinned, and compiles in this workspace today — the binary-size discussion
  (§5) is therefore about *feature-trimming an already-present crate*, not adopting a
  new one.

- **STALE-4 — duplicate engine.** Neither prior audit noted that the engine exists as
  **two compiled copies**: the satellite `molt-runtime-regex/src/regex.rs` (5,708 lines,
  `with_core_gil!` + FFI bridge) and the in-tree `runtime/molt-runtime/src/builtins/regex.rs`
  (5,708 lines, `with_gil_entry_nopanic!` + direct `crate::` calls), wired at
  `builtins/mod.rs:116` (`pub(crate) mod regex;`). They are kept hand-synchronized
  under a "satellite-parity guard" (the header comment `regex.rs:2-9`). **This is
  ~5.7K lines of duplicated unsound code compiled into every binary.** Collapsing it is
  a first-order deletion target (§7).

### 0.2 Subsystem score (re-scored at HEAD)

doc-29 scored regex IMPORTANCE×GAP = 26/48 on the "any pattern with a named group or
lookahead raises NotImplementedError" premise (STALE-2). Re-scored against the *real*
state:

| Domain | Importance | Gap (HEAD) | Product | Note |
|---|---|---|---|---|
| Data science | 3 | 2 | 6 | log/data parsing works for *most* patterns; silent ReDoS truncation + casefold divergence are correctness landmines |
| Engineering | 3 | 2 | 6 | config/tokenizer parsing; the duplicate-unsound-engine debt + missing atomic groups |
| Web | 3 | 3 | 9 | **ReDoS is a security property for servers**; the silent-truncation-on-depth behavior is worse than a hang (wrong answer, no signal) — and there is no guaranteed-linear mode |
| ML | 1 | 1 | 1 | tokenizer regexes (GPT-2 BPE `'s|'t|...| ?\p{L}+|...`) need `\p{...}` Unicode classes, today unsupported |

**Weighted product: 22 / 48.** Lower raw number than doc-29's 26 because basic
matching *works*; but the **web row is now 9 (CRITICAL)** because the failure mode
flipped from "loud NotImplementedError" to "silent wrong answer + ReDoS," which is
the more dangerous class. **Verdict: FRONTIER-DESIGN-NOW** (this doc), riskiest phase
flagged in §7.

### 0.3 Refused designs (stated up front, with why)

- **REFUSED-A: "keep the homegrown backtracker, just add a recursion-budget /
  depth-bailout that raises instead of returning None."** This is the localized hack the
  top-of-CLAUDE.md policy exists to reject. The depth cap is a *symptom*; the disease is
  that the engine has no linear-time core for the linear subset, so it backtracks
  exponentially on patterns a DFA matches in O(n). Capping the budget converts silent
  wrong-answers into loud failures but leaves molt unable to match `(a|a)*b` against a
  long string at all — CPython 3.11+ matches it fine (its `sre` is also backtracking but
  with memoization + the new atomic-group optimization, and real programs lean on the
  fast linear subset). The structural fix is the two-tier engine. Rejected.

- **REFUSED-B: single-engine pure-`regex`-crate (drop backref/lookaround entirely).**
  Trades the parity contract. `re.fullmatch(r'(\w+) \1', s)` (backref), `(?<=\$)\d+`
  (lookbehind), `(?(1)a|b)` (conditional) are valid CPython 3.12 patterns in wide use
  (deduplication, financial parsing, optional-group matching). A default tier that
  raises on them is a silent-parity-break by omission. Rejected as the *default*;
  *adopted* as the opt-in UNLEASHED tier (§4), where the trade is explicit.

- **REFUSED-C: single-engine pure-backtracker, but make it RE2-quality.** "Just write a
  better backtracker." A correct, memoized, linear-on-the-linear-subset backtracker
  that *also* matches `sre` capture semantics IS essentially the `sre` VM port (Tier-B)
  — but without the DFA, it leaves 10–50× throughput on the table for the 90% of
  patterns that are backref-free. The DFA path is not optional for the perf contract
  (molt MUST beat CPython on every bench; CPython `re` ≈ 130 MB/s, `regex` crate ≈
  1–2 GB/s). Rejected: you need both engines.

- **REFUSED-D: adopt `fancy-regex` as both tiers.** `fancy-regex` 0.18.0 is MIT
  (license-clean) and its hybrid architecture (delegate easy subexpressions to `regex`,
  backtrack the hard ones) is *exactly the right shape* — but it **targets Oniguruma
  semantics, not `sre`** (its README: "Aims to be compatible with Oniguruma syntax when
  the relevant flag is set"; it makes **no** CPython byte-compatibility claim). Concrete
  `sre` divergences `fancy-regex` would import: different empty-match advance rules,
  different `$`/`\Z` newline handling, Oniguruma `\h`/`\R`/`\K` extensions absent in
  `sre`, different conditional and named-group syntaxes, and Oniguruma's capture-reset
  semantics under repetition. Shipping it under a CPython-`re` label would be exactly
  the "ship a generic engine under a fidelity label" anti-pattern the project forbids.
  **Adopted as an architecture reference** (the delegate-or-backtrack split, §3-a) and
  **studied, not vendored, for Tier-B**. Rejected as the engine.

---

## 1. The two-tier engine — the decision

### 1.1 Shape

```
                 re.compile(pattern, flags)
                            │
                 ┌──────────▼───────────┐
                 │  PARSE → sre-faithful │   one parser, one AST (HIR-M).
                 │  AST  (HIR-M)         │   Mirrors CPython _sre opcodes 1:1
                 └──────────┬───────────┘   so the classifier reasons on the
                            │               same structure sre compiles.
                 ┌──────────▼───────────┐
                 │  CLASSIFY (sound)    │   §1.3. Walks HIR-M. "Tier-F-eligible"
                 │  over the AST        │   iff NO backref, NO lookaround, NO
                 └─────┬──────────┬─────┘   conditional, NO atomic/possessive,
                       │          │         NO sre-only casefold edge.
              Tier-F   │          │  Tier-B
         (linear/DFA)  ▼          ▼  (sre VM)
   ┌────────────────────────┐  ┌──────────────────────────────┐
   │ regex-automata meta::  │  │ Rust port of the _sre         │
   │ Regex (lazy DFA +      │  │ bytecode VM: leftmost-first   │
   │ one-pass + PikeVM +    │  │ backtracking, memoized, with  │
   │ BoundedBacktracker +   │  │ a regex-crate PREFILTER for   │
   │ Teddy/memmem prefilter)│  │ the literal anchor of the     │
   │  — translate HIR-M →   │  │ pattern. Matches sre captures │
   │  regex-syntax Hir      │  │ exactly. Bounded budget under │
   │  once at compile.      │  │ UNLEASHED-off raises on ReDoS │
   └────────────────────────┘  │ via a memo table, never lies. │
                               └──────────────────────────────┘
```

There is **one parser** and **one AST** ("HIR-M" — molt's high-level IR, mirroring the
`sre` opcode set `sre_constants.OPCODES`). The classifier is a pure function over
HIR-M. The two engines are *backends* for the same front-end — this guarantees the
classifier and both engines agree on what the pattern *means* (group numbering, flag
scoping, parse errors) even when they disagree on how to *execute* it.

### 1.2 Why two tiers and not one (the evidence)

| | Tier-F only (REFUSED-B) | Tier-B only (REFUSED-C) | **Two-tier (chosen)** |
|---|---|---|---|
| backref `\1` / lookaround / conditional | ✗ rejects (parity break) | ✓ | ✓ |
| linear-time on `(a\|a)*b` × long input | ✓ (DFA) | ✗ exponential | ✓ (Tier-F) |
| throughput on literal/class/alternation | 1–2 GB/s | ~130 MB/s class | 1–2 GB/s (Tier-F) |
| ReDoS immunity (default) | ✓ | needs memo+budget | ✓ Tier-F immune; Tier-B memoized+budgeted |
| `sre` capture semantics | n/a | ✓ (by construction) | ✓ (Tier-B owns captures; Tier-F captures verified, §3-b) |
| binary size | one crate, trimmable | hand-written, tiny | crate (trimmed) + VM |

The two-tier split is the only design that satisfies **both** the parity contract
(Tier-B) **and** the perf contract (Tier-F), with ReDoS-immunity-by-default for the
common case.

### 1.3 The classifier (sound, AST-based — never regex-on-regex)

A pattern is **Tier-F-eligible** iff its HIR-M contains none of:

1. `Backref(n)` / named backref `(?P=name)` — `regex` crate cannot express backrefs
   (they break the O(n) guarantee; this is RE2's founding constraint).
2. `Look{...}` (any lookahead/lookbehind, positive or negative) — `regex` crate has no
   lookaround.
3. `Conditional{...}` — `(?(id)yes|no)`, no `regex`-crate equivalent.
4. Atomic group `(?>...)` / possessive `*+ ++ ?+ {m,n}+` (3.11) — *partially*
   expressible (possessive ≈ greedy under a DFA since a DFA never backtracks), but the
   `sre` *observable* (which alternative wins, what captures) under possessive differs
   from greedy in backtracking contexts; classify conservatively to Tier-B until a
   per-construct equivalence proof lands (§3-d). **Conservative = sound.**
5. A casefold edge where `sre`'s table-driven IGNORECASE and the `regex` crate's
   `unicode-case` folding are known to disagree (§3, DIV-3). Detected structurally: if
   `IGNORECASE` is set AND the pattern contains a literal/class whose codepoints fall in
   the small "sre-special-fold" set (the four codepoints İ/ı/ſ/K plus the dotted-I
   family, §3 DIV-3), route to Tier-B. Otherwise Tier-F's folding is provably identical.

The classifier is **a fold over the AST, not a regex applied to the pattern string.**
A regex-on-regex heuristic (e.g. `if '\\' + digit in pattern: Tier-B`) is explicitly
forbidden — it would misclassify `\\1` (escaped backslash + literal 1) as a backref
and `[\\1]` (octal in class) inconsistently. The classifier sees the *parsed*
`Backref` node or it does not. **Soundness theorem (to be a Phase-2 test, §7):** every
pattern the classifier routes to Tier-F is expressible in `regex-syntax::Hir` with
identical leftmost-first match semantics; the proof obligation is discharged by the
translation function `hir_m_to_regex_hir` being total on the Tier-F-eligible subset
and by the differential harness (§3.6).

**Soundness > completeness.** If translation is ever unsure, it routes to Tier-B
(slower but correct), never the reverse. A pattern wrongly sent to Tier-F could
silently mismatch; a pattern wrongly sent to Tier-B is merely slower. The classifier
is therefore biased toward Tier-B on any ambiguity.

### 1.4 The parity-required vs UNLEASHED-eligible boundary (THE mandated section)

This mirrors doc-33 §1.4 (the house template: every row is a concrete observable; the
column says whether it is fixed by parity or eligible for an explicit trade).

| # | Observable surface | DEFAULT-tier contract (parity-required) | UNLEASHED-tier eligibility (what it trades) |
|---|---|---|---|
| R1 | **Pattern acceptance** (every valid CPython 3.12+ pattern compiles) | byte-identical: backref, lookaround, conditional, atomic, possessive, scoped flags all compile and match | **traded.** UNLEASHED (`--regex-tier=unleashed`) **rejects at compile** any pattern requiring Tier-B (backref/lookaround/conditional/atomic), raising `re.error("feature unavailable in linear-time mode")`. The acceptance *surface* shrinks to the RE2 subset. Nothing else changes. |
| R2 | **Match spans & group captures** (incl. under `(a*)*`, alternation, optional groups) | byte-identical to `sre` for the whole accepted surface | unchanged for accepted patterns — Tier-F captures are verified equal to `sre` (§3-b). No trade on what it accepts. |
| R3 | **Empty-match iteration** (`finditer`/`split`/`sub` 3.7+ rule: an empty match may occur immediately after a non-empty one, never adjacent to another empty) | byte-identical (`sub('x*','-','abxd') == '-a-b--d-'`) | unchanged. The iteration driver is tier-independent (§3, DIV-2). |
| R4 | **Worst-case time** (catastrophic backtracking on `(a+)+$` × `"aaaa…!"`) | Tier-F patterns: **O(n) guaranteed** (DFA). Tier-B patterns: **bounded** — a memo table + a configurable step budget (`sys`-style, default generous); on budget exhaustion **raises `RecursionError`-class**, never returns a wrong answer. (CPython 3.11+ `sre` also backtracks but added memoization; molt's budget+memo is *stricter*, never silently truncating.) | **traded → strengthened.** UNLEASHED makes worst-case time **O(n) for the entire accepted surface** (no Tier-B exists), i.e. ReDoS becomes *structurally impossible*. The trade (R1) buys an absolute guarantee. |
| R5 | **`re.error` / `PatternError` message, arity, attributes** (`msg`, `pattern`, `pos`, `lineno`, `colno`) | byte-identical message text + all five attributes (TODAY MISSING `lineno`/`colno`, §2-e) | unchanged — UNLEASHED still raises identical `re.error` for *parse* errors; only adds the R1 "feature unavailable" error for rejected-but-valid patterns. |
| R6 | **`IGNORECASE` case folding** | byte-identical to `sre` (table-driven full fold incl. the İ/ı/ſ/K specials, §3 DIV-3) | unchanged — both tiers use the same fold tables (Tier-F via `unicode-case`, Tier-B via the ported `_casefix` table). No trade. |
| R7 | **`\b`/`\B`/`\w`/`\d`/`\s` class definitions** (Unicode vs ASCII flag) | byte-identical to `sre`'s Unicode-word / `unicode-perl` definitions, ASCII flag respected | unchanged — same class tables both tiers. No trade. |
| R8 | **Pattern-object identity & cache** (`re.compile` returns the cached object; `re._cache` LRU=512; `re.purge()`) | byte-identical: same object on cache hit, same eviction, `purge()` clears | unchanged. |
| R9 | **`sre_constants` / `sre_compile` / `sre_parse` / `_sre` surface** | present and behaviorally correct (TODAY STUBS, §2-f) — `sre_parse.parse()` returns a real structure; `_sre` compile path exists | unchanged — these internal modules are tier-independent. |
| R10 | **Throughput** | Tier-F **≥ CPython `re`** on every bench (target: 5–20× on literal/class/alternation); Tier-B ≥ CPython on backref/lookaround patterns | **strengthened** — UNLEASHED is all-Tier-F, so every accepted pattern gets DFA throughput. |

**The contract in one line:** the default tier surrenders **nothing** vs CPython 3.12+
`re`; the UNLEASHED tier surrenders exactly **{R1 pattern-acceptance: it rejects
backref/lookaround/conditional/atomic at compile}** and in exchange upgrades **{R4/R10:
O(n) worst-case + DFA throughput on the *entire* accepted surface}**. UNLEASHED is
"molt's RE2 mode" — semantically a strict *subset* of CPython `re`, never a divergent
*reinterpretation*. The default tier **never silently diverges**: a Tier-B budget
exhaustion raises, it does not return a wrong span.

---

## 2. Current-state audit (HEAD `8679b0655`) — the real gaps

The engine is `molt-runtime-regex/src/regex.rs` (live; satellite) ≡ `builtins/regex.rs`
(duplicate; in-tree). All anchors below are the satellite copy.

### 2-a. Silent ReDoS truncation — IMPORTANCE 3 × GAP 3 = **9 (CRITICAL, web)**

`MatchState::depth` is capped at `MAX_RECURSION_DEPTH = 5000` (`regex.rs:2123`).
`try_match` increments depth and, on overflow, **returns `None`** (`regex.rs:2195-2199`)
— i.e. "no match." For a pattern like `(a+)+$` against `"aaaaaaaaaaaaaaaaaaaa!"`, the
backtracker explodes; once depth crosses 5000 the engine reports **no match where
CPython reports no match too** — but for a pattern that *should* match deep input, the
cap makes molt report **no match where CPython matches**. This is a **silent wrong
answer**, the most dangerous divergence class (worse than the hang it replaced — a hang
is at least observable). The structural fix is Tier-F (DFA, no recursion) for the linear
subset and a **memo-table + raising budget** for Tier-B (§1.4 R4).

### 2-b. No linear-time core; `search` is O(n²) state-rebuild — IMPORTANCE 3 × GAP 3 = **9**

`execute_match` mode `"search"` (`regex.rs:2950-2969`) loops `for start in pos..=end`
and **constructs a fresh `MatchState` (cloning the entire `Vec<char>` of the haystack)
at every start position** (`regex.rs:2958`). That is O(n) allocations of O(n) each =
**O(n²) just in setup**, before any matching. There is no prefilter (no `memchr`, no
Teddy), so a literal search `"needle"` in a 1 MB haystack scans char-by-char with a
full MatchState rebuild per position. Tier-F's `meta::Regex` does this with a
`memchr::memmem` / Teddy prefilter at ~GB/s.

### 2-c. Capture-under-nested-repetition is acknowledged-fragile — IMPORTANCE 3 × GAP 2 = **6**

`try_match_group_then_rest` (`regex.rs:2383-2450`) and
`try_match_group_with_continuation` (`regex.rs:2454-2553`) carry ~120 lines of
apology comments ("Simple approach (works for vast majority of patterns)…", "For
proper backtracking, we need inner to know about rest… the group span depends on
inner's end position, which is not directly available…"). The capture of `(a*)*` —
where the *last* iteration's empty match must (per `sre`) leave group 1 set to the
last non-empty capture, an infamous edge — is not handled by a principled
continuation; it is handled by a two-phase "try inner alone, then re-try with
continuation" heuristic that does not provably match `sre`'s capture-reset rule. This
is a **latent capture divergence** on nested quantifiers. Tier-B (a faithful `sre` VM
port) makes it correct by construction; Tier-F delegates captures to the verified
PikeVM/one-pass path (§3-b).

### 2-d. IGNORECASE / `\w` / word-boundary use ad-hoc folding — IMPORTANCE 2 × GAP 2 = **4**

`char_eq` folds via `a.to_lowercase() == b.to_lowercase()` (`regex.rs:2177-2185`);
`is_word_char` is `'_' || is_ascii_alphanumeric() || (!ascii && is_alphabetic())`
(`regex.rs:2171-2173`); the literal fast-path in `functions_re.rs:58-64` uses
`segment.to_lowercase() == literal.to_lowercase()`. None of these is `sre`'s
table-driven behavior. Divergences: (i) `re.I` `[a-z]` does NOT match ſ (U+017F) or
K (U+212A) here, but DOES in CPython (§3 DIV-3); (ii) `\w` includes codepoints
`is_alphabetic()` accepts that `sre`'s word table excludes (and vice-versa for some
marks). Tier-F's `unicode-case`/`unicode-perl` tables + Tier-B's ported `_casefix`
table fix this.

### 2-e. `re.error` missing `lineno`/`colno` — IMPORTANCE 2 × GAP 2 = **4 (parity)**

`re/__init__.py:81-86` defines `error(msg, pattern=None, pos=None)` with attributes
`msg, pattern, pos`. CPython 3.13 `PatternError` (alias `error`) has **five**:
`msg, pattern, pos, lineno, colno` (docs.python.org/3/library/re.html). molt omits
`lineno`/`colno` and does not expose the `PatternError` alias. (This is the same
*class* of bug as the re.error-arity fix in MEMORY `project_session_20260602_correctness_sweep`.)
Fix: compute `lineno`/`colno` from `pos` in the Rust compiler, thread through
`molt_re_compile`'s error path, add the `PatternError = error` alias.

### 2-f. `sre_*` / `_sre` internal modules are stubs — IMPORTANCE 2 × GAP 2 = **4**

`sre_compile.compile()` returns `None` (`sre_compile.py:10-13`); `sre_parse.parse()`
returns `[]` (`sre_parse.py:10-15`); `sre_constants.OPCODES` is a 7-element subset
(`sre_constants.py:10-18`); `_sre.__getattr__` raises `RuntimeError` (`_sre.py:13-16`);
`re/_casefix.py` `EXTRA_CASES = {}` (empty); `re/_parser.py:parse` calls a non-existent
`_re._Parser` (`_parser.py:33-36` — will `RuntimeError`). Libraries that introspect
`sre_parse` (some linters, `sre`-based syntax highlighters) break. The end-state
exposes a real `sre_parse.parse` returning a faithful structure (backed by HIR-M) and a
populated `sre_constants.OPCODES`.

### 2-g. Surface holes — IMPORTANCE 2 × GAP 2 = **4**

Missing from `re/__init__.py`: `re.Scanner` (the lexer-scanner class — tokenizer use
case, §4), `re.template` is absent, `re.Match.__copy__`/`__deepcopy__`, possessive/
atomic parse (3.11), `\Z`-correct semantics (current `end_abs` at `regex.rs:2619-2628`
wrongly matches before a trailing newline — `\Z` must match **only** at absolute end;
docs: "`\Z` Matches only at the end of the string"), `\p{...}`/`\P{...}` Unicode-property
classes (ML tokenizer blocker), and the 3.12 conditional `(?(name)...)` accepting only
ASCII group names.

---

## 3. Semantics fidelity — the divergence table and its pinning tests

The CPython `test_re.py` suite (`third_party/cpython-3.12/Lib/test/test_re.py`, present
in-repo) is the oracle. Below are the known finite-automata-vs-`sre` traps; each row
names the **pin** (the differential test that must pass on both tiers, gated per phase).

| # | Divergence | `sre` behavior | FA-engine risk | Pin (oracle test) |
|---|---|---|---|---|
| DIV-1 | **leftmost-first vs leftmost-longest** | `sre` is leftmost-**first** (Perl): `re.match('a|ab','ab').group()=='a'` | `regex` crate is ALSO leftmost-first (BurntSushi: literal-trie ordering preserves first-alternative preference, not longest). POSIX leftmost-longest is a *different* crate mode molt must NOT enable. | `assert re.match('a|ab','ab')[0]=='a'`; alternation-order battery from `test_re.py::test_*alternation*` |
| DIV-2 | **empty-match advance (3.7+)** | "an empty match can occur immediately after a non-empty match … `sub('x*','-','abxd')=='-a-b--d-'`" | Both tiers share the molt iteration driver, but the *driver* must implement the 3.7 rule, not the engine. Current Rust `prev_empty_at` logic (`regex.rs:3160-3196,3289-3343`) approximates it; must be replaced by the exact rule. | `re.sub('x*','-','abxd')=='-a-b--d-'`; `re.split(r'\b','Words, words, words.')==['','Words',', ','words',', ','words','.']`; `list(re.finditer('','abc'))` span battery |
| DIV-3 | **IGNORECASE full fold** | full Unicode fold; `[a-z]` w/ `re.I` matches the 52 ASCII + **İ ı ſ K** (U+0130/0131/017F/212A); `ß`/`ı` dotted-I edge (bpo-31193) | `regex` crate `unicode-case` fold is Simple+Full case folding and **does** include ſ→s, K→k — but the `[a-z]`-range-includes-4-specials behavior is an `sre` *range-expansion* quirk; verify equality, route the 4-codepoint edge to Tier-B if unequal (classifier rule §1.3.5) | `[m for m in re.findall('(?i)[a-z]+','baﬄeſK')]`; `test_re.py::test_ignore_case*`; the İ/ı battery |
| DIV-4 | **`$` vs `\Z` vs MULTILINE** | `$` matches end-or-before-final-`\n`; `\Z` matches **only** absolute end; `$` in MULTILINE matches before any `\n`; single `$` in `'foo\n'` finds **two** empty matches | current `end_abs` is BUGGY (matches before trailing `\n`, `regex.rs:2625`); `regex` crate `$`=`\z` by default and needs `(?m)` mapping + explicit `\Z`→`\z` translation in HIR-M→Hir | `re.findall(r'$','foo\n')` → 2 matches; `re.search(r'foo.$','foo1\nfoo2\n')` normal vs `(?m)`; `\Z` rejects-before-newline battery |
| DIV-5 | **capture under `(a*)*` / nested repetition** | the empty trailing iteration does not clobber group 1's last non-empty capture; `re.match(r'(a*)*','aaa').group(1)` per `sre` | naïve backtracker clobbers (§2-c); PikeVM/one-pass capture semantics in `regex` crate are leftmost-first and **agree with sre here** — but only if Tier-F owns the capture (not the lazy DFA which has no captures) | `re.match(r'(a*)*','aaa')` group(0)/group(1); `(a|b)*` last-capture battery; `test_re.py::test_*repeat*group*` |
| DIV-6 | **`\d \w \s` Unicode vs ASCII** | Unicode by default; `re.A` restricts to ASCII; `\w` = `sre`'s Unicode-word (alnum + `_` + specific categories) | `regex` `unicode-perl` `\w`=`\p{Alphabetic}∪\p{M}∪\p{Nd}∪\p{Pc}∪Join_Control` — verify equal to `sre`'s definition; ASCII flag → `(?-u)` in Hir | `re.findall(r'\w+','héllo_123 ́')` U vs A; `test_re.py::test_*word*`, category battery |
| DIV-7 | **possessive/atomic (3.11)** | `a*+a` fails on `'aaaa'`; `(?>.*).` never matches; `x*+≡(?>x*)` | not in current parser; on a DFA, possessive ≈ greedy (DFA never backtracks) so a *bare* possessive over a non-capturing atom is Tier-F-safe, but inside captures/alternation the observable differs → classify to Tier-B (§1.3.4) | `re.match('a*+a','aaaa') is None`; `re.match('(?>.*).','x') is None`; atomic-group capture battery |
| DIV-8 | **conditional `(?(id)y\|n)`** | yes/no by group-participation; 3.12: id ASCII-digits only, name ASCII-only | no `regex`-crate equivalent → always Tier-B | the docs' `(<)?(\w+@\w+(?:\.\w+)+)(?(1)>|$)` battery: matches `<u@h.com>` and `u@h.com`, rejects `<u@h.com` and `u@h.com>` |
| DIV-9 | **scoped flags `(?i:...)` / `(?-i:...)` / global `(?i)`** | flag scope is lexical to the group; global `(?i)` at start applies to whole pattern; 3.11 deprecates mid-pattern global flags | `regex` crate supports `(?i:...)` and `(?flags-flags:...)` natively → translate HIR-M `ScopedFlags` → Hir group flags; verify scope boundary equals `sre` | `re.findall('(?i:AB)cd','abCD ABcd')` scope battery; mid-pattern-flag deprecation-warning parity |
| DIV-10 | **bytes patterns** (`re.compile(b'...')`) | `bytes` patterns match `bytes`; `\w` is ASCII-only for bytes; no `\p{}`; locale via `re.L` | `regex::bytes::Regex` is the byte-oriented sibling; the engine must dispatch str→`regex::Regex`, bytes→`regex::bytes::Regex`; locale (`re.L`) is Tier-B-only (locale tables) | `re.findall(rb'\w+', b'ab cd')`; `re.L` locale battery (Tier-B) |

### 3-a. Tier-F front-end: HIR-M → `regex-syntax::Hir` (borrowed shape: fancy-regex delegation)

The translation `hir_m_to_regex_hir` lowers the Tier-F-eligible HIR-M subtree into a
`regex_syntax::hir::Hir`, then builds `meta::Regex::builder().build_from_hir(&hir)`.
This is the `fancy-regex` "delegate the easy subexpression to the `regex` crate"
move (BurntSushi/`fancy-regex` PERFORMANCE.md), but applied at the **whole-pattern**
granularity (the classifier already proved the whole pattern is easy). Building from
`Hir` (not re-parsing the pattern string through `regex::Regex::new`) is essential:
it lets molt control flag mapping (`re.A`→`(?-u)`, `re.S`→`(?s)`, `re.M`→`(?m)`,
`re.X` stripped at parse time), `\Z`→`\z` rewriting (DIV-4), and class definitions
(DIV-6) explicitly, rather than hoping `regex`'s parser agrees with `sre`'s.

### 3-b. Tier-F captures: verified-equal to `sre`, owned by PikeVM/one-pass

Per BurntSushi, only the **PikeVM**, **BoundedBacktracker**, and **one-pass DFA**
report capture offsets; the lazy/dense/sparse DFA report only match *spans*
(burntsushi.net/regex-internals §captures). `meta::Regex` already does the optimal
thing: run the lazy DFA to find match *bounds*, then run PikeVM/one-pass *only on the
matched span* for captures. molt uses `meta::Regex::captures` directly. The DIV-5
nested-capture semantics of `regex`'s PikeVM are leftmost-first and **agree with `sre`**
on `(a*)*` (both keep the last non-empty capture) — this is *verified*, not assumed, by
the DIV-5 pin running on Tier-F. If any capture divergence is ever found, the offending
construct is added to the classifier's Tier-B routing set (§1.3) — soundness preserved.

### 3-c. Tier-B: a faithful Rust port of the `_sre` bytecode VM (NOT fancy-regex)

Tier-B is a from-scratch Rust implementation of CPython's `_sre` matching VM
(`Modules/_sre/sre_lib.h` semantics — studied as a PSF-licensed *oracle*, reimplemented
clean). It is a **leftmost-first backtracking VM over the same opcode set** `sre`
compiles to (`LITERAL`, `IN`, `ANY`, `AT`, `BRANCH`, `REPEAT`/`MAX_UNTIL`/`MIN_UNTIL`,
`GROUPREF`/`GROUPREF_EXISTS`, `ASSERT`/`ASSERT_NOT`, `ATOMIC_GROUP`, `POSSESSIVE_REPEAT`).
Three molt-specific strengthenings over a naïve port:

1. **Memoization** — a visited-set keyed on `(opcode_pc, string_pos)` makes the linear
   subset linear and bounds the rest, the way CPython 3.11 added `sre` memoization for
   atomic groups. This is what makes the §1.4-R4 budget *rarely* hit on real patterns.
2. **Regex-crate prefilter** — extract the required literal prefix/suffix/inner of the
   Tier-B pattern (via `regex-syntax` literal extraction over the backref-free skeleton)
   and use `memchr::memmem`/Teddy to jump the VM to candidate start positions, instead
   of trying every position. This gives Tier-B a fast-search outer loop even though its
   inner match is backtracking. (Same idea as `fancy-regex`'s delegation, but for the
   *anchor*, not the whole pattern.)
3. **Raising budget, never truncating** — on memo-bounded-budget exhaustion, raise a
   `RecursionError`-class exception with the pattern + position, **never** return a
   wrong span (fixes §2-a structurally).

Tier-B is studied from `_sre` (PSF, semantics only) and from RE2's "Pike VM with
submatch tracking" exposition (swtch.com/~rsc/regexp/regexp2.html, BSD ideas) — but the
*backtracking* core is required for backrefs (RE2 famously refuses them; molt's default
tier must not).

### 3-d. The atomic/possessive equivalence proof (defers some patterns to Tier-F later)

A later optimization (Phase 4, §7): prove that a possessive/atomic construct over a
*non-capturing, non-backref* sub-pattern is observably identical to its greedy form
under a DFA (true because a DFA explores all paths simultaneously and a possessive
quantifier only forbids *backtracking*, which a DFA never does). Such constructs then
become Tier-F-eligible, widening the fast path. Until that proof + its DIV-7 pin land,
they are conservatively Tier-B. This is the soundness-first discipline: widen Tier-F
only behind a proof + a test.

### 3.5 The pure-Python wrapper's fate (explicit, per the brief)

- `re/__init__.py` — **demoted, not deleted.** It remains the public object layer
  (`Pattern`, `Match`, `error`/`PatternError`, module functions, the LRU cache, flag
  constants) because those are observable Python types/identities CPython programs
  introspect (`isinstance(m, re.Match)`, `re.Pattern.__doc__`, pickling `Pattern`). It
  loses nothing today (it already has no match logic) but **gains** `lineno`/`colno`
  (§2-e), `re.Scanner`, `PatternError` alias, and `\p{}` flag plumbing. Net: stays
  ~500 lines, all object-protocol, zero match logic.
- `re/_compiler.py`, `re/_parser.py` — **rewritten to back onto HIR-M.** Today they
  `import re as _re` and call non-existent internals (`_parser.py:33`). End-state:
  `sre_parse.parse(p)` returns a real structure built from the Rust `molt_re_parse`
  (a new introspection intrinsic exposing HIR-M as `sre`-shaped tuples), so `sre`-based
  tools work.
- `re/_constants.py`, `sre_constants.py` — **completed** (full `OPCODES`, `ATCODES`,
  `CHCODES`, error constants) to match `sre_constants`.
- `re/_casefix.py` `EXTRA_CASES` — **populated** from the ported fold table (DIV-3).
- The two homegrown Rust engines (`molt-runtime-regex/src/regex.rs` matcher §2 +
  `builtins/regex.rs` duplicate) — **DELETED** once Tier-F+Tier-B are green (§7 Phase
  3). The satellite crate `molt-runtime-regex` survives as the *home* of the new
  two-tier engine (it keeps the FFI bridge in `bridge.rs`); the in-tree
  `builtins/regex.rs` duplicate is deleted outright (STALE-4 debt retired).

### 3.6 The differential harness (the oracle runner)

Extend the existing `tools/fuzz_compiler.py` (the doc-31 multi-oracle differential
harness) with a **regex oracle**:

1. **Corpus** = `third_party/cpython-3.12/Lib/test/test_re.py` cases (extracted as
   `(pattern, flags, subject, method)` quadruples) + a generated grammar-fuzzer over
   the HIR-M node set + a ReDoS-pattern battery.
2. **Three-way diff per quadruple**: CPython `re` (the oracle, run under host python3) ⟂
   molt Tier-F ⟂ molt Tier-B. For Tier-F-eligible patterns, **all three must agree** on
   `(span, groups, groupdict, finditer-positions, sub-output, split-output)`. For
   Tier-B-only patterns, CPython ⟂ Tier-B must agree (Tier-F refuses).
3. **Classifier-soundness oracle (CPython-free)**: every pattern the classifier marks
   Tier-F-eligible must `build_from_hir` without error AND match-equal Tier-B on a
   random subject battery — catching a mis-classification even without CPython.
4. **Gate**: a phase is not done until its slice of `test_re.py` is green on both tiers
   (§7 per-phase gates).

---

## 4. UNLEASHED tier, prefilters, multi-pattern

### 4.1 UNLEASHED (`molt build --regex-tier=unleashed`) — RE2 mode

A per-build opt-in (mirroring doc-33's UNLEASHED concurrency tier). Effect:
**the classifier's Tier-B route becomes a compile error.** Any pattern needing
backref/lookaround/conditional/atomic raises `re.error("feature requires the default
regex tier; unavailable under --regex-tier=unleashed")` *at `re.compile`*. The accepted
surface is exactly the RE2 / `regex`-crate subset; in exchange, **every accepted
pattern is O(n) worst-case (ReDoS structurally impossible)** and gets DFA throughput
(R4/R10). Use case: **servers and edge/WASM web handlers** matching attacker-controlled
or attacker-adjacent input (URL routers, input validators, log ingest) where a ReDoS
hang/blowup is a DoS vector. The trade is named exactly (R1: pattern-acceptance) and is
the *only* thing surrendered. This is the RE2 philosophy (google/re2: "a fast, safe,
thread-friendly alternative to backtracking engines") offered as an explicit dial, never
as a silent default. The default build is **full-parity**; UNLEASHED is opt-in and
**fails closed** (rejects-at-compile, never silently-mismatches).

### 4.2 SIMD prefilters (Teddy / memmem) — both tiers, default-on native/WASM

Tier-F gets prefilters for free via `meta::Regex` (literal extraction → `memchr::memmem`
single-substring with x86_64/aarch64 "generic SIMD two-rare-bytes" + Teddy multi-substring
ported from Hyperscan; burntsushi.net/regex-internals §prefilters). Tier-B gets the
anchor-prefilter (§3-c.2). `memchr 2.8.0` is **already a direct dep** of `molt-runtime`,
`molt-runtime-text`, and `molt-runtime-serial` (`Cargo.toml:184`, etc.) and
`aho-corasick`/`regex-automata` are in `Cargo.lock` — the SIMD substrate is present.

### 4.3 Multi-pattern matching (`re.Scanner` / tokenizers)

`re.Scanner` (the lexer-scanner: a list of `(pattern, action)` pairs scanned in order)
is the tokenizer use case (used by `json`'s scanner, `tokenize`, hand-rolled lexers).
End-state: compile the alternation of all scanner patterns into **one** `meta::Regex`
with a capture group per rule and dispatch on which group matched — `regex-automata`'s
multi-pattern API (`PatternID` per match) makes this O(n) over the input regardless of
rule count, vs CPython's per-rule retry. This is a place molt can be *dramatically*
faster than CPython (Scanner is notoriously slow in `sre`). Tokenizer patterns needing
`\p{L}`/`\p{N}` (GPT-2/tiktoken BPE pre-tokenizer) are Tier-F-eligible once `\p{}`
parsing lands (§2-g) — a direct ML win.

---

## 5. Per-target matrix + binary size

| Target | Tier-F engine | SIMD prefilter | Tier-B | Binary-size posture |
|---|---|---|---|---|
| **native (x86_64/aarch64)** | full `meta::Regex` | Teddy (AVX2/SSSE3/NEON) + `memmem` 2-rare-byte SIMD | full `_sre` VM | full Unicode build acceptable on native; ~750 KB Unicode tables (see §5.1) — within native budget |
| **WASM (browser/WASI)** | full `meta::Regex` | `memchr` `simd128` (Teddy has a `wasm32` path via `aho-corasick`; verify enabled) | full `_sre` VM | **the <2 MB binary target binds here** — feature-trim Unicode (§5.1); `MOLT_HERMETIC_MODULE_ROOTS=1` build (feedback_hermetic_wasm_build) |
| **Luau** | transpiled call into the same Rust runtime intrinsics (Luau backend calls `molt_re_*` like any other intrinsic) | inherits native | inherits native | Luau is a transpile target over the Rust runtime; no separate engine |

**Verify (Phase-1 task):** `regex`/`regex-automata` `wasm32-unknown-unknown` +
`wasm32-wasi` build with `simd128`. `regex` is `#![no_std]`-capable in `regex-automata`
(the `alloc`+`std` split); the meta engine needs `std` for the lazy-DFA cache mutex but
WASM-single-thread can use the `alloc`-only PikeVM path if `std` is a problem. This is a
known-good configuration (the crate documents WASM support) but must be pinned by a CI
target.

### 5.1 Binary size — feature-trimming the (already-present) crate

The `regex` crate default is ~1 MB, of which **Unicode tables ≈ 750 KB** (BurntSushi,
PR #613; DeepWiki feature-flags). molt's lever, per the <2 MB mandate
(feedback_priority_perf_startup_first):

- **Keep** `unicode-case` (R6/DIV-3 IGNORECASE fold) + `unicode-perl` (R7/DIV-6
  `\w\d\s\b`) — these are **parity-load-bearing**; dropping them would break the default
  tier's R6/R7 contract. Together ≈ 500 KB.
- **Drop** `unicode-script`, `unicode-segment`, `unicode-age`, `unicode-gencat`
  (general-category beyond perl classes) on WASM **unless** `\p{Script=...}`/`\p{Age=...}`
  patterns are used — and they rarely are. Saves ≈ 250 KB. If a pattern needs a dropped
  property, the classifier routes it to Tier-B (which carries its own minimal tables)
  rather than bloating Tier-F.
- **Keep** `perf-literal` (Teddy/memmem prefilter — the throughput) and `perf-dfa`
  (lazy DFA). **Drop** `perf-dfa-full` (fully-compiled DFA — "Large" size/compile cost,
  off by default anyway; BurntSushi). The lazy DFA gives ~the same throughput without
  the size.
- **Per-app DCE** (feedback_treeshaking + the doc-29 binary-size lever): the WASM build
  links only the engine config the app's patterns need. A constant-pattern-only app
  (all `re.compile(r"literal")`) that never hits Tier-B can tree-shake the entire `_sre`
  VM out (§6 AOT path makes this analyzable).

Net WASM target: Tier-F ≈ 500–700 KB (trimmed) + Tier-B ≈ 80–120 KB (the VM is small;
its tables are the cost) — comfortably inside the per-subsystem share of the <2 MB
budget, and **less** than the current 5.7 KB-line homegrown engine compiles to once you
count both duplicate copies.

---

## 6. The AOT superpower — compile-time pattern compilation

The defining molt advantage over CPython: **when the pattern is a compile-time
constant, compile the automaton at molt-build time and embed it in the binary — zero
runtime compile cost.**

### 6.1 The constant-pattern path

The frontend already constant-folds. A call `re.compile(r"...")` / `re.match(r"...", s)`
/ `re.sub(r"...", ...)` where the pattern (and flags) are literal is detected at lower
time. For such sites:

1. **Compile-time**: parse → HIR-M → classify.
   - Tier-F-eligible → build `meta::Regex` from Hir at *build* time, **serialize the
     lazy-DFA's underlying dense DFA via `regex-automata`'s `Automaton::to_bytes()`**
     (the API BurntSushi documents for `regex-cli`-style embedding — "serializing fully
     compiled DFAs to a file and generating Rust code to read them … zero-copy
     deserialization"), and embed the bytes as a `static` in the binary. Runtime
     `re.compile` of that literal returns a `Pattern` wrapping a `from_bytes()`
     zero-copy view — **no parse, no NFA build, no DFA build at runtime.**
   - Tier-B → embed the compiled HIR-M opcode program as a `static` byte array; runtime
     wraps it directly (no parse).
2. **Runtime (dynamic pattern)**: `re.compile(user_input)` where the pattern is *not*
   constant → the full runtime path (parse → HIR-M → classify → build), cached in the
   existing LRU (`re._cache`, cap 512). This path is unchanged in shape from today, just
   pointed at the new engine.

### 6.2 Cache keying

- **Compile-time embedded patterns**: keyed by `(normalized_pattern_bytes, flags,
  tier, target_arch)` at build time. Two `re.compile(r"\d+")` sites in different modules
  dedup to **one** embedded automaton (the build-time pattern interner). `target_arch`
  is in the key because the serialized DFA's `memchr`/Teddy prefilter selection is
  arch-specific (a DFA serialized for AVX2 must not load on a NEON target — the
  `from_bytes` check enforces this; molt's key avoids even attempting it).
- **Runtime dynamic patterns**: the existing `(pattern, flags)` LRU key
  (`re/__init__.py:90,387`) — unchanged, now caching the new `Pattern` handle.
- **Serialized-DFA validity**: `regex-automata`'s `from_bytes` validates alignment +
  endianness + version; molt's build embeds the bytes already aligned for the target,
  and the runtime `from_bytes` is the zero-copy fast path (no validation cost beyond the
  header check) on the trusted self-produced bytes.

### 6.3 Why this beats CPython categorically

CPython compiles every `re.compile` at runtime (its `re._cache` only saves *re*-compiles
within a run). molt's constant patterns have **their automaton in `.rodata`** — the
first match is as fast as the millionth, startup pays nothing (feedback_priority_perf
startup-first). This is the AOT thesis applied to regex: the work CPython does at import
time, molt does at build time.

---

## 7. Phased build plan (complete-piece phases, deletions, gates, risk)

Each phase is independently shippable, leaves the tree in a coherent state, and has a
differential gate. LoC estimates are Rust unless noted. The **riskiest phase is Phase 3
(the cutover/deletion)** — flagged.

### Phase 0 — Scaffolding + classifier + harness (no behavior change) — ~600 LoC

- Add `regex = { version = "1.12", default-features = false, features = ["std",
  "unicode-case", "unicode-perl", "perf-literal", "perf-dfa"] }` to
  `molt-runtime-regex/Cargo.toml` (the crate is already in `Cargo.lock` 1.12.3 — pins,
  no new vendored code).
- Define **HIR-M** as the canonical AST (lift the existing `ReNode` enum, §regex.rs:992
  — it is already close; add `AtomicGroup`, `PossessiveRepeat`, `UnicodeProp(\p{})`).
- Implement the **classifier** `tier_of(&HirM) -> Tier` (§1.3) as a pure fold.
- Implement `hir_m_to_regex_hir` (§3-a) — total on the Tier-F-eligible subset.
- Extend `tools/fuzz_compiler.py` with the regex oracle (§3.6).
- **Gate**: classifier-soundness oracle green (every Tier-F-eligible fuzz pattern
  `build_from_hir`s and match-equals the *existing* engine on a subject battery). No
  user-visible change yet (engine still the old one).
- **Deletes**: nothing.

### Phase 1 — Tier-F live for eligible patterns (the throughput + ReDoS-immunity win) — ~900 LoC

- Route Tier-F-eligible compiles through `meta::Regex` (`build_from_hir`); `execute`/
  `finditer`/`split`/`sub`/`subn` for Tier-F patterns use `meta::Regex::captures_iter`
  + the molt iteration driver implementing the **3.7 empty-match rule** (DIV-2) and the
  `sub`/`split` capture-inclusion rules. Tier-B patterns still use the old homegrown
  engine (temporary coexistence — documented as the hybrid state).
- Fix DIV-4 (`\Z`→`\z`, `$`/`(?m)` mapping) and DIV-6/DIV-3 flag mapping in the
  translation.
- Add `regex::bytes::Regex` dispatch for bytes patterns (DIV-10).
- Verify WASM `simd128` build (§5).
- **Gate**: the Tier-F-eligible slice of `test_re.py` (literals, classes, alternation,
  anchors, quantifiers, scoped flags, Unicode classes, IGNORECASE non-special) green
  3-way (CPython ⟂ Tier-F ⟂ old-engine). Throughput bench (§8) shows Tier-F ≥ 5× CPython
  on literal/class/alternation. **The §2-a silent-ReDoS-truncation bug is now fixed for
  all Tier-F patterns** (DFA, no recursion).
- **Deletes**: nothing yet (old engine still serves Tier-B).

### Phase 2 — Tier-B: faithful `_sre` VM port (closes the correctness holes) — ~1,400 LoC

- Implement the **memoized leftmost-first `_sre` VM** (§3-c) over HIR-M: backrefs,
  lookaround (variable-width lookbehind **rejected** with the `sre` error-parity message,
  per the brief), conditionals, atomic groups + possessive (3.11, DIV-7), the
  raising-budget (R4, fixes §2-a for Tier-B), and the anchor-prefilter (§3-c.2).
- Port the `_casefix` fold table (DIV-3) and the Unicode-word table (DIV-6) shared by
  both tiers; populate `re/_casefix.py::EXTRA_CASES`.
- Fix §2-c (capture-under-nested-repetition) **by construction** (the VM tracks
  group marks exactly like `sre`).
- **Gate**: the Tier-B slice of `test_re.py` (backref, lookaround incl. variable-width
  lookbehind *rejection* parity, conditional, atomic/possessive, `(a*)*` capture
  battery DIV-5, `re.L` locale) green 2-way (CPython ⟂ Tier-B). The ReDoS battery either
  matches CPython or raises the budget exception — **never returns a wrong span**.
- **Deletes**: nothing yet (the cutover is Phase 3).

### Phase 3 — Cutover + deletion (RISKIEST) — net **−5,400 LoC** (deletes ~5,700, adds ~300 glue)

- Switch the live `molt_re_*` intrinsics to dispatch Tier-F/Tier-B exclusively; **delete
  the homegrown matcher** (`regex.rs` §2 matcher body, the `try_match*` family
  `:2194-2896`, `execute_match` `:2913-2972`) and the **entire duplicate**
  `builtins/regex.rs` + its `mod.rs:116` wiring (STALE-4 debt retired).
- Delete the dead lookaround-descriptor helpers (`molt_re_positive_lookahead` etc.,
  `regex.rs:223-462`) and the vestigial `functions_re.rs` literal-advance intrinsics
  (`molt_re_literal_advance`/`_any_advance`/`_matches`) once the shims (`_compiler.py`/
  `_parser.py`/`_casefix.py`) are repointed at the new `molt_re_parse` introspection
  intrinsic.
- Add `re.error` `lineno`/`colno` + `PatternError` alias (§2-e); add `re.Scanner` (§4.3)
  on the multi-pattern API; complete `sre_constants`/`sre_parse`/`sre_compile` (§2-f).
- **Gate (the big one)**: the **entire** `test_re.py` green on the default tier across
  native + WASM + Luau; the full `tools/fuzz_compiler.py` regex oracle green; binary
  size measured ≤ the §5.1 budget on WASM; **no `NotImplementedError`, no
  `SENTINEL_FALLBACK`, no depth-truncation path remains** (grep-clean). This is the
  phase that flips the default; it is gated hardest and is where a coexistence bug would
  bite — hence riskiest. Mitigation: Phases 1–2 already proved both tiers green *in
  parallel* with the old engine, so Phase 3 is a *switch + delete*, not a *new
  behavior*.
- **Deletes**: ~5,700 lines of unsound duplicated engine. This is the largest single
  debt retirement in the subsystem.

### Phase 4 — AOT embedding + UNLEASHED + widenings (the perf/ergonomics frontier) — ~700 LoC

- Implement the **compile-time constant-pattern path** (§6): build-time HIR-M → Tier-F
  DFA `to_bytes()` embedded as `static`; runtime `from_bytes` zero-copy; the build-time
  pattern interner + cache key (§6.2). Tier-B constant patterns embed their opcode
  program.
- Implement **UNLEASHED** (`--regex-tier=unleashed`, §4.1): classifier Tier-B route →
  compile error; per-app DCE drops the `_sre` VM entirely when no pattern needs it.
- Implement the **atomic/possessive→Tier-F widening proof** (§3-d) behind its DIV-7 pin.
- Add `\p{...}`/`\P{...}` Unicode-property classes (ML tokenizer, §4.3) — Tier-F via
  `regex` property support.
- **Gate**: AOT path shows zero runtime-compile cost for constant patterns (startup
  bench); UNLEASHED rejects-at-compile the Tier-B battery and is O(n) on the ReDoS
  battery; `\p{L}` tokenizer (GPT-2 BPE pre-tokenizer regex) matches CPython.
- **Deletes**: under UNLEASHED builds, the `_sre` VM (via DCE) — not a source deletion,
  a link-time one.

### LoC ledger

| Phase | Adds | Deletes | Net |
|---|---|---|---|
| 0 | ~600 | 0 | +600 |
| 1 | ~900 | 0 | +900 |
| 2 | ~1,400 | 0 | +1,400 |
| 3 | ~300 | ~5,700 | **−5,400** |
| 4 | ~700 | 0 (link-time only) | +700 |
| **Σ** | ~3,900 | ~5,700 | **−1,800** |

The end-state is **smaller** than today (a correct two-tier engine + DFA + `_sre` VM is
fewer lines than the two duplicated unsound copies) **and** linear-time-by-default
**and** byte-identical to CPython 3.12+.

---

## 8. Benchmark lane (molt MUST beat CPython on every bench)

Baselines: CPython 3.12 `re` (≈ 130 MB/s typical); `regex` crate (≈ 1–2 GB/s class,
BurntSushi). Benches (each on native release-fast + WASM + Luau, per the perf contract):

| Bench | Pattern / workload | CPython `re` baseline | molt target | Engine |
|---|---|---|---|---|
| **literal-scan** | `re.search("needle", 1 MB haystack)` | ~scan speed, no SIMD | **≥ 10× (memmem SIMD prefilter)** | Tier-F |
| **class-scan** | `re.findall(r"\d+", 1 MB)` | ~130 MB/s | **≥ 8×** (lazy DFA) | Tier-F |
| **alternation** | `re.findall("foo|bar|baz|qux", 1 MB)` | slow (per-alt) | **≥ 10×** (Teddy multi-substring) | Tier-F |
| **capture-heavy** | `re.finditer(r"(\w+)=(\w+)", 1 MB config)` | ~130 MB/s | **≥ 5×** (lazy-DFA-bounds + one-pass captures) | Tier-F |
| **scanner/tokenizer** | 30-rule `re.Scanner` over 1 MB source | very slow (per-rule retry) | **≥ 15×** (single multi-pattern DFA, §4.3) | Tier-F |
| **backref** | `re.fullmatch(r"(\w+) \1", s)` dedup | ~130 MB/s | **≥ 1.5×** (memoized VM + anchor prefilter) | Tier-B |
| **lookahead** | `re.findall(r"\d(?=px)", 1 MB)` | ~130 MB/s | **≥ 2×** (VM + prefilter) | Tier-B |
| **pathological / ReDoS** | `(a+)+$` × `"a"*40 + "!"` | **hangs/exponential** | Tier-F-eligible variant: **O(n)**; Tier-B variant: **raises in bounded steps** (CPython hangs) — a *correctness+safety* win, not just speed | both |
| **compile-latency** | `re.compile(r"<50-char pattern>")` ×10⁴ | runtime parse+compile each | **≈ 0 for constant patterns (AOT-embedded, §6)**; ≥ 2× for dynamic | both |

The pathological bench is the headline: where CPython hangs, molt's default tier either
runs O(n) (Tier-F) or raises in bounded steps (Tier-B) — **molt is strictly safer AND
faster**, satisfying the "faster than CPython on every benchmark" contract even on the
adversarial input class.

---

## 9. Dependency edges & composition

- **Independent of all in-flight compiler arcs** (E1 inliner, S5 MemSSA, S6 SCEV, RC
  ownership #20) — confirmed at HEAD; the engine lives in the runtime satellite, not the
  TIR pipeline. The only cross-arc touch is the **AOT constant-pattern path** (§6), which
  rides the existing frontend constant-folding + per-app DCE (feedback_treeshaking) — a
  consumer of those, not a modifier.
- **Composes with WASM tree-shaking** (the constant-pattern DCE, §5.1/§6) and the
  hermetic WASM build (feedback_hermetic_wasm_build).
- **`molt-runtime-regex` satellite** stays the home; its `bridge.rs` FFI (the `__molt_regex_*`
  shims in `runtime/molt-runtime/src/regex_bridge.rs`) is reused unchanged — the new
  engine allocates Python objects through the same bridge.
- **No GIL interaction beyond today's** — `meta::Regex` is `Send + Sync`; a compiled
  `Pattern` is immutable and shareable, which is *better* than the current
  `Mutex<HashMap>` registry (`regex.rs:1055`) and aligns with doc-33's free-threading
  direction (an immutable compiled pattern needs no per-object lock under UNLEASHED
  concurrency).

---

## 10. Summary

molt's `re` engine end-state is a **two-tier design over one `sre`-faithful parser**:
**Tier-F** = the `regex`/`regex-automata` crate (`meta::Regex`: lazy DFA + one-pass +
PikeVM + bounded backtracker + Teddy/memmem SIMD prefilter) for the linear,
backref-free subset — built `from_hir` so molt controls flag/class/`\Z` mapping for
`sre` parity; **Tier-B** = a from-scratch, memoized, leftmost-first Rust port of the
`_sre` bytecode VM (backrefs, lookaround, conditionals, atomic/possessive) with a
regex-crate anchor-prefilter and a **raising** budget. A **sound AST classifier** routes
patterns (biased to Tier-B on any ambiguity — soundness over completeness). The default
tier is **byte-identical to CPython 3.12+**, ReDoS-immune on the common case and
never-silently-wrong on the rest; the opt-in **UNLEASHED tier** is RE2-mode
(guaranteed-O(n), rejects backref/lookaround at compile). Constant patterns are
**compiled to embedded automata at molt-build time** (the AOT superpower — zero runtime
compile cost). The current **5,700-line duplicated unsound homegrown backtracker is
deleted**; the pure-Python `re/__init__.py` is **demoted to a thin object layer**. Net
LoC is *negative*. The riskiest step is the Phase-3 cutover/deletion, de-risked by
proving both tiers green in parallel with the old engine in Phases 1–2.

Sources (provenance):
- BurntSushi, [Regex engine internals as a library](https://burntsushi.net/regex-internals/) — meta engine, prefilters, DFA serialization (`to_bytes`/`from_bytes`), captures, leftmost-first.
- [rust-lang/regex](https://github.com/rust-lang/regex) + [regex-automata docs](https://docs.rs/regex-automata) — engine APIs (`meta::Regex`, `dfa::dense`, `nfa::thompson`, `onepass`).
- [regex feature-flags / binary size (DeepWiki)](https://deepwiki.com/rust-lang/regex/1.3-feature-flags-and-configuration) + [PR #613](https://github.com/rust-lang/regex/pull/613) — Unicode ≈750 KB, sub-feature trimming, regex-lite ≈94 KB.
- [fancy-regex](https://github.com/fancy-regex/fancy-regex) (MIT, v0.18.0) + [PERFORMANCE.md](https://github.com/google/fancy-regex/blob/master/PERFORMANCE.md) — delegate-or-backtrack hybrid; Oniguruma (NOT sre) target.
- [google/re2](https://github.com/google/re2) + [Russ Cox, Regular Expression Matching](https://swtch.com/~rsc/regexp/) (BSD ideas) — linear-time philosophy, Pike VM submatch tracking.
- [CPython re docs](https://docs.python.org/3/library/re.html) (PSF, semantics oracle) — empty-match 3.7 rule, `$`/`\Z`/MULTILINE, IGNORECASE full fold, `PatternError` attributes (msg/pattern/pos/lineno/colno), atomic/possessive 3.11, conditionals 3.12.
- [Python 3.11 atomic groups & possessive quantifiers](https://learnbyexample.github.io/python-regex-possessive-quantifier/) (bpo-433030).
- In-repo audited at HEAD `8679b0655`: `runtime/molt-runtime-regex/src/regex.rs`, `runtime/molt-runtime/src/builtins/regex.rs`, `runtime/molt-runtime/src/builtins/functions_re.rs`, `runtime/molt-runtime-regex/src/bridge.rs`, `src/molt/stdlib/re/__init__.py`, `src/molt/stdlib/{sre_constants,sre_parse,sre_compile,_sre}.py`, `Cargo.lock` (regex 1.12.3), `runtime/molt-runtime/Cargo.toml`, `docs/design/foundation/{16,29,33}_*.md`, `third_party/cpython-3.12/Lib/test/test_re.py`, `tools/fuzz_compiler.py`.
