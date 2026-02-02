# Agent Locks

This file coordinates parallel agent work to prevent file collisions.

## How to use
- Claim a file or directory by adding a line: `<agent-id> -> <path>`.
- Keep claims narrow and time-bound.
- Remove your claim when finished.

## Active locks
codex-65907 -> runtime/molt-backend/src/lib.rs
codex-26850 -> src/molt/stdlib/importlib/
codex-51384 -> src/molt/stdlib/logging.py
codex-99620 -> tests/differential/basic/
codex-65907 -> runtime/molt-runtime/src/object/ops.rs
codex-65907 -> runtime/molt-runtime/src/builtins/methods.rs
codex-65907 -> runtime/molt-runtime/src/state/cache.rs
codex-65907 -> runtime/molt-runtime/src/builtins/io.rs
codex-65907 -> src/molt/stdlib/os.py
codex-65907 -> src/molt/stdlib/builtins.py
codex-65907 -> src/molt/_intrinsics.pyi
codex-65907 -> src/molt/cli.py
codex-65907 -> docs/spec/STATUS.md
codex-65907 -> docs/ROADMAP.md
codex-65907 -> docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md
codex-65907 -> tests/differential/COVERAGE_INDEX.yaml
codex-65907 -> tests/differential/INDEX.md
codex-65907 -> tests/differential/planned/errno_basic.py
codex-65907 -> tests/differential/planned/gettext_basic.py
codex-65907 -> tests/differential/planned/glob_basic.py
codex-65907 -> tests/differential/planned/hmac_basic.py
codex-65907 -> tests/differential/planned/ipaddress_basic.py
codex-65907 -> tests/differential/planned/locale_basic.py
codex-65907 -> tests/differential/planned/random_state_basic.py
codex-65907 -> tests/differential/planned/shlex_basic.py
codex-65907 -> tests/differential/planned/shutil_basic.py
codex-65907 -> tests/differential/planned/signal_basic.py
codex-65907 -> tests/differential/planned/stat_basic.py
codex-65907 -> tests/differential/planned/struct_basic.py
