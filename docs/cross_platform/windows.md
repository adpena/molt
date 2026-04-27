# Building Molt for Windows (x86_64-pc-windows-msvc)

## Cross-compile from macOS (limited)

The MSVC target requires the Windows SDK + MSVC headers, which Apple machines do not have. Cross-compile from Mac is technically possible via the `xwin` project (downloads the MSVC SDK) but is fragile.

**Recommended path: build natively on Windows.**

## Build natively on Windows

```powershell
# On Windows with Visual Studio Build Tools 2022 + Rust:
rustup default stable-x86_64-pc-windows-msvc
git clone <your-fork>
cd molt
cargo build --profile release-fast -p molt-backend --features native-backend

# Verify
.\target\release-fast\molt-backend.exe --version
```

## Run compliance

```powershell
python -m pytest tests\compliance\ -p no:cacheprovider -q
```

## GNU target alternative

If the user prefers `x86_64-pc-windows-gnu`, the MinGW toolchain works without Visual Studio. Same `cargo build` command; binary uses MinGW libc instead of MSVC.
