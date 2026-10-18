# Flag strings

Status: flag transport, typed identities and WASM export vectors implemented.
The Windows release compiler boundary is implemented; full release qualification
remains open. Owner: integrator. Ledger: HF-362 and HF-363.

## Problem

Cargo and cc-rs read argument vectors from single environment strings. Each
consumer splits its strings with its own rule, and only one rule removes
quotes. Molt wrote and read these strings with Python's `shlex` in seven
places. `shlex.quote` quotes any token with a backslash, so on Windows every
WASI SDK search path reached rustc as `'native=C:\...'`. The plan then refused
its own flags ("`-L all` resource is missing or has the wrong kind"), and 14
or more hosted-Windows cases failed. A POSIX path with a quote or a backslash
fails the same way.

## How the consumers split

Each rule is ported from the consumer's source; the module cites it.

| Rule | Consumer | Split |
|------|----------|-------|
| `CARGO_ENCODED` | `CARGO_ENCODED_RUSTFLAGS` (Cargo `rustflags_from_env`) | at `\x1f`; `""` is no flags |
| `CARGO_RUSTFLAGS` | `RUSTFLAGS`, `RUSTDOCFLAGS` | at `' '`, then `trim()` each piece |
| `CARGO_LIST` | config strings and `CARGO_*` list variables (Cargo `StringList`) | `split_whitespace()` |
| `CC_WHITESPACE` | cc-rs `*FLAGS` by default (`envflags`) | `split_ascii_whitespace()` |
| `CC_SHELL` | cc-rs `*FLAGS` when `CC_SHELL_ESCAPED_FLAGS` is true | the `shlex` crate's `Shlex` |

Python's tools differ from each consumer: `str.split()` also splits at
U+001C..U+001F; the old cc-rs reader split at `\v`; Python `shlex` treats
`\r` as a blank, keeps `\$` inside double quotes and keeps a backslash-newline.

## How leading projects do it

- Cargo added `CARGO_ENCODED_RUSTFLAGS` because a space-separated string
  cannot carry a path with a space; it never shell-unquotes.
- Go `cmd/internal/quoted`: one `Split` and one `Join` for `CGO_CFLAGS`;
  `Join` refuses a word `Split` cannot read back.
- Bazel `Args` and CMake lists keep argv as vectors until the process
  boundary.

## Design

`molt.flag_strings.FlagStringRule` is a closed enum with one member per
consumer rule.

- `decode(value)` returns the vector the consumer reads.
- `encode(tokens, variable=...)` returns a string the consumer decodes to
  exactly `tokens`. It refuses an empty token, a NUL, and any token the rule
  would split, and names the variable. A WASI SDK under a path with a space
  fails before any build, with that diagnostic.
- `FlagStringRule.cc(env)` applies cc-rs `env_value_is_true` to
  `CC_SHELL_ESCAPED_FLAGS`, through the environment's name rule.
- `CC_SHELL` decoding refuses input that the crate would silently truncate
  (an open quote, a trailing backslash).

Consumers: `runtime_cargo_plan` (Cargo flag inputs, C flag projection,
encoded flags), `toolchain_capture`, `llvm_toolchain` (SDK projection),
`native_link_command` (the link driver reads `CFLAGS` as cc-rs does) and
`tools/release/native_build.py`.

## Costs

None on a hot path. Each decode is one linear pass over a string of tens of
bytes, once per plan. The `rustc` differential test compiles two small crates
in about 0.5 s.

The native Windows release producer serializes its compiler-owned options once,
linearly in the path length. It adds no process, file or runtime check. The
compiler-boundary regression runs in the existing Windows portability command;
it does not rebuild Molt. Emitted programs pay no verification overhead.

## Windows release compiler boundary

