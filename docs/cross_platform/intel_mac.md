# Building Molt for Intel Mac (x86_64-apple-darwin)

## Cross-compile from Apple Silicon Mac

```bash
# On the M-series Mac:
rustup target add x86_64-apple-darwin
cargo build --target x86_64-apple-darwin --profile release-fast -p molt-backend --features native-backend

# Verify
file target/x86_64-apple-darwin/release-fast/molt-backend
# Should report: Mach-O 64-bit executable x86_64

# Distribute:
scp target/x86_64-apple-darwin/release-fast/molt-backend intel-mac:/tmp/
```

## Build natively on the Intel Mac

Same `cargo build --profile release-fast -p molt-backend --features native-backend`. The macOS sysroot is identical to Apple Silicon, just a different ISA.

## Run compliance

```bash
ssh intel-mac:
  cd /path/to/molt
  python3 -m pytest tests/compliance/ -p no:cacheprovider -q
```
