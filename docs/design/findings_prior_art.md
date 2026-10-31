# Findings Prior Art

**Status:** Active reference, researched 2026-10-29.
**Owner:** the findings-family workers; the ledger integrator keeps it aligned.

This document maps each root-cause family in the ledger's
[family dependency order](../agent/V1_HANDOFF_FINDINGS.md#family-dependency-order-2026-10-29)
to prior art. Each section gives the problem in Molt's terms, the systems that
solved it, what Molt should take and refuse, and the cheapest experiment that
confirms the choice. A family's design doc cites its section here and extends
it. The ledger rows stay the authority for each defect.

Labels: **[F]** fact, sourced in the same item; **[I]** inference from sourced
facts and the ledger rows; **[H]** hypothesis that an experiment must test.
Source files are pinned to a tag or commit unless only a living document
exists. Every URL was fetched on 2026-10-29.

Every choice must fit Molt: an AOT compiler with CPython as the semantic
oracle, native and WASM as co-equal P0 targets, deterministic by default,
ready for free threading, and the verified subset of
[spec 0215](../spec/areas/compat/contracts/verified_subset_contract.md) with the
dynamic-execution limits of
[spec 0216](../spec/areas/compat/contracts/dynamic_execution_policy_contract.md).

## A. Typed op and runtime-call ABI

**Problem.** Each backend restates op kinds, runtime symbols, ownership and
target legality by hand. One op then means different things per backend
(HF-485, HF-571), and a dead or unsupported op still compiles (HF-197, HF-287,
HF-288).

**Prior art.**
- *LLVM RuntimeLibcalls.* [F] LLVM declares each runtime-library call once in
  TableGen, with a per-target predicate such as `isOSDarwin`. Source:
  https://github.com/llvm/llvm-project/blob/llvmorg-21.1.0/llvm/include/llvm/IR/RuntimeLibcalls.td.
- *MLIR ODS and dialect conversion.* [F] ODS declares operands, results,
  traits, memory effects and verifiers per op. A `ConversionTarget` marks ops
  legal, illegal or dynamically legal, and a full conversion fails while an
  illegal op remains. Sources: https://mlir.llvm.org/docs/DefiningDialects/Operations/,
  https://mlir.llvm.org/docs/DialectConversion/. Cost [I]: dynamic legality is
  code, so no reader can list it without running it.
- *Cranelift and Wasmtime.* [F] The meta crate declares each instruction with
  flags such as `can_trap` and `other_side_effects` and generates the opcode
  enum. `foreach_builtin_function!` lists each runtime builtin once and expands
  to its index and signature. Sources:
  https://github.com/bytecodealliance/wasmtime/blob/v26.0.0/cranelift/codegen/meta/src/shared/instructions.rs,
  https://github.com/bytecodealliance/wasmtime/blob/v26.0.0/crates/environ/src/builtin.rs.
- *CPython's cases generator.* [F] One DSL in `bytecodes.c` generates the
  interpreter and per-op flags (`HAS_ERROR_FLAG`, `HAS_ESCAPES_FLAG`) that the
  optimizer reads. Sources:
  https://github.com/python/cpython/blob/v3.13.0/Tools/cases_generator/interpreter_definition.md,
  https://github.com/python/cpython/blob/v3.13.0/Include/internal/pycore_opcode_metadata.h.
- *PyO3.* [F] The type system separates an owned `Bound<'py, T>` from a
  `Borrowed<'a, 'py, T>`. Source: https://pyo3.rs/v0.22.5/types.html.

**Adopt.** [I] Extend the existing authority, `op_kinds.toml` with
`tools/gen_op_kinds.py`. Each runtime-call op declares its symbol, arity,
argument and result ownership, effects (may raise, may call Python) and
per-target legality. Generate closed Rust enums and every backend's lowering
table, so a missing arm or a different symbol fails to compile. Check legality
before codegen, as `ConversionTarget` does, so `--target rust` and MLIR refuse
with a typed diagnostic. Generate owned and borrowed handle types for the
intrinsic ABI from the same rows (HF-262).

**Refuse.** [I] TableGen or MLIR as the registry engine: a C++ tool adds a
toolchain for a job the TOML generator does. Legality callbacks: keep
legality as data so the support matrix can project it.

**Experiment.** Generate the `dict_set` and `iter` rows (HF-485, HF-571) as a
closed enum, delete one backend's string arm, and confirm the crate fails to
compile. Then run the dict-subclass `__setitem__` and `iter(1)` programs on
native, LLVM and WASM.

## B. Binding-flow resolution

**Problem.** The binding flow matches decorators by spelling (HF-203). After
any callback it assumes every module global may change, so discovery admits
unreachable modules (HF-181, HF-140, HF-126) and calls stay late-bound (HF-79).

**Prior art.**
- *mypyc.* [F] References to functions, types and most attributes in one
  compilation unit use early binding, and monkeypatching compiled definitions
  is not supported. Source:
  https://mypyc.readthedocs.io/en/latest/differences_from_python.html. Cost [F]:
  tests that patch compiled functions break.
- *CPython 3.13 tier-2 optimizer.* [F] It watches the globals and builtins
  dicts, turns `_LOAD_GLOBAL_MODULE` and `_LOAD_GLOBAL_BUILTINS` into constants
  after a function-version check, and on a write invalidates dependent
  executors and unwatches the dict; it re-watches only a bounded number of
  times. Source:
  https://github.com/python/cpython/blob/v3.13.0/Python/optimizer_analysis.c.
- *GraalVM Native Image.* [F] A closed-world points-to analysis from the entry
  points decides which code is reachable. Source:
  https://www.graalvm.org/latest/reference-manual/native-image/basics/.
  Cost [I]: the analysis over-approximates; see family G.
- *Bun `--aot`* (merged 2026-10-27, experimental). [F] A program with no
  reachable `eval` is closed. Existing members of built-in objects are
  read-only, so `arr.push(x)` compiles to the operation. A value the build
  cannot see is tested once, and a failed test takes a slow version. Source:
  https://github.com/oven-sh/bun/pull/45631. Cost [F]: replacing a built-in
  member throws `TypeError`.

**Adopt.** [I] Resolve decorators, classes and calls to binding identities
(`ClassId`, qualified names), as mypyc does. Replace the worst-case callback
rule with an optimistic premise: only the owning module writes its globals.
Verify the premise over the closure and re-scan only where it fails (HF-140's
next step). Keep runtime binding guards (HF-79) where the proof cannot decide,
in the CPython watcher style: guard, invalidate, stop after repeated writes.
`docs/design/verified_namespace_premises.md` already owns the premise design.

**Refuse.** [I] Bun's read-only built-ins: CPython allows those writes, and
spec 0216 would need an explicit rationale. Prove builtin exactness from the
whole program instead (HF-181).

**Experiment.** Build HF-203's three repro programs and an `import json`
program with the owner-only premise. Compare admitted module counts and
differential results before and after.

## C. Proof coverage

**Problem.** Proof cells are named by hand. Whole trees, feature-gated Cargo
targets and feature sets therefore run in no cell (HF-73, HF-99, HF-565,
HF-314), Lean modules no Lake root reaches go unbuilt (HF-63), and lint masks
hide unreached code (HF-87, HF-178). Which cells a change selects, and whether
a passing result is reused, belongs to F (2026-10-30 re-cut).

**Prior art.**
- *Bazel.* [F] Tests are targets with declared dependencies;
  `rdeps(universe, set)` returns every target that depends on a changed one.
  Source: https://bazel.build/query/language. Cost [I]: every edge needs a
  BUILD declaration.
- *Pants.* [F] Pants infers Python dependencies from imports and selects with
  `--changed-since` and `--changed-dependents=transitive`. Sources:
  https://www.pantsbuild.org/stable/docs/using-pants/key-concepts/targets-and-build-files,
  https://www.pantsbuild.org/stable/docs/using-pants/advanced-target-selection.
  Cost [I]: an import by computed string needs an explicit edge.
- *LLVM lit.* [F] Every file in a suite directory is a test. `REQUIRES:` and
  `UNSUPPORTED:` give an explicit UNSUPPORTED result, not a silent omission.
  Source: https://llvm.org/docs/CommandGuide/lit.html.
- *cargo-hack and cargo-nextest.* [F] `--each-feature` and `--feature-powerset`
  test feature combinations; nextest runs each test in its own process.
  Sources: https://github.com/taiki-e/cargo-hack,
  https://nexte.st/docs/design/why-process-per-test/.
- *pytest-testmon and Google TAP.* [F] testmon selects tests from per-test
  coverage (https://testmon.org/); TAP selects affected tests from the build
  graph (https://research.google/pubs/taming-google-scale-continuous-testing/).
  Cost [I]: coverage sees Python only, not Rust, generated or data inputs.

**Adopt.** [I] Derive cell membership from graphs Molt already has.
`cargo metadata` gives each `[[test]]` with its `required-features`; a cell
must enable them, and each declared feature such as `free-threaded` gets a
cell. Add the Python import graph and the generator graph. A file that no cell
can run reports UNSUPPORTED with a reason, as in lit. Select by reverse
dependencies of the changed files.

**Refuse.** [I] Coverage-based selection as a gate: it misses Rust and data
inputs. A Bazel migration: Molt needs the graph, not the tool.

**Experiment.** For the last 20 main commits, compute reverse dependencies from
`cargo metadata` and the import graph, and compare them with the globs'
selections. Count what no glob selected; HF-565's seven targets must appear.

[I] Seven rows have other causes: Lean, ASan and free-threaded GC defects
that a running cell exposes (HF-63, HF-66, HF-186, HF-484), and lint-mask debt
(HF-87, HF-178, HF-533).

## D. Verdict truth (executable ledger)

**Problem.** Expected failures live in calibrated snapshots and prose. A
build-time refusal cannot be pinned (HF-563), regressions stay invisible
(HF-295, HF-215), and infrastructure failures count as compiler failures
(HF-388, HF-422).

**Prior art.**
- *LLVM lit.* [F] `XFAIL:` sits in the test file. An expected failure that
  passes is XPASS and fails the run; UNRESOLVED means no outcome could be
  determined. Source: https://llvm.org/docs/CommandGuide/lit.html.
- *rustc compiletest.* [F] `//@ check-fail`, `//@ build-fail` and
  `//@ run-fail` name the phase that must fail; `//~ ERROR` binds an expected
  diagnostic to a line. Source: https://rustc-dev-guide.rust-lang.org/tests/ui.html.
- *GraalPy tag files.* [F] GraalPy runs CPython's unittests through per-test,
  per-platform tag files of tests known to pass; `mx graalpytest --retag`
  regenerates them. Source:
  https://github.com/oracle/graalpython/blob/graal-24.2.0/docs/contributor/CONTRIBUTING.md.
  Cost [I]: a tag records a pass, not why a test fails.
- *Bazel test protocol.* [F] A test reports infrastructure failures through
  `TEST_INFRASTRUCTURE_FAILURE_FILE`, apart from its own result. `flaky = True`
  reruns a test up to three times, and the docs discourage it. Sources:
  https://bazel.build/reference/test-encyclopedia,
  https://bazel.build/reference/be/common-definitions.
- *Csmith and C-Reduce.* [F] Random differential testing finds compiler bugs,
  and C-Reduce minimizes each failing program. Sources:
  https://github.com/csmith-project/csmith (paper:
  https://www.cs.utah.edu/~regehr/papers/pldi11-preprint.pdf),
  https://github.com/csmith-project/creduce.

**Adopt.** [I] Put a typed expected outcome next to each program, extending
`MOLT_META` and `tools/finding_coverage.py`: pass; run failure (exit code and
stdout digest); build failure (phase and diagnostic code); unsupported cell;
infrastructure. XPASS fails the lane. Each expected failure names its ledger
row. Replace dated snapshots with a retag tool that records the commit and the
interpreter. Only infrastructure outcomes retry.

**Refuse.** [I] Retry-until-pass for compiler outcomes: it hides races. An
expected failure as a closure: the ledger forbids waivers.

**Experiment.** Pin HF-557's three build refusals as build failures with code
`MOLT-FE007`, then confirm the pin reports XPASS on a tree with the fix.

## E. Performance verdict truth

**Problem.** The regression gate never armed (HF-222), wall time is the only
axis (HF-459), and a small delta cannot be told apart from code placement
(HF-245).

**Prior art.**
- *rustc-perf.* [F] Triage reads instruction count, max RSS and binary size,
  and notes that RSS "is much more variable than instruction count". A change
  is significant when it falls outside an interquartile-range fence over
  historical results. Sources:
  https://github.com/rust-lang/rustc-perf/blob/15d95905429e577d5a710b17c49e04b4451c263b/triage/README.md,
  https://github.com/rust-lang/rustc-perf/blob/15d95905429e577d5a710b17c49e04b4451c263b/docs/comparison-analysis.md.
- *benchstat and Criterion.rs.* [F] benchstat compares samples with a
  Mann-Whitney U test and reports no change when the difference is not
  significant. Criterion bootstraps confidence intervals against a saved
  baseline. Sources: https://pkg.go.dev/golang.org/x/perf/cmd/benchstat,
  https://bheisler.github.io/criterion.rs/book/analysis.html.
- *Layout bias.* [F] Mytkowicz et al. showed that environment size and link
  order can reverse a conclusion (https://doi.org/10.1145/1508244.1508275).
  Stabilizer re-randomizes code, stack and heap layout so that a comparison
  becomes a statistical test
  (https://people.cs.umass.edu/~emery/pubs/stabilizer-asplos13.pdf).
- *Counters.* [F] XNU's `rusage_info_v4` carries `ri_instructions` and
  `ri_cycles`. Source:
  https://github.com/apple-oss-distributions/xnu/blob/xnu-11215.1.10/bsd/sys/resource.h.

**Adopt.** [I] Commit the armed baseline per main commit as data, as rustc-perf
keeps results per artifact. Gate on retired instructions, which are nearly
deterministic within one toolchain and host class. Report wall time with a U
test and a minimum sample count. Derive each benchmark's threshold from its
own history. Read instructions with `proc_pid_rusage` on macOS and
`perf_event_open` on Linux. `docs/design/optimization_evidence_registry.md`
owns rejected-verdict context (HF-460).

**Refuse.** [I] One fixed percentage for every benchmark; gates on wall time
from shared CI runners.

**Experiment.** Count instructions for HF-245's generator `next()` loop on
both commits. Equal counts with a wall-time change point to placement; relink
with the lane's symbol order to confirm.

## F. Observed inputs and content identity

**Problem.** Molt keys artifacts by checkout path, module name or ambient
environment. Cargo reports changed libraries Fresh (V1-01), objects differ per
checkout (HF-372), checkouts displace each other's runtime (HF-473), and a
loader runs an older body (V1-15). The same missing fact, what a build, test
or proof reads and the identity of that content, is restated in about ten
places: source walks that hash debris (HF-590), records keyed by the device
number (HF-604), undeclared environment inputs (HF-591, HF-522), hand-written
closure lists and globs (HF-601, HF-433), and static whole-module reachability
that selects 983 of 1008 test files for one hub change and reuses nothing
(HF-605, HF-585, HF-334, HF-35). [I] One observed-input, content-keyed model
serves build keys, test selection and reuse, and proof custody alike; C's
Bazel `rdeps` item is the selection half, and HF-605's research adds the
test-result caches (Go's test cache, Buck2, pytest-testmon).

**Prior art.**
- *Bazel Remote Execution API.* [F] An `Action` is the digest of its `Command`
  plus the Merkle digest of its input root. Source:
  https://github.com/bazelbuild/remote-apis/blob/v20250211/build/bazel/remote/execution/v2/remote_execution.proto.
- *Go build IDs.* [F] Each package and binary carries an action ID (hash of
  inputs) and a content ID (hash of output); `toolID` changes when the
  compiler binary changes. Source:
  https://github.com/golang/go/blob/go1.23.0/src/cmd/go/internal/work/buildid.go.
- *Nix.* [F] A store path embeds a hash of its build inputs, so generations
  never overwrite each other. Sources:
  https://nixos.org/manual/nix/stable/store/store-path.html,
  https://edolstra.github.io/pubs/phd-thesis.pdf. Cost [I]: hashing the whole
  environment over-invalidates.
- *Path remapping.* [F] rustc's `--remap-path-prefix` and the C compilers'
  prefix maps replace build paths in outputs. Sources:
  https://doc.rust-lang.org/rustc/remap-source-paths.html,
  https://reproducible-builds.org/docs/build-path/.
- *Content freshness.* [F] PEP 552 hash-based `.pyc` files record a source
  hash that a checked loader compares. Cargo's unstable `-Zchecksum-freshness`
  replaces mtimes with checksums. Sources: https://peps.python.org/pep-0552/,
  https://doc.rust-lang.org/cargo/reference/unstable.html#checksum-freshness.
- *Test-consumed executables.* [F] Cargo builds each binary target before an
  integration test and passes its path in `CARGO_BIN_EXE_<name>`; Bazel builds
  a test's `data` dependencies and exposes them through runfiles. A test never
  finds an executable by probing a path. Sources:
  https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates,
  https://bazel.build/reference/be/common-definitions#typical.data.

**Adopt.** [I] One identity type: the digest of declared inputs, with tools
identified by content, as Go does, not by `PATH` text (HF-385). Store
generations immutably by digest and let each checkout select its own (HF-473,
HF-244). Remap checkout paths to one prefix in every emitted path (HF-372).
Bind a loader to the admitted body by source hash, as a checked `.pyc` does
(V1-15). `docs/design/cargo_input_authority.md` and
`docs/design/build_early_cutoff.md` own the Cargo and build legs.

**Refuse.** [I] Hashing the whole environment: `PATH`-only differences already
cause rebuilds (HF-385). Freshness by mtime alone: it reported Fresh for
changed content (V1-01).

**Experiment.** Build one commit in two checkouts with path remapping and
compare the native objects with `cmp` (HF-372). Equal bytes prove the path was
the only difference.

## G. Fabricated support surfaces

**Problem.** Stubs link and fail only at run time: 380 stub modules (HF-273),
seven runtime feature families (HF-195), a `posixpath` stub under working
modules (HF-123), and FFI and GPU paths that report success for no work
(V1-17, V1-11).

**Prior art.**
- *Zig.* [F] `@compileError` fails the build only "when semantically
  analyzed"; code that nothing reaches is not analyzed. Source:
  https://ziglang.org/documentation/0.13.0/#compileError.
- *GraalVM Native Image.* [F] GraalVM for JDK 23 (24.1.0) made
  `--report-unsupported-elements-at-runtime` the default and deprecated the
  option: an unsupported element now fails at first use, not at build.
  Sources: https://github.com/oracle/graal/blob/vm-24.1.0/substratevm/CHANGELOG.md,
  https://github.com/oracle/graal/pull/8815. [I] Points-to reachability
  over-approximates, so build errors fired on code that never ran.
- *CPython on WebAssembly and Pyodide.* [F] CPython documents that modules
  using processes, threads, networking or signals are unavailable or limited
  on WebAssembly; Pyodide removes or unvendors modules it cannot support.
  Sources: https://docs.python.org/3/library/intro.html#webassembly-platforms,
  https://pyodide.org/en/stable/usage/wasm-constraints.html.
- *Rust `std` on unsupported targets* (counterexample). [F]
  `sys/pal/unsupported` compiles the full API and returns an `unsupported`
  error when called. Source:
  https://github.com/rust-lang/rust/blob/1.82.0/library/std/src/sys/pal/unsupported/common.rs.
  Cost [I]: the program builds and fails only when it runs.

**Adopt.** [I] Replace stub code with a typed unsupported declaration per
module, attribute or feature and target cell. Reaching one from the admitted
closure is a build error with the support-matrix diagnostic, like Zig's lazy
`@compileError`. Where reachability stays imprecise, raise CPython's own
failure, such as `ModuleNotFoundError` at import, not a stub that imports and
fails later.

**Refuse.** [I] Success-returning substitutes (V1-17). Build-time refusal over
an imprecise closure: GraalVM's reversal shows its cost. So G also depends on
B (precise reachability), an edge the ledger does not list.

**Experiment.** Declare `posixpath` unsupported and build a
`mimetypes.guess_type` program (HF-123); expect a refusal that names the reach
path. Then count how many of the 380 stubs the differential corpus reaches.

## H. Import system and stdlib surface

**Problem.** Molt's import machinery and stdlib miss behavior that CPython's
own tests reach: frozen precedence (HF-558), private importlib helpers
(HF-559), package resources (HF-383), run-time test discovery (HF-578) and two
module stores (HF-276).

**Prior art.**
- *CPython frozen modules.* [F] CPython 3.12 freezes its startup modules,
  including `_sitebuiltins`, `os`, `site`, `codecs`, `io` and `posixpath`. The
  default `sys.meta_path` holds a built-in finder, a frozen finder and the
  path-based finder. Sources:
  https://github.com/python/cpython/blob/v3.12.0/Tools/build/freeze_modules.py,
  https://docs.python.org/3/reference/import.html.
- *PyInstaller hooks.* [F] A hook declares `hiddenimports`;
  `collect_submodules(package)` lists submodules at build time for imports by
  computed name. Source: https://pyinstaller.org/en/stable/hooks.html.
  Cost [I]: per-package hooks drift from their packages.
- *PyOxidizer.* [F] `OxidizedFinder` imports modules and serves resources from
  memory through the importlib APIs. Source:
  https://gregoryszorc.com/docs/pyoxidizer/main/oxidized_importer.html. The
  reader contract is `TraversableResources`
  (https://docs.python.org/3/library/importlib.resources.abc.html).
- *Bun `--aot`.* [F] `import()` and `require()` of a computed path admit the
  files that match the path's constant parts. Source:
  https://github.com/oven-sh/bun/pull/45631.
- *CPython sources as oracle.* [F] `_get_sourcefile` and the zip name rule
  (UTF-8 when flag 0x800 is set, else cp437) are in 3.12's pure-Python
  sources. Sources:
  https://github.com/python/cpython/blob/v3.12.0/Lib/importlib/_bootstrap_external.py,
  https://github.com/python/cpython/blob/v3.12.0/Lib/zipfile/__init__.py.

**Adopt.** [I] Generate the frozen set per CPython version from
`freeze_modules.py` and give it precedence in the import authority (HF-558).
Compile the target version's pure-Python importlib helpers rather than writing
substitutes (HF-559). Admit discovered packages through one typed policy
modeled on `collect_submodules` (HF-578). Serve resources through
`TraversableResources` over the compiled payload (HF-383).
`docs/design/foundation/import_bedrock_frozen_module_layer.md` owns the store.

**Refuse.** [I] Run-time hook code that patches modules, and any pretence of a
module the build did not admit.

**Experiment.** Admit `test.test_importlib` by the submodule policy and run it
compiled. Count subpackages that reach their test cases (0 of 20 today).

## I. Frame, exception and coroutine model

**Problem.** Recursion has no stack guard on native or WASM (HF-442). Native code
calls the runtime per statement to track lines (HF-261). A user raise and
catch is about 150 times slower than CPython (HF-561). Suspended frames box
every live value (HF-229).

**Prior art.**
- *CPython exception and location tables.* [F] Since 3.11 a `try` costs
  nothing until something raises; handlers come from an exception table.
  Locations are a compact varint table read only when a traceback needs one.
  Sources: https://github.com/python/cpython/blob/v3.12.0/Objects/exception_handling_notes.txt,
  https://github.com/python/cpython/blob/v3.12.0/Objects/locations.md.
  PEP 669 instruments a code object only for events a tool enables
  (https://peps.python.org/pep-0669/).
- *CPython lazy frames.* [F] The interpreter runs on `_PyInterpreterFrame` and
  creates a `PyFrameObject` only when Python code needs one. Source:
  https://github.com/python/cpython/blob/v3.14.0/InternalDocs/frames.md.
- *Stack limits.* [F] CPython 3.14 compares the stack pointer with soft and
  hard limits from the OS stack bounds; 3.12 counted C calls. HotSpot keeps
  yellow, red and reserved guard zones per thread. Sources:
  https://github.com/python/cpython/blob/v3.14.1/InternalDocs/stack_protection.md,
  https://github.com/openjdk/jdk/blob/jdk-21-ga/src/hotspot/share/runtime/stackOverflow.hpp.
  Cost [F]: 3.14.0's check broke stack-switching libraries
  (https://discuss.python.org/t/python-3-14-0-is-incompatible-with-stack-switching-systems-what-do-we-do/104880).
- *WASM.* [F] Wasmtime bounds guest stack with `Config::max_wasm_stack` and
  traps on overflow. Source:
  https://docs.wasmtime.dev/api/wasmtime/struct.Config.html#method.max_wasm_stack.
- *Coroutine frames.* [F] LLVM keeps only values live across a suspend point
  in the coroutine frame. Source: https://llvm.org/docs/Coroutines.html.
  `docs/design/typed_suspension_state_machines.md` maps the rest.

**Adopt.** [I] Emit one static PC-to-location table per function, decoded at
traceback time, and delete the per-statement calls (HF-261). WASM code cannot
read its PC, so store a call-site index before calls that may raise [H: cheaper
than today's calls]. Check the stack pointer against OS bounds on native;
derive a depth budget from the host stack on WASM. Profile HF-561 for frame
materialization first.

**Refuse.** [I] Releasing named locals at last use: HF-548 cites CPython, but
CPython keeps a named local until the frame ends or the name is rebound. Early
release changes `__del__` and weakref timing; it is right only for compiler
temporaries.

**Experiment.** Run HF-561's raise loop under the wall sampler and count frame
materializations. If frames dominate, defer them and measure again.

## J. Runtime execution context

**Problem.** Compiled code enters the runtime through unchecked names and
repeated thread-local lookups: several TLS reads per entry (HF-198), an opaque
call per refcount (HF-205), heap literals rebuilt per call (HF-507) and four
symbol-name bridges (HF-206).

**Prior art.**
- *CPython thread state.* [F] CPython 3.12 keeps the current thread state in
  one thread-local, `_Py_tss_tstate`, and internal functions take `tstate` as
  an argument. Source:
  https://github.com/python/cpython/blob/v3.12.0/Include/internal/pycore_pystate.h.
- *Context registers and arguments.* [F] HotSpot reserves `r15_thread` on
  x86-64 and `rthread` (x28) on AArch64; V8 reserves `kRootRegister` (r13 on
  x64); Wasmtime passes a `VMContext` pointer to compiled functions. Sources:
  https://github.com/openjdk/jdk/blob/jdk-21-ga/src/hotspot/cpu/x86/assembler_x86.hpp,
  https://github.com/openjdk/jdk/blob/jdk-21-ga/src/hotspot/cpu/aarch64/assembler_aarch64.hpp,
  https://github.com/v8/v8/blob/12.0.1/src/codegen/x64/register-x64.h,
  https://docs.wasmtime.dev/contributing-architecture.html.
- *Inline reference counts.* [F] PEP 683 immortal objects let `Py_INCREF`
  skip the write; PEP 703 biased counting lets the owner thread use a
  non-atomic local count. Sources: https://peps.python.org/pep-0683/,
  https://peps.python.org/pep-0703/.
- *Explicit service context.* [F] HPy passes an `HPyContext` to every API call
  instead of resolving global symbols. Source:
  https://docs.hpyproject.org/en/latest/overview.html. CPython holds literals
  once per code object in `co_consts`
  (https://docs.python.org/3/reference/datamodel.html#code-objects).

**Adopt.** [I] Pass one execution-context pointer through compiled code and
runtime entry as an argument, as CPython and Wasmtime do. Stamp it with the
runtime generation so one check covers every per-thread cache (HF-200).
Generate inline refcount fast paths from `molt-codegen-abi`: immortal check,
plain add, owner-thread check under free threading. Replace the bridges with
generated typed service tables (HF-206). Materialize literals once per code
object (HF-507).

**Refuse.** [I] A pinned context register before measurement: it costs a
register in every function and differs per target.

**Experiment.** Count TLS accessor calls per runtime entry in HF-198's call
loop on macOS, pass the context, and count again; wall time must follow.

**Design.** [docs/design/foundation/call_anatomy.md](foundation/call_anatomy.md) measures the
call, frame, attribute, constructor and raise paths against CPython 3.12 and stages the
execution context, frame records and compile-time binding with per-stage instruction budgets.

## K. Runtime ownership and resources

**Problem.** Runtime ownership is convention: arena words retained twice
(V1-14), charges bound to threads (V1-19), auto-interned strings never retired
(V1-20), quadratic dict deletion (V1-27), and native state in global handle
maps with aliasing `&mut` (HF-208).

**Prior art.**
- *CPython object lifecycle.* [F] Each type owns `tp_dealloc`, `tp_traverse`
  and `tp_clear`; PEP 442 adds `tp_finalize` so objects in cycles finalize
  safely. The C API states which calls steal a reference and which borrow.
  Sources: https://docs.python.org/3/c-api/gcsupport.html,
  https://peps.python.org/pep-0442/,
  https://docs.python.org/3/c-api/intro.html#reference-count-details.
- *Native layouts.* [F] A PyO3 `#[pyclass]` struct lives inside its Python
  object and joins the GC through `__traverse__` and `__clear__`; CPython's
  `_decimal` embeds `mpd_t` in `PyDecObject`. Sources:
  https://pyo3.rs/v0.22.5/class/protocols.html,
  https://github.com/python/cpython/blob/v3.12.0/Modules/_decimal/_decimal.c.
- *CPython dicts and interning.* [F] Deletion leaves a `DKIX_DUMMY` tombstone
  in the index table; resizing happens on insert. CPython interns names used
  in programs and strings passed to `sys.intern`. Sources:
  https://github.com/python/cpython/blob/v3.12.0/Objects/dictobject.c,
  https://docs.python.org/3/library/sys.html#sys.intern.
- *cgroup v2 memory ownership.* [F] A memory area is charged to the cgroup
  that created it and stays charged until it is released, even if the process
  moves. Source: https://docs.kernel.org/admin-guide/cgroup-v2.html#memory-ownership.

**Adopt.** [I] Declare native layouts with drop and traverse through
`gen_heap_kinds.py`, then delete the handle maps and their `__del__` methods
(HF-208). Record the charged owner on each allocation and debit that owner on
release, as memcg does (V1-19). Delete with tombstones and compact on resize
(V1-27). Intern only what CPython interns (V1-20). Follow the steal and borrow
conventions in aggregate builders (V1-14).

**Refuse.** [I] Global maps keyed by integer handles: they outlive owners.

**Experiment.** Insert n keys into a dict and delete them in insertion order
for n = 10^4, 10^5 and 10^6. Time per delete must stay flat, as in CPython 3.12.

## L. C-API boundary

**Problem.** The C-API bridge keeps two error channels, reconciled at 269
sites, and locks sharded maps on most crossings (HF-209). It keeps two
`Python.h` headers (HF-283, HF-61) and stubs warning entry points (HF-527).

**Prior art.**
- *PyPy cpyext.* [F] PyPy's authors describe the cost of C-API emulation:
  objects need a counterpart on each side and a lookup between them at each
  crossing. Source:
  https://www.pypy.org/posts/2018/09/inside-cpyext-why-emulating-cpython-c-8083064623681286567.html.
- *CPython 3.12's error indicator.* [F] 3.12 keeps the raised exception
  normalized as one object and adds `PyErr_GetRaisedException` and
  `PyErr_SetRaisedException`. Sources:
  https://docs.python.org/3.12/whatsnew/3.12.html,
  https://docs.python.org/3/c-api/exceptions.html#c.PyErr_GetRaisedException.
- *CPython's stable ABI manifest.* [F] `Misc/stable_abi.toml` lists each
  function, data symbol and struct of the limited API once, for
  `Tools/build/stable_abi.py` to read. Source:
  https://github.com/python/cpython/blob/v3.12.0/Misc/stable_abi.toml.
- *HPy.* [F] Extensions use handles and an explicit context; debug mode finds
  leaked and closed handles. Sources:
  https://docs.hpyproject.org/en/latest/overview.html,
  https://docs.hpyproject.org/en/latest/debug-mode.html. Cost [I]: each
  extension must port.

**Adopt.** [I] One runtime-owned exception slot that `PyErr_*` reads and
writes directly, like CPython 3.12's single object. O(1) views for managed
objects through a header flag; maps only for foreign objects. One declarative
manifest, like `stable_abi.toml`, generates both headers, the exports and the
data symbols (HF-283, HF-61). Route `PyErr_WarnExplicit*` to the runtime's
`warn_explicit`, as `Python/_warnings.c` does
(https://github.com/python/cpython/blob/v3.12.0/Python/_warnings.c).

**Refuse.** [I] HPy as the v1.0 extension ABI: the target extensions use the
CPython API. Its debug-mode checks are worth copying into a checked build.

**Experiment.** A C extension raises and clears an error 10^6 times and reads
list items. Count bridge map lookups and time before and after the single slot.

## M. Codec, text and IO state

**Problem.** Registered codec error handlers are stored but never called, and
decoder tables stay borrowed across decoding (V1-18). Incremental BZ2 and LZMA
replay input (V1-30). The VFS adapter publishes only at close (V1-33). Regex
lacks bounded, interruptible execution (HF-329).

**Prior art.**
- *PEP 293.* [F] An encoder or decoder calls the registered handler with a
  `UnicodeError`; the handler returns a replacement and a resume position, or
  raises. Source: https://peps.python.org/pep-0293/.
- *CPython `_pyio`.* [F] The pure-Python `TextIOWrapper` rebuilds `tell()`
  from the decoder's `getstate()` snapshot. Source:
  https://github.com/python/cpython/blob/v3.12.0/Lib/_pyio.py.
- *Bounded decompression.* [F] `LZMADecompressor.decompress(data, max_length)`
  returns at most `max_length` bytes, keeps the rest and sets `needs_input`.
  Source: https://docs.python.org/3/library/lzma.html#lzma.LZMADecompressor.decompress.
- *Regex engines.* [F] CPython's `_sre` checks signals every 4,096 steps.
  fancy-regex hands patterns without backtracking features to the linear-time
  `regex` crate, whose `LeftmostFirst` match kind gives Perl-style priority.
  Sources: https://github.com/python/cpython/blob/v3.12.0/Modules/_sre/sre_lib.h,
  https://github.com/fancy-regex/fancy-regex/blob/0.20.0/README.md,
  https://docs.rs/regex-automata/0.4.9/regex_automata/enum.MatchKind.html.

**Adopt.** [I] Call handlers by the PEP 293 protocol with no table borrow held
across the call: copy state out, call, then reacquire. Keep decoder state in
the decoder object, not in thread-locals. Keep decompressor state between
calls and honor `max_length`. Use `_pyio` as a differential oracle. Keep
CPython's backtracking semantics with step-counted signal checks and a
resource budget. A linear engine is an optimization only for patterns proven
equivalent [H: captures agree].

**Refuse.** [I] Replaying compressed prefixes; publishing file contents only
at close.

**Experiment.** Run a codec handler that re-enters `codecs.decode` from the
callback. Then run `re.match(r'(a*)*b', 'a' * 30)` with a SIGALRM handler that
raises, on native and WASM.

## N. OS, process and network boundary

**Problem.** A compiled binary runs `[sys.executable, "-c", src]` as itself and
forks without end (HF-519). It keeps inherited SIGPIPE and SIGXFSZ (HF-441),
mishandles closed standard descriptors (HF-455), returns raw WASI errnos
(HF-518) and has no Windows poller (HF-177).

**Prior art.**
- *CPython startup.* [F] `signal_install_handlers` ignores `SIGPIPE` and
  `SIGXFSZ`; `create_stdio` returns `None` for an invalid descriptor; PEP 446
  makes new descriptors non-inheritable. `sys.executable` may be empty or
  `None` when CPython has no executable path. Sources:
  https://github.com/python/cpython/blob/v3.12.0/Modules/signalmodule.c,
  https://github.com/python/cpython/blob/v3.12.0/Python/pylifecycle.c,
  https://peps.python.org/pep-0446/,
  https://docs.python.org/3/library/sys.html#sys.executable.
- *Frozen applications.* [F] PyInstaller explains that multiprocessing spawns
  `sys.executable -c`, so a frozen app calls `freeze_support()`. Nuitka detects
  `-c` and `-m` and sets a launch token in the environment to stop
  self-execution fork bombs. Sources:
  https://pyinstaller.org/en/stable/common-issues-and-pitfalls.html,
  https://nuitka.net/user-documentation/common-issue-solutions.html.
- *Windows readiness.* [F] mio polls sockets through the AFD driver
  (`IOCTL_AFD_POLL`) with an IOCP. wepoll does the same and says `select` and
  `WSAPoll` do not scale and have other issues. Sources:
  https://github.com/tokio-rs/mio/blob/v1.0.2/src/sys/windows/afd.rs,
  https://github.com/piscisaureus/wepoll/blob/0598a791bf9cbbf480793d778930fc635b044980/README.md.
- *WASI sockets.* [F] `wasi:sockets` defines the WASI preview 2 socket
  interfaces. Source: https://github.com/WebAssembly/wasi-sockets.

**Adopt.** [I] Copy CPython's startup contract exactly: signals, stdio,
close-on-exec, and runtime descriptors at 3 or above. Decide the
`sys.executable` contract with its consumers: recognize `-c` and `-m` argv and
refuse them with a typed diagnostic, and serve multiprocessing spawn through a
declared entry (Nuitka's guard with PyInstaller's `freeze_support`). Map WASI
errnos through one table to CPython exceptions. Build the Windows poller on
the AFD design; the ledger's `WSAPoll` choice (HF-177) needs this comparison.

**Refuse.** [I] Silent re-execution of `main`.

**Experiment.** Rebuild HF-519's probe with the argv guard and count processes
after 10 s. The count must stay at two or fewer.

## O. Versioned target facts

**Problem.** Generators and the runtime read facts from the build host, not
from the declared Python version and target: StringPrep and Unicode tables
(V1-22), errno (V1-23), one `unittest` tree for all versions (HF-122),
`sys.abiflags` on free-threaded builds (HF-568) and WASI `uname` (HF-567).

**Prior art.**
- *CPython Unicode tables.* [F] `makeunicodedata.py` pins `UNIDATA_VERSION`
  (15.0.0 in 3.12) and also builds the 3.2.0 database. `stringprep` imports
  `ucd_3_2_0`, but `map_table_b3` calls `str.lower()`, which follows the
  running interpreter's Unicode version. Sources:
  https://github.com/python/cpython/blob/v3.12.0/Tools/unicode/makeunicodedata.py,
  https://github.com/python/cpython/blob/v3.12.0/Lib/stringprep.py.
  [I] CPython's own StringPrep output thus depends on the Python version, and
  Molt's generator (`tools/gen_stringprep_tables.py`) calls the host
  `stringprep`; this confirms V1-22's cause.
- *Go `x/sys/unix`.* [F] Per-OS, per-architecture constant files
  (`zerrors_*`) are generated from each target's own headers inside
  containers, committed, and selected by build tags. Source:
  https://github.com/golang/sys/blob/v0.26.0/unix/README.md.
- *Target-version analysis.* [F] mypy evaluates `sys.version_info` checks
  against `--python-version`, not the running interpreter;
  `ast.parse(feature_version=...)` is best-effort. PEP 720 records
  cross-compilation failures from host `sysconfig`. Sources:
  https://mypy.readthedocs.io/en/stable/command_line.html,
  https://docs.python.org/3/library/ast.html#ast.parse,
  https://peps.python.org/pep-0720/.
- *Declared target facts.* [F] PEP 703 builds carry ABI tag `t`;
  `_osx_support` takes the macOS platform from `MACOSX_DEPLOYMENT_TARGET`;
  wasi-libc's `uname` returns `wasi`, `(none)` and `0.0.0`. Sources:
  https://peps.python.org/pep-0703/,
  https://github.com/python/cpython/blob/v3.12.0/Lib/_osx_support.py,
  https://github.com/WebAssembly/wasi-libc/blob/wasi-sdk-24/libc-top-half/musl/src/misc/uname.c.

**Adopt.** [I] Generate each fact table per Python version and target from
that version's CPython sources or that target's headers, pin it, and select it
by the declared coordinate, as Go does. Oracles such as `str.lower` run on the
declared version's interpreter, which the output identity names. Publish build
facts (free threading, deployment target) from the runtime's own features.
`docs/design/sysconfig_build_values.md` owns the sysconfig leg.

**Refuse.** [I] A generator that reads the host interpreter without naming its
version in the output identity.

**Experiment.** Run `tools/gen_stringprep_tables.py` under 3.12, 3.13 and 3.14
and diff the B.3 tables. The diff must equal the `str.lower` differences
between those versions.

## P. Mid-end fact soundness and pass firing

**Problem.** Rewrites run without checked preconditions (HF-204, HF-214).
Facts such as `bce_safe` outlive the pass that proved them, memory SSA keeps a
second block order (HF-555), and guard passes exit early on nearly every
function (HF-573) or skip the stdlib (HF-572).

**Prior art.**
- *Alive2.* [F] Alive2 uses an SMT solver to check that optimized LLVM IR
  refines the original. Sources: https://github.com/AliveToolkit/alive2,
  https://web.ist.utl.pt/nuno.lopes/pubs/alive2-pldi21.pdf.
- *Cranelift.* [F] The e-graph mid-end holds only pure operations;
  side-effecting instructions stay in the control-flow skeleton. Crocus
  verifies ISLE lowering rules with SMT. Sources:
  https://github.com/bytecodealliance/rfcs/blob/main/accepted/cranelift-egraph.md,
  https://doi.org/10.1145/3617232.3624862.
- *LLVM pass manager.* [F] A pass returns `PreservedAnalyses`, and the manager
  invalidates the rest. `-pass-remarks-missed` reports where a pass did not
  fire. MemorySSA builds on the function's dominator tree. Sources:
  https://llvm.org/docs/NewPassManager.html, https://llvm.org/docs/Remarks.html,
  https://llvm.org/docs/MemorySSA.html.

**Adopt.** [I] Declare purity per op in `op_kinds.toml`, and make
`gen_op_kinds` reject a rewrite on an impure op that has no type guard
(HF-204's `And(x, true)`). Hold proofs such as `bce_safe` as invalidated
analyses, not op attributes. Fold Python integers with arbitrary-precision or
checked arithmetic, never i64 wrapping (HF-214). Check integer rewrite rules
with SMT, as Alive2 does. Count applied and missed rewrites per pass and
function, and test that loop benchmarks fire them (HF-573). Derive memory SSA
order from the canonical CFG (HF-555).

**Refuse.** [I] Bail-outs on exception regions as a permanent state, and a
stdlib skip with no stated exit.

**Experiment.** Add the purity check to the generator and list every rule it
rejects. Each rejection is a latent bug or a missing guard.

## Q. User compile and edit latency

**Problem.** An unchanged rebuild costs 34 s (HF-250). Warm builds re-derive
runtime and backend identities (HF-252), probe tools again (HF-357) and resend
whole-program IR (HF-516), and the frontend fingerprint over-invalidates
(HF-254).

**Prior art.**
- *rustc and salsa.* [F] rustc's red-green algorithm re-runs a query only when
  an input changed, and stops when a recomputed result keeps its fingerprint.
  salsa "backdates" an unchanged result so its dependents stay valid. Sources:
  https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html,
  https://salsa-rs.github.io/salsa/reference/algorithm.html.
- *Build-system theory.* [F] Mokhov, Mitchell and Peyton Jones define early
  cutoff and verifying traces (https://doi.org/10.1145/3236774). Bazel calls
  early cutoff change pruning; ninja's `restat` gives it per rule. Sources:
  https://bazel.build/reference/skyframe, https://ninja-build.org/manual.html.
- *Resident compilers.* [F] dmypy keeps program state in memory. Zig 0.14 with
  `-fincremental --watch` re-analyzed a 500,000-line codebase in 63 ms after a
  14 s first pass; without watching, its cache must `fstat` many files even on
  a hit. Sources: https://mypy.readthedocs.io/en/stable/mypy_daemon.html,
  https://ziglang.org/download/0.14.0/release-notes.html.

**Adopt.** [I] Check a verifying trace in the launcher: if each recorded input
still has its stat identity, return the artifact before Python starts
(HF-250). Give each stage its own fingerprint and backdate unchanged outputs
(HF-252, HF-254). Make the stdlib partition incremental per body digest
(HF-516). A resident daemon with file watching can follow; stat verification
stays the authority. `docs/design/build_early_cutoff.md` owns the record.

**Refuse.** [I] File watching as the only truth: a watcher sees nothing while
its process is down.

**Experiment.** Time `molt build` on an unchanged program with only the
launcher check; count `stat` calls. Compare with today's 34 s.

[I] Six rows are not early cutoff: first-link cost of a new program (HF-26,
HF-371, HF-172), per-command guard overhead (HF-517), compiler PGO (HF-458)
and daemon admission (HF-225).

## R. Development state ownership and reconciliation

**Problem.** Custody records guess ownership: four process-liveness probes,
two wrong on Windows (HF-379); tests that write into the checkout (HF-387,
HF-550); private scratch roots (HF-370); Windows-only harness failures
(HF-169, HF-544, HF-551). Nothing reconciles declared state after a crash or
reboot: storage, provisioning and placement each repair themselves, or wait for
a person (HF-582, HF-583, HF-602). [I] The model is a declared-state
reconciler that runs again on every start, as Terraform's plan and apply and
Kubernetes controllers do; records it keeps use F's durable identity.

**Prior art.**
- *Process identity.* [F] Linux `pidfd_open` returns a descriptor bound to one
  process, which avoids PID-reuse races. Windows job objects manage a process
  tree as one unit. psutil compares PID and creation time. Linux subreapers
  adopt orphaned descendants. Sources:
  https://man7.org/linux/man-pages/man2/pidfd_open.2.html,
  https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects,
  https://psutil.readthedocs.io/en/stable/#psutil.Process.is_running,
  https://man7.org/linux/man-pages/man2/PR_SET_CHILD_SUBREAPER.2const.html.
- *Windows `os.kill`.* [F] On Windows, `os.kill` sends `CTRL_C_EVENT` and
  `CTRL_BREAK_EVENT` as console events and ends the process with
  `TerminateProcess` for any other value. Source:
  https://docs.python.org/3/library/os.html#os.kill. [I] So `os.kill(pid, 0)`
  is no liveness probe there (HF-379).
- *Hermetic tests.* [F] Bazel gives each test a private `TEST_TMPDIR` and a
  `TEST_UNDECLARED_OUTPUTS_DIR`; Cargo sets `CARGO_TARGET_TMPDIR` for
  integration-test scratch. Sources: https://bazel.build/reference/test-encyclopedia,
  https://doc.rust-lang.org/cargo/reference/environment-variables.html.

**Adopt.** [I] One process-identity type in `src/molt`: a pidfd or handle where
the OS has one, else PID with start time; never a signal for liveness. One
scratch and evidence root per run outside the checkout, passed through the
environment, and a check that `git status` is unchanged after a run. Delete
custody layers that protect no product invariant (HF-23).
`docs/design/memory_guard_custody_sampling.md` owns tree sampling.

**Refuse.** [I] Ptrace or Seatbelt custody where job objects, cgroups or
subreapers already give the fact.

**Experiment.** On the Windows host, probe a child in its own console group
with `os.kill(pid, 0)` and with `OpenProcess` plus `GetExitCodeProcess`.
Record which probe interrupts the child.

## S. Landing pipeline and self-build cost

**Problem.** CI repeats work: one proof command at a time (HF-190, HF-27),
181 test binaries that each link the workspace (HF-22), a target directory
per checkout (HF-10), and a runtime crate too large for small runners (HF-68).
The merge gate tries to be complete and serial: end-to-end, property and
cross-OS work runs before every landing instead of on a continuous farm after
it (HF-600), and stale queue requests ride in later batches (HF-603). [I] Rust's
bors tests the exact merge commit, and LLVM keeps its merge-time suite small
and runs the heavy suites on post-commit builders.

**Prior art.**
- *cargo-nextest.* [F] `cargo nextest archive` builds test binaries once;
  `--partition` runs one slice per machine, by hash or by count. Sources:
  https://nexte.st/docs/ci-features/archiving/,
  https://nexte.st/docs/ci-features/partitioning/.
- *One integration-test binary.* [F] Cargo compiles each file in `tests/` as
  its own crate (https://doc.rust-lang.org/cargo/reference/cargo-targets.html#integration-tests).
  rust-analyzer's author merges them into one binary to cut link time
  (secondary source:
  https://matklad.github.io/2021/02/27/delete-cargo-integration-tests.html).
- *Measured scheduling.* [F] `cargo build --timings` shows the crate critical
  path; pytest-split balances shards by recorded durations; Cargo's
  `build.build-dir` separates intermediates from final artifacts. Sources:
  https://doc.rust-lang.org/cargo/reference/timings.html,
  https://github.com/jerry-git/pytest-split,
  https://doc.rust-lang.org/cargo/reference/config.html#buildbuild-dir.
- *rustc's release build.* [F] `opt-dist` trains PGO and BOLT profiles in
  rustc's own pipeline. Source:
  https://github.com/rust-lang/rust/tree/1.82.0/src/tools/opt-dist.

**Adopt.** [I] Build the Rust test binaries once per cell and shard their runs
(nextest archive and partition). Merge each crate's integration tests into one
harness (HF-22). Split serial proof families into parallel jobs, ordered by
`--timings` and measured durations (HF-190). Share intermediates only under a
content key (family F), never by path (HF-10).
`docs/design/parallel_build_architecture.md` owns the crate split.

**Refuse.** [I] One target directory across differing sources: HF-10's
two-workspace control returned a wrong value.

**Experiment.** Run `cargo test --no-run --timings` for one crate before and
after merging its integration tests; compare link and wall time.

## T. Installed delivery and witnesses

**Problem.** Release evidence does not bind the resolved dependency graphs
(V1-06), vendored patches never reach upstream (HF-576), the JIT dies under
the hardened runtime (HF-386), and the x402 worker never settles a payment
(HF-380).

**Prior art.**
- *SLSA and attestations.* [F] SLSA build provenance v1.0 records the build
  definition and `resolvedDependencies` in an in-toto statement; PEP 740
  publishes attestations for PyPI files. Sources:
  https://slsa.dev/spec/v1.0/provenance, https://peps.python.org/pep-0740/.
- *cargo-auditable.* [F] It embeds the resolved dependency list in the
  compiled binary. Source: https://github.com/rust-secure-code/cargo-auditable.
- *DEP-3.* [F] Debian tags each carried patch with `Origin` and `Forwarded`
  (a URL, `no` or `not-needed`). Source: https://dep-team.pages.debian.net/deps/dep3/.
- *Apple Silicon JIT.* [F] Apple requires `MAP_JIT` memory,
  `pthread_jit_write_protect_np` toggling and the `allow-jit` entitlement; V8
  maps with `MAP_JIT`. cranelift-jit 0.137 (Wasmtime v50) has a
  `JITMemoryProvider` trait. Sources:
  https://developer.apple.com/documentation/apple-silicon/porting-just-in-time-compilers-to-apple-silicon,
  https://github.com/v8/v8/blob/12.0.1/src/base/platform/platform-posix.cc,
  https://github.com/bytecodealliance/wasmtime/blob/v50.0.0/cranelift/jit/src/memory/mod.rs.
  [F] A GitHub code search of Wasmtime's default branch on 2026-10-29 found no
  `MAP_JIT` or `pthread_jit_write_protect_np`, so HF-386's "as wasmtime does"
  needs correction.
- *x402.* [F] x402 separates `verify` from `settle` at the facilitator;
  EIP-3009 authorizations carry a nonce that settlement consumes once.
  Sources: https://github.com/coinbase/x402, https://eips.ethereum.org/EIPS/eip-3009.

**Adopt.** [I] Emit SLSA provenance that binds `Cargo.lock`, `uv.lock` and the
npm locks to each artifact digest, and embed the Cargo graph with
cargo-auditable. Give each vendored patch DEP-3 headers and a check that
reports when upstream makes it unnecessary. Implement a `MAP_JIT` memory
provider. Settle, or record the nonce, before serving an x402 request.
Installed-consumer witnesses run from the installed artifact, not the
checkout.

**Refuse.** [I] Source checksums as delivery evidence (V1-06).

**Experiment.** Build `molt-backend` with `cargo auditable` and diff its
embedded graph against `Cargo.lock`.

## U. GPU device boundary

**Problem.** GPU work in the tree goes through Molt-owned substitutes: a
`src/tinygrad` facade, a reimplemented op set in `runtime/molt-gpu` and
`demos/tinygrad/reference_stdlib`. Upstream packages cannot reach a device
through Molt (HF-613, HF-614, HF-615).

**Prior art.**
- *`webgpu.h`.* [F] The WebGPU native C API (`webgpu-native/webgpu-headers`)
  is implemented by Dawn on Metal, D3D12 and Vulkan. Sources:
  https://github.com/webgpu-native/webgpu-headers,
  https://dawn.googlesource.com/dawn.
- *tinygrad WebGPU backend.* [F] `tinygrad/runtime/ops_webgpu.py` calls
  `webgpu.h` through generated ctypes bindings (`tinygrad.runtime.autogen.webgpu`)
  and renders WGSL, verified 2026-10-30. Source:
  https://github.com/tinygrad/tinygrad/blob/master/tinygrad/runtime/ops_webgpu.py.
- *Browser hosting.* [I] Emscripten builds `webgpu.h` programs against the
  browser's WebGPU through a host-backed implementation; Pyodide has no GPU
  backend for packages.
- *Array interchange.* [F] DLPack and the buffer protocol hand device and host
  arrays between libraries without copies. Source: https://dmlc.github.io/dlpack/latest/.

**Adopt.** [I] Four boundaries: the CPython C-API/ABI (family L), ctypes and
cffi FFI, the buffer protocol and DLPack, and `webgpu.h`. Upstream tinygrad
runs unmodified over them. On WASM, `webgpu.h` is host-backed and its
asynchronous waits use the suspension bridge (HF-610).

**Refuse.** [I] Rewriting, vendoring or maintaining upstream packages (owner,
2026-10-30). A Molt-owned primitive lives at the C ABI, never as a Python facade.

**Experiment.** Run tinygrad's WebGPU test suite, unmodified, on macOS/Metal
through Molt's ctypes FFI to Dawn.

## Candidates not used

Perry has only a README and blog posts, and no family needs a TypeScript
compiler. Codon, Cython, Nuitka's optimizer, Lean and the K framework,
mutation testing (cargo-mutants, mutmut) and Sigstore beyond PEP 740 did not
fit a family's stated cause. HF-488 already cites λπ for semantic alignment.
