# 23 — The Grand Council: advisory/research/design canon

Status: living roster (2026-06-05). Chair: Chris Lattner.

Purpose: a named roster of the people whose published work is the canon for
molt's design questions, organized into chambers mapped to molt's actual
roadmap arcs. The council is operationalized three ways (see §Operationalization):
seats become adversarial-review lenses, judge-panel personas for design docs,
and per-arc canon reading lists. Seat-inclusion test: every seat must be
justified by a molt arc it directly advises — working output over fame.

Composition doctrine (how Lattner runs bodies like this, per Swift core team /
LLVM dev meetings / MLIR open design meetings): small standing core, on-demand
chambers, a permanent red team, bias for people who still ship.

## Standing Core (cross-cutting design; weekly)

| Seat | Domain | molt arc |
|---|---|---|
| Chris Lattner (chair) | LLVM/Clang/Swift/MLIR/Mojo | Substrate-first doctrine; Mojo = nearest living Python-family AOT with a Repr-like lattice |
| Mark Shannon | CPython specializing adaptive interpreter (PEP 659) | Guarded-specialization middle tier (class-version guards, ICs, deopt) |
| CF Bolz-Tereick | PyPy | Verified-subset boundary honesty; "approach PyPy on dynamism-heavy" competitive ladder |
| Chris Fallin | Cranelift, regalloc2, ISLE | Native backend IS Cranelift; ISLE lowering-rules-as-data = M2 end-state for compile_func_inner |
| Daan Leijen | Koka/Perceus/mimalloc | Entire MM ladder: DropInsertion → borrow inference → reuse → allocator bake-off |
| Russ Cox | Go build/module system | Go-like DX mandate: content-addressed caching, deterministic builds, crate-DAG decomposition |
| George Hotz | tinygrad | Holds the public ML contract — exact tinygrad fidelity is turn-blocking policy |
| Nuno Lopes | Alive2 | V1 translation validation: prove TIR passes refine semantics |

## Compiler & VM Chamber