MSVC ignores `/pathmap` without `/experimental:deterministic`, issuing D9007
while returning success. Native releases author both options in MSVC's
[documented `CL` environment](https://learn.microsoft.com/en-us/cpp/build/reference/cl-environment-variables).
The compiler consumes that string before its explicit arguments. CPython's
`subprocess.list2cmdline` owns Windows argument quoting. The serialized value is
bounded by the compiler's 1024-character limit, measured in Windows UTF-16 units;
oversize values fail before compilation. Ambient `CL` and `_CL_` remain excluded
by the release environment, and existing compile identities capture both names.

Only `/Brepro` remains in Windows `CFLAGS` and `CXXFLAGS`. This matters because
[cmake-rs 0.1.58](https://docs.rs/cmake/0.1.58/src/cmake/lib.rs.html) first obtains
cc-rs's decoded compiler arguments, then concatenates them into a CMake flag
string without quoting each argument. It does not simply forward the shell
quotes in raw `CFLAGS`. Embedding literal quotes in a path operand makes CMake's
command work but changes direct compiler-argument semantics. Keeping path-bearing
options in the compiler-owned environment preserves both consumers without an
adapter, vendored dependency, response file or second flag parser.

The owning control compiles real C and C++ through direct driver arguments and
CMake/Ninja in two relocated roots, including spaces, a quote, Unicode and `#`.
It requires effective `__FILE__` remapping, no ignored-option diagnostic and
byte-identical object files per route. The source and test select the existing
Windows PR/main proof command. This is compiler-boundary qualification, not a
complete Windows release build or release-matrix acceptance. A compatible
CMake/Ninja encoding pair is required: older ANSI-generating CMake versions can
corrupt Unicode paths when paired with a UTF-8 Ninja. The retained qualification
uses the already installed Visual Studio CMake, explicitly selected in the proof
environment; it does not modify machine-wide tool selection.
The proof captures image CMake and the existing pinned Ninja identities. The
development dependency group installs that same Ninja pin without installing
the scientific-package build groups. There is one executable-policy authority
for Ninja; queue admission is part of the regression's control surface.

## Exceptions and open questions

- `llvm_toolchain._llvm_config_tokens` parses `llvm-config` output, a
  different producer with its own quoting. It stays.
- Non-Windows path-bearing flags still require actual CMake-consuming release
  qualification. The adapter's unquoted join cannot preserve arbitrary argv
  boundaries; the Windows compiler environment is not a portable replacement
  for that separate boundary. Full activated dependency and release-cell
  qualification stays under HF-363.

## Slice 2: typed identity vectors

Runtime build identities held Rust flag vectors as `shlex.join` strings, and
`_RuntimeFlagProjection.rustflags` split them with
`shlex.split(posix=os.name != "nt")` and stripped `"` from every token. On
Windows the quotes stayed in the identity; on every host `feature="std"`
became `feature="std`. Now `RuntimeBuildMemberPlan` and the WASM build spec
hold tuples. The member plan's `__post_init__` refuses anything else, the
projection refuses a string, and the identity functions read `plan.rustflags`
instead of a second string copy (the dead `base_rustflags` parameter is gone).

## Slice 3: WASM export link vectors

`wasm_runtime_*_export_link_args` returns a tuple of Rust flag tokens, with
each export represented by `("-C", "link-arg=--export-if-defined=NAME")`.
Export ordering remains sorted. The WASM build spec carries `runtime_exports`
and `link_flags` as tuples; shared import flags precede those export tokens.
Cargo plan and member fingerprint projections consume the tuples directly.

`wasm_link_args_from_rust_flag_tokens` extracts ordered linker operands with
`rust_flag_spans` and refuses a string. Relocatable runtime links and the
combined Cargo producer pass those operands through
`write_wasm_link_args_response_file`, one admitted argument per line. Cargo
environment strings still use `FlagStringRule` at their boundary. No shell
quoting or splitting occurs on the export vectors.

Runtime family identities serialize flag tuples as JSON lists, with the same
tokens the old string split into, so fingerprint sidecars keep version 3 and
no artifact rebuilds. Tools that carry export
symbol sets rather than flag vectors keep those sets.

This removes string construction and shell parsing over the export surface
from build planning and relinking. Tuple construction and operand extraction
are linear in the number of exports, after the existing sort. It adds no
processes, filesystem operations or response-file writes; response-file
content addressing and reuse are unchanged.

## Verification

- `tests/test_flag_strings.py`: golden cases from the consumers' source,
  round trips for every rule, and a differential test against Rust `std` and
  the `shlex` crate version cc-rs locks.
- `tests/cli/test_runtime_family_authority.py`: an SDK under a path with a
  quote (and, on POSIX, a backslash) reaches the plan unchanged; an SDK under
  a path with a space fails early.
- `tests/cli/test_wasm_runtime_exports.py` and
  `tests/cli/test_runtime_wasm_single_compile.py`: export tokens, ordering,
  shared imports and member identity projections.
- `tests/test_wasm_longdouble_durable_provisioning.py` and
  `tests/test_bench_wasm_runtime_build_flags.py`: token extraction without
  shell interpretation and byte-stable response files.
