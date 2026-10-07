# Vendored wasm32 long-double link archives

Two archives, committed here so they are **present by construction** on every
machine, session, and CI runner (the reloc runtime long-double fix):

- `libc-printscan-long-double.a` — wasi-libc's real `%L` printf/scanf formatters
  (`wasm32-wasip1` multilib variant).
- `libclang_rt.builtins-wasm32.a` — LLVM `compiler-rt` binary128 soft-float
  builtins the formatters (and numpy's own long double arithmetic) call.

## Why this is vendored (durability, not convenience)

Molt's reloc runtime link (`_link_runtime_staticlib_to_reloc_wasm` in
`src/molt/cli/runtime_build.py`) whole-archives wasi-libc's
`libc-printscan-long-double.a` so numpy's `long double` repr/parse
(`NumPyOS_ascii_formatl` / `strtold`) does not hit wasi-libc's
`long_double_not_supported` stub, which lowers to a raw `unreachable` trap at
`_multiarray_umath` import. Those long-double formatters call the binary128
soft-float builtins (`__addtf3` / `__multf3` / `__subtf3` / …). Those builtins
are **not** part of the wasi-sysroot tarball — `libc.a` and
`libc-printscan-long-double.a` ship in the sysroot's `lib/wasm32-wasip1/`
multilib, but `libclang_rt.builtins-wasm32.a` lives in wasi-sdk's compiler-rt
resource dir (`<wasi-sdk>/lib/clang/<ver>/lib/wasip1/`), which the provisioned
sysroot subset does not include.

Before this vendoring the archives were placed into the sysroot lib dir by hand.
That is a provisioning race: a fresh / wiped / CI / another-machine target-root
provisions the sysroot subset **without** compiler-rt (and a session sysroot can
miss the long-double formatter entirely), so
`wasm_clang_rt_builtins_archive()` / `wasm_wasi_printscan_long_double_archive()`
returned `None`, the reloc link degraded, and the long-double stub was relinked
— reintroducing the exact `unreachable` trap the fix removed (effect-attestation
failure; witness RUN 20260710T164604).

Committing the archives makes them resolvable with zero provisioning: the
resolvers in `molt.cli.wasm_toolchain` fall back to these copies when the sysroot
lib dir (and, for builtins, a full wasi-sdk compiler-rt resource dir) does not
have them.

## Provenance and bumping

`provenance.toml` records the WASI SDK release the archives came from, each
archive's path inside that SDK, its size and SHA-256. Each host's SDK embeds
its own build paths (compiler-rt's `__FILE__` abort strings), so hosts never
agree byte for byte; the copies come from the CI reference host's SDK archive,
whose digest matches the upstream release record, and every host's SDK must
ship the same archive members.

The archives move only with the WASI SDK pin:

```
uv run python3 tools/pin_freshness.py --update wasi-sdk
```

That one command verifies every host's SDK archive, rewrites
`config/llvm_toolchain_releases.toml`, these archives and `provenance.toml`
together, and restores all of them if the manifest loader rejects the result.
`tests/test_wasm_longdouble_durable_provisioning.py` fails when the archives,
`provenance.toml` and the pinned SDK disagree. The reloc runtime fingerprint
folds each archive's `(name, size, mtime)`
(`_reloc_link_archive_fingerprint_token`), so a swapped archive invalidates
the cached reloc runtime.