| Seat | Domain | molt arc |
|---|---|---|
| Cliff Click | HotSpot C2, sea-of-nodes, deopt | Deopt skeleton + guarded-tier exit design |
| Lars Bak | V8/HotSpot/Dart VM | Inline caches at industrial scale (call_method_ic lineage) |
| Urs Hölzle | Self, polymorphic inline caches | Poly-IC roadmap item — he invented them |
| Mike Pall | LuaJIT | Definitive NaN-boxing; chief witness for value-format bake-off (#33); Luau backend lineage |
| Vyacheslav Egorov | V8/Dart optimization forensics | Perf-cliff debugging craft (loop-IV cliff class) |
| Jeff Bezanson, Keno Fischer | Julia | Type-specialization without mandatory annotations; "Julia but better" benchmark; Keno: debugger/compiler interop |
| Tobias Grosser, Albert Cohen | Polly / MLIR affine, polyhedral | Y4 loop-transform + SIMD tiers |
| Mehdi Amini | MLIR infra | Pass managers, verifiers, threading — S1 AnalysisManager/PassManager review |
| Vikram Adve | LLVM co-creator | Heterogeneous compilation; working elder |

## Python Chamber

| Seat | Domain | molt arc |
|---|---|---|
| Guido van Rossum | Language design; faster-cpython | What Python means + how to make it fast — both halves |
| Brandt Bucher | CPython copy-and-patch JIT | The other serious compile-CPython-semantics effort; guard design |
| Pablo Galindo Salgado | PEG parser, CPython error messages | Parity ladder lives on byte-identical error-message conformance 3.12/13/14 |
| Sam Gross | nogil / free-threading (PEP 703) | GIL/subinterpreter ladder must not contradict his map |
| Eric Snow | Subinterpreters (PEP 554/734) | RuntimeState-scoped slots already point here |
| Yury Selivanov | asyncio, uvloop | asyncio parity arc (active P0 work) |
| Nathaniel J. Smith | Trio, structured concurrency | Async-semantics correctness conscience |
| Raymond Hettinger | itertools/collections | Generator-fusion keystone targets itertools-as-plain-Python; idiom authority |
| Tim Peters | timsort, float repr | list.sort + float formatting parity landmines |
| Jukka Lehtosalo, Eric Traut | mypy, pyright | Verified-subset conformance manifest is a typing-semantics artifact |
| David Hewitt | PyO3 | Rust↔Python ABI; Y1.5 extension-module ABI (METH_FASTCALL et al.) |
| Charlie Marsh | ruff, uv | Existence proof for Rust-for-Python tooling with go-like DX |
| Samuel Colvin | pydantic-core | Archetype Y1.5 Rust-backed library target |

## GPU / ML Chamber

| Seat | Domain | molt arc |
|---|---|---|
| Philippe Tillet | Triton | Closest prior art to molt.gpu: Python-ish kernels → GPU with a real cost model |
| Jason Ansel, Horace He | TorchDynamo/Inductor | Capturing Python dynamism for compilation; graph-break taxonomy = our subset boundary, mirrored |
| Jonathan Ragan-Kelley | Halide | Algorithm/schedule separation framing for fusion + scheduling |
| Tianqi Chen | TVM, MLC | Compiler-driven kernel search; portable deployment |
| Tri Dao | FlashAttention | Kernel-craft ground truth adjacent to DFlash fidelity (verifier/drafter separation, KV injection) |
| Bill Dally | NVIDIA architecture | Keeps GPU cost models honest against silicon |

## Web / WASM Chamber

| Seat | Domain | molt arc |
|---|---|---|
| Alon Zakai | Emscripten, Binaryen | We maintain a relooper; he wrote the canonical one + successor |
| Luke Wagner | WASM co-design, component model/WASI | WASM tier's linking future |
| Ben Titzer | WASM co-founder, Wizard, Virgil | Engine-side semantics; one-person-toolchain taste model |
| Andreas Rossberg | WASM formal semantics | Keeps the WASM backend honest (Leroy's role, for WASM) |
| Nick Fitzgerald | wasmtime, wasm-tools | Our actual test substrate; host-import wiring bug class |

## OS & Portability Chamber

| Seat | Domain | molt arc |
|---|---|---|
| Rob Pike | Go, Plan 9, UTF-8 | "Bigger the interface, weaker the abstraction" — decomposition doctrine |
| Ian Lance Taylor | gold linker, gccgo, Go generics | Link-root manifests, LTO, dead-code stripping are linker questions |
| Austin Clements | Go runtime | Scheduler/GC/runtime interplay for free-threading future |
| Bryan Cantrill | DTrace, Oxide | Debuggability/observability as first-class axes |
| Brendan Gregg | Flame graphs, systems perf methodology | Standing profile-first bug-hunt mode IS his method |
| Krste Asanović | RISC-V | Y3 portability ladder ISA endpoint |
| Raymond Chen | Win32 compatibility | Y1 Windows port conscience: compatibility is a forever contract |

## Memory & Unsafe-Core Chamber

| Seat | Domain | molt arc |
|---|---|---|
| Ralf Jung | Miri, stacked/tree borrows, RustBelt | Weekly Miri over a NaN-boxed unsafe core; defines what our `unsafe` may mean |
| Hans Boehm | Memory models, conservative GC | Atomics/memory-model scars for free-threading |
| Steve Blackburn | MMTk, Immix | Y2 cycle-collector decision (trial-deletion vs alternatives) |
| Emery Berger | Allocators; measurement methodology | Keeps bench claims statistically honest |
| Niko Matsakis | Borrow checker, Polonius | Perceus borrow inference = borrow checking inferred, not declared |
| Graydon Hoare | Rust creator (later Swift org) | Taste elder for our implementation language |

## Verification & Red Team (reviews every major landing)

| Seat | Domain | molt arc |
|---|---|---|
| John Regehr (red-team chair) | Csmith, UB hunting | Fuzzing the frontend/TIR; UB audits |
| Xavier Leroy | CompCert | What verified compilation costs; where to spend it |
| Leonardo de Moura | Z3, Lean4 | In-tree Lean4 proofs |
| Daniel Kroening | CBMC → Kani | Kani in CI; model-checking unsafe runtime kernels |
| Simon Peyton Jones | GHC, stream fusion | Generator-fusion keystone lineage (deforestation); research that ships |

## Elders' bench (counsel, not committees)

- David Ungar — Self: origin of "dynamic made fast".
- David Patterson — HW/SW co-design.
- Anders Hejlsberg — TypeScript: the only completed gradual-typing of an entire
  dynamic-language ecosystem; the verified-subset strategy is the TS playbook for Python.
- Empty chair: Fran Allen — every loop optimization in S6 SCEV/CountedLoop
  descends from her work.

Anti-roster: no pure-celebrity seats, no one who treats it as a hall of fame.
The council works or it doesn't exist.

## Operationalization (use today)

1. **Seats as adversarial-review lenses** (the standing recursive-review waves
   get named seats; sharper reviewer prompts):
   - Leijen lens — RC/ownership/double-free (the lens that REJECTED DropInsertion v1)
   - Pall lens — value repr / NaN-box truncation / bigint boundaries
   - Shannon lens — specialization vs CPython semantics (guard completeness)
   - Lopes lens — IR/artifact identity, translation validation
   - Regehr lens — fuzzing, UB, miscompile hunting
   - Rossberg lens — WASM semantic conformance
   - Cox lens — build-graph determinism, caching, DX
   - Boehm lens — memory model / atomics
   - Galindo lens — error-message byte parity across 3.12/13/14
2. **Judge panels for design docs**: spawn N reviewers, each arguing as a
   specific seat ("what would Click say about this deopt design").
3. **Canon routing** — per-arc reading lists (consult the artifact, not vibes):

| molt arc | Canon |
|---|---|
| V1 translation validation | Alive2 (Lopes); CompCert TV lineage (Leroy) |
| M2 lowering-as-data | Cranelift ISLE RFC (Fallin/Sharp); Go cmd/compile ssa .rules; TableGen/ISel |
| Guarded-specialization tier | PEP 659 (Shannon); Self PICs (Hölzle/Chambers/Ungar); V8 feedback vectors (Bak/Egorov); copy-and-patch (Bucher/Xu) |
| Value-format bake-off (#33) | LuaJIT NaN-box (Pall); V8 Smi/pointer compression; JSC JSValue; Julia tagged unions (Bezanson) |
| MM ladder | Perceus paper (Leijen); Immix/MMTk (Blackburn); CPython gc (trial deletion); mimalloc paper; Boehm memory-model papers |
| Generator fusion keystone | Stream fusion/deforestation (Wadler→SPJ); LLVM CoroElide; Halide schedule separation (Ragan-Kelley) |
| molt.gpu | Triton (Tillet); TVM (Chen); tinygrad source (Hotz — the literal contract); FlashAttention/DFlash (Dao et al.) |
| WASM tier | Component model (Wagner); Binaryen/relooper→stackifier (Zakai); WASM spec (Rossberg); wasmtime (Fitzgerald) |
| DX/build | Go build-cache design docs (Cox); cargo profiles/codegen-units; Buck2/Bazel remote-cache lineage |
| Unsafe core | Stacked/tree borrows + Miri (Jung); RustBelt; Kani docs (Kroening lineage) |
| asyncio parity | asyncio/uvloop (Selivanov); Trio structured concurrency (NJ Smith) |
| Verified-subset manifest | TypeScript design lineage (Hejlsberg); pyright conformance (Traut); mypy (Lehtosalo); typing-spec conformance suite |
