# 90-Day Roadmap

This document is the rolling execution slice derived from
[ROADMAP.md](../ROADMAP.md). It is not a competing current-state document; for
current support, use [spec/STATUS.md](spec/STATUS.md).

## 0-30 Days

- Land the documentation architecture rewrite and docs enforcement gates.
- Tighten the validation loop around generated compatibility and benchmark
  summaries.
- Close high-value correctness and parity regressions affecting native and WASM.

## 30-60 Days

- Push more stdlib behavior into Rust intrinsics and remove remaining Python-only
  semantic duplication.
- Harden daemon, CLI, and harness workflows for multi-agent development.
- Improve benchmark coverage and reduce the distance to the performance target.

## 60-90 Days

- Expand compatibility coverage where current contracts are already stable.
- Improve same-contract proof between native and WASM.
- Tighten release-facing proof bundles for standalone binaries and `libmolt`.
