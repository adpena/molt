# Building Molt for Raspberry Pi (aarch64-unknown-linux-gnu)

## Two supported approaches

### Approach A: Build natively on the Pi (recommended)

Cleanest, no toolchain/sysroot fuss. The Pi just needs Rust + the same dependencies the Mac uses.

```bash
# On the Raspberry Pi:
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
sudo apt-get install -y build-essential pkg-config libssl-dev

git clone https://github.com/<your-fork>/molt.git
cd molt
cargo build --profile release-fast -p molt-backend --features native-backend

# Run compliance smoke
python3 -m molt build --target native --output /tmp/hello tests/cross_platform/smoke/hello.py
/tmp/hello
```

Expected wall time: ~25–40 min for a Pi 5; longer on Pi 4.

### Approach B: Cross-compile from macOS via Docker buildx

Faster iteration. Requires Docker Desktop or OrbStack with buildx and the `linux/arm64` platform enabled.

```bash
# On the Mac:
docker buildx create --name molt-cross --use --platform linux/amd64,linux/arm64
docker buildx build --platform linux/arm64 \
    -t molt-aarch64-linux:latest \
    -f tests/cross_platform/docker/Dockerfile.linux-arm64 \
    --load .
docker run --rm molt-aarch64-linux:latest /usr/local/bin/run_compliance.sh
```

(Dockerfile to be added by tests/cross_platform agent.)

### Approach C (NOT supported): direct cross-compile from macOS without sysroot

Tried and rejected. macOS lacks the Linux libc / libpthread / libdl / crt*.o objects required to link an aarch64-linux ELF binary. Both system `ld` (rejects GNU options) and Homebrew `lld` (no sysroot) fail at the link step. The clean fix is Approach A or B; Approach C would require a manually-managed sysroot tarball, which is a maintenance liability and not worth shipping.

## Verification

Once a binary is on the Pi:

```bash
file /usr/local/bin/molt
# Should report: ELF 64-bit LSB executable, ARM aarch64

# Distribute the cross_run.py harness:
scp tools/cross_run.py tools/cross_hosts.toml pi@raspberrypi.local:/tmp/
ssh pi@raspberrypi.local 'cd /tmp && python3 cross_run.py --inventory cross_hosts.toml --smoke'
```

The matrix should report `pi / aarch64-unknown-linux-gnu  N/N PASS`.
