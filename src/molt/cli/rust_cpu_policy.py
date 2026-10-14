"""The CPU that Molt's own Rust code is compiled for, resolved once per build.

``MOLT_PERF_PROFILE=native-arch`` compiles the Rust that a native build uses,
the runtime staticlib and the backend compiler, with ``-C target-cpu=native``.
A build resolves the policy once from its request: the target and the knob.
It then carries the result as a typed value. Each Cargo plan appends the flag
to the Rust flags that Cargo would select, and pins the result as
``CARGO_ENCODED_RUSTFLAGS``. The compiler fingerprint and the runtime build
identity bind those flags, so native-arch and baseline outputs never share a
cache entry. The CLI never writes ``RUSTFLAGS`` or any other process
environment variable, so a later build in the same process (for example a WASM
build) cannot inherit the flag.
"""

from __future__ import annotations

import enum
from collections.abc import Mapping

PERF_PROFILE_ENV = "MOLT_PERF_PROFILE"
NATIVE_ARCH_PERF_PROFILE = "native-arch"
NATIVE_CPU_RUSTFLAGS = ("-C", "target-cpu=native")


class RustCpuPolicy(enum.Enum):
    """The target CPU of the Rust code that one build compiles."""

    BASELINE = "baseline"
    NATIVE = "native"

    def rustflags(self, flags: tuple[str, ...]) -> tuple[str, ...]:
        """Return the Rust flags a Cargo plan pins under this policy.

        ``flags`` are the flags Cargo would select without the policy. The
        native flag goes last, so it wins over an earlier ``target-cpu``. The
        result is stable when the plan applies it again to its own flags.
        """
        if self is RustCpuPolicy.BASELINE or flags[-2:] == NATIVE_CPU_RUSTFLAGS:
            return flags
        return (*flags, *NATIVE_CPU_RUSTFLAGS)


def resolve_rust_cpu_policy(
    target: str, env: Mapping[str, str]
) -> tuple[RustCpuPolicy, str | None]:
    """Resolve the policy of one build from its target and its environment.

    ``target`` is the build target after alias selection (``llvm`` is
    ``native``). Only the ``native`` target compiles for the host CPU; another
    target keeps the baseline and gets a warning. A value other than
    ``native-arch`` is an error, never a silent baseline.
    """
    raw = env.get(PERF_PROFILE_ENV, "").strip()
    if not raw:
        return RustCpuPolicy.BASELINE, None
    if raw != NATIVE_ARCH_PERF_PROFILE:
        raise ValueError(
            f"{PERF_PROFILE_ENV} must be {NATIVE_ARCH_PERF_PROFILE!r} or unset, "
            f"not {raw!r}."
        )
    if target != "native":
        return RustCpuPolicy.BASELINE, (
            "Native-arch perf profile requested, but non-native target "
            f"{target!r} selected; using the baseline CPU."
        )
    return RustCpuPolicy.NATIVE, None
