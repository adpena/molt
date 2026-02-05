# Agent Locks

This file coordinates parallel agent work to prevent file collisions.

## How to use
- Claim a file or directory by adding a line: `<agent-id> -> <path>`.
- Keep claims narrow and time-bound.
- Remove your claim when finished.

## Active locks
codex -> runtime/molt-runtime/src/state/cache.rs
codex -> runtime/molt-runtime/src/builtins/functools.rs
codex -> runtime/molt-runtime/src/builtins/itertools.rs
codex -> runtime/molt-runtime/src/builtins/operator.rs
codex -> runtime/molt-runtime/src/builtins/types.rs
codex -> runtime/molt-runtime/src/builtins/types.rs
codex -> src/molt/stdlib/contextvars.py
codex -> src/molt/stdlib/base64.py
codex -> src/molt/stdlib/bisect.py
codex -> src/molt/stdlib/fnmatch.py
codex -> src/molt/stdlib/importlib/__init__.py
codex -> src/molt/stdlib/importlib/machinery.py
codex -> src/molt/stdlib/importlib/util.py
codex -> src/molt/stdlib/json.py
codex -> src/molt/stdlib/pprint.py
codex -> src/molt/stdlib/random.py
codex -> src/molt/stdlib/unittest.py
codex -> src/molt/stdlib/string.py
codex -> src/molt/stdlib/__init__.py
codex -> src/molt/stdlib/sys.py
codex -> src/molt/stdlib/traceback.py
codex -> src/_intrinsics.py
codex -> src/molt/stdlib
