# Flag strings

Status: slices 1 (boundary authority), 2 (typed identity vectors) and 3
(WASM export link vectors) implemented, 2026-10-18. Owner:
integrator (coordination task `claude-cargo-flag-strings`). Ledger: HF-362.

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

## Exceptions and open questions

- `llvm_toolchain._llvm_config_tokens` parses `llvm-config` output, a
  different producer with its own quoting. It stays.
- `tools/release/native_build.py` gives `CFLAGS` to cc-rs (shell rule) and to
  CMake, which pastes `CFLAGS` into build commands. On Windows, MSVC does not
  read single quotes, so a quoted token may break a CMake build. This is
  unverified; it needs a Windows release build cell.

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

Runtime family identities continue to serialize flag tuples as JSON lists.
Artifact fingerprint sidecars use version 4, invalidating earlier sidecars
for this completed contract migration; the family/member identity wire
schemas retain their existing list representation. Tools that carry export
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
