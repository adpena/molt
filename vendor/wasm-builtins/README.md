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

## Provenance (pinned)

Both from wasi-sdk-33 (`33.0+m`): LLVM 22.1.0 (`llvm: 4434dabb6991`), wasi-libc
`161b3195fc25`.

| file | target | size (bytes) | sha256 |
| --- | --- | --- | --- |
| `libc-printscan-long-double.a` | `wasm32-wasip1` | 111146 | `744a4c150a0352732923c167ba284f435947f5836205d9470827bb84256148b9` |
| `libclang_rt.builtins-wasm32.a` | `wasm32-wasi` (used for `wasm32-wasip1`) | 456060 | `b1e23c0376609e09052ff225f290d971b0f8eabd3ffd0737e5d0ebb10f1880d1` |

Each is byte-identical to the corresponding archive shipped inside the
`wasi-sysroot-33.0+m` toolchain (verified equal sha256). NOTE: the
`wasm32-wasip1` and legacy `wasm32-wasi` multilib variants of
`libc-printscan-long-double.a` are **not** identical — the `wasm32-wasip1`
variant is vendored to match Molt's `--target wasm32-wasip1` link.

## Regenerating / bumping

From a wasi-sdk-33 install (matching the `wasi-sysroot-33.0+m` toolchain):

```
cp <wasi-sdk>/share/wasi-sysroot/lib/wasm32-wasip1/libc-printscan-long-double.a \
   vendor/wasm-builtins/libc-printscan-long-double.a
cp <wasi-sdk>/lib/clang/22/lib/wasip1/libclang_rt.builtins-wasm32.a \
   vendor/wasm-builtins/libclang_rt.builtins-wasm32.a
```

When bumping, update the sha256 + versions in this table and in
`tests/test_wasm_longdouble_printf_link.py`. The reloc runtime fingerprint folds
each archive's `(name, size, mtime)` (`_reloc_link_archive_fingerprint_token`),
so a swapped archive correctly invalidates the cached reloc runtime.
