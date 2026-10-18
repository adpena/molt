# Flag strings

Status: slice 1 (boundary authority) implemented, 2026-10-18; slice 2
(typed identity vectors) pending. Owner: integrator (coordination task
`claude-cargo-flag-strings`). Ledger: HF-362.

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

## Slice 2

Identities still hold some flag vectors as strings (`resolved_rustflags:
str | tuple`, `cargo_rustflags`, `fingerprint_rustflags`, `link_flags`), and
`_RuntimeFlagProjection.rustflags` splits them with a host-dependent `shlex`.
Slice 2 makes every internal flag vector a tuple, deletes the string branch,
and keeps strings only at the process boundary through this module.

## Verification

- `tests/test_flag_strings.py`: golden cases from the consumers' source,
  round trips for every rule, and a differential test against Rust `std` and
  the `shlex` crate version cc-rs locks.
- `tests/cli/test_runtime_family_authority.py`: an SDK under a path with a
  quote (and, on POSIX, a backslash) reaches the plan unchanged; an SDK under
  a path with a space fails early.
