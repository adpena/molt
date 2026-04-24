# BOLT Post-Link Optimization for Molt Binaries

BOLT (Binary Optimization and Layout Tool) reorders functions and basic blocks
in a compiled binary using profiling data.  Applied to molt-compiled native
executables, it improves instruction cache locality and branch prediction,
yielding 5-20% throughput gains on hot loops.

## Prerequisites

- LLVM BOLT (`llvm-bolt`, ships with LLVM >= 14)
- `perf` (Linux) or Instruments (macOS; BOLT itself is Linux-only today)
- A representative workload to profile against

## Workflow

### 1. Build with relocations preserved

The linker must emit relocations so BOLT can rewrite the binary:

```bash
# Native backend (Cranelift object output)
export RUSTFLAGS="-C link-arg=-Wl,--emit-relocs"
cargo build --profile release-fast -p molt-backend --features native-backend
```

For molt-compiled user binaries, pass the flag through the molt CLI:

```bash
python3 -m molt build --target native --link-flags="-Wl,--emit-relocs" app.py
```

### 2. Profile the workload

Collect hardware performance counter data for the target workload:

```bash
perf record -e cycles:u -o perf.data -- ./binary <args>
```

For long-running servers, use `-g --call-graph dwarf` to capture call graphs
that BOLT can use for interprocedural optimization.

### 3. Convert profile to BOLT format

```bash
perf2bolt -p perf.data -o perf.fdata ./binary
```

If the binary was built with LBR (Last Branch Record) support:

```bash
perf record -e cycles:u -j any,u -o perf.data -- ./binary <args>
perf2bolt -p perf.data -o perf.fdata -nl ./binary
```

### 4. Optimize

```bash
llvm-bolt ./binary -o ./binary.bolt \
    -data=perf.fdata \
    -reorder-blocks=ext-tsp \
    -reorder-functions=hfsort \
    -split-functions \
    -split-all-cold \
    -dyno-stats
```

Key flags:
- `-reorder-blocks=ext-tsp`: Extended TSP block layout (best general-purpose).
- `-reorder-functions=hfsort`: Hot/cold function clustering via call graph.
- `-split-functions`: Move cold code out of hot function bodies.
- `-split-all-cold`: Aggressively split all cold basic blocks.
- `-dyno-stats`: Print estimated performance improvement statistics.

### 5. Validate and deploy

```bash
# Verify correctness
./binary.bolt --self-test  # or run your test suite

# Replace original
mv ./binary.bolt ./binary
```

## Expected Gains

| Workload type          | Typical improvement |
|------------------------|---------------------|
| Compute-bound loops    | 10-20%              |
| Import-heavy startup   | 5-10%               |
| Mixed I/O + compute    | 3-8%                |

## Limitations

- BOLT is Linux-only. macOS binaries require Instruments profiling exported
  to a compatible format, or cross-compilation.
- Position-independent executables (PIE) require `--emit-relocs` at link time;
  without it, BOLT cannot rewrite the binary.
- Stripped binaries lose symbol information; build with debug info or use
  `-funique-internal-linkage-names` for best results.
- BOLT is a post-link step; it must be re-run whenever the binary changes or
  the workload profile shifts significantly.

## Integration with CI

For release builds, add a BOLT step after linking:

```yaml
- name: BOLT optimize
  run: |
    perf record -e cycles:u -o perf.data -- ./target/release-fast/binary bench_workload
    perf2bolt -p perf.data -o perf.fdata ./target/release-fast/binary
    llvm-bolt ./target/release-fast/binary -o ./target/release-fast/binary.bolt \
        -data=perf.fdata -reorder-blocks=ext-tsp -reorder-functions=hfsort \
        -split-functions -split-all-cold
    mv ./target/release-fast/binary.bolt ./target/release-fast/binary
```
