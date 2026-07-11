# Product Surface Audit — Measured New-User Reality

Status: measured on Windows from `C:\Molt\wt-pillars` at commit `5de436f647`
on 2026-07-11. This is an observation record, not a support claim.

## 1. Level 0: `molt run script.py`

`molt run` exists and is the default native compile-and-execute path. It accepts a
file or `--module`, builds through the same wrapper as `molt build`, then executes
the produced binary. A trivial script needs no project file and no user flags.

Measured queue evidence:

- Run: `20260711T161957-product-pillars-cli-audit-rerun-f16dbb9cf0c5493a`.
- Script: `print("hello from molt")`.
- Cold wall time: **272.707 s** to completed output on this checkout and dedicated
  `C:\Molt\target\pillars` target.
- Output correctness: stdout was exactly `hello from molt`.
- Output quality: stderr exposed `Compiling hello.py...`, a raw C compiler warning
  about deprecated `getenv`, and the internal artifact path. Verdict: functional,
  but not a drop-in first-run experience.
- Warm-neighbor observation: a second small program completed in **34.940 s**,
  showing substantial reuse but still far beyond an interactive run budget.

The first attempted unsupported sample (an async-comprehension program) compiled
and printed `<object>` instead of rejecting or matching CPython. That is a
separate semantic frontier, not evidence of a good subset boundary.

A known frontend rejection, `pow(2, 3, mod=5)`, failed before backend execution:

- Run: `20260711T162531-product-pillars-unsupported-baseline-e9df7736feaf4631`.
- Wall time: **22.625 s**.
- Error: `MOLT_COMPAT_ERROR`, feature and source location included, no traceback.
- Before this arc it omitted the canonical boundary contract and a workaround.
- Verdict: honest-early mechanism exists; the product shape was incomplete.

## 2. Level 1: `molt build`

The concise help shows common artifact controls, but the parser contains **54
option actions** for `build`. `run` contains **19 option actions**. The visible
simplicity is presentation-only; the underlying product model remains a wide
flag algebra.

For the ecosystem witness, low-level target, split-runtime, linked-output,
sysroot/toolchain, package-root, manifest, runtime-profile, and diagnostic facts
are transported through a mixture of flags and environment variables. A static
inventory of `src/molt/cli/*.py` found **205 distinct `MOLT_*` keys**. Many are
legitimate internal transport facts, but they are not separated structurally
from user configuration, so users and automation can discover and depend on
implementation channels.

Configuration is read from two peer sources by `_load_molt_config`:

1. `molt.toml` at the project root.
2. `[tool.molt]` in `pyproject.toml`, merged over it.

Parse failures are silently ignored. The merge is not reported as a conflict.
Some command settings then resolve through flags, some through the merged table,
and many through direct environment reads. Verdict: there is not yet one
configuration authority.

## 3. Progressive Disclosure Reality

### Level 0 — run a script

Intended surface: `molt run app.py`. Leaks today: compiler progress, raw native
tool warnings, artifact paths, and multi-minute cold compilation.

### Level 1 — build an artifact

Intended surface: `molt build app.py --target wasm`. Leaks today: 54 parser
options expose emit modes, linker topology, sysroots, cache internals, feedback
profiles, diagnostics plumbing, and runtime layout in one command.

### Level 2 — browser or split runtime

The user must understand `--split-runtime`, linked/unlinked WASM, runtime and app
artifacts, platform/profile coupling, and host-loader requirements. The product
does not yet offer one typed browser deployment intent that derives those facts.

### Level 3 — extension packages

Extension custody has real commands and manifests, but users cross between
`pyproject.toml`, extension subcommands, package metadata, capability manifests,
toolchain discovery, and environment overrides. Package, target, and profile are
independent knobs rather than composable typed values.

## 4. External Drop-In Bar

Primary documentation establishes the comparison bar:

- uv: `uv run file.py` executes a dependency-free script directly and resolves
  declared script/project dependencies automatically.
- Bun: `bun run file` (or `bun file`) runs source directly; `bunfig.toml` is the
  automatically discovered local configuration authority and `--silent` keeps
  runner noise out of program output.
- Deno: `deno run file` runs without a build command or config, auto-discovers
  `deno.json` when present, and keeps capabilities explicit through permissions.

Sources: `https://docs.astral.sh/uv/guides/scripts/`,
`https://bun.sh/docs/runtime`, `https://bun.sh/docs/runtime/bunfig`, and
`https://docs.deno.com/runtime/run/`.

The shared properties are: one obvious command, source-first execution, automatic
project discovery, quiet success, cached repeat execution, and advanced controls
that remain invisible until requested.
