"""Pure invocation and Cargo-operation authority for the runtime test gate."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import math
from pathlib import Path
from typing import Literal, Mapping, Sequence
from molt import host_capacity

WASI_TARGET = "wasm32-wasip1"
OBSERVER_FEATURE = "l7-attestation-probe"
RuntimeGatePolicy = Literal[
    "semantic", "allocation-observer", "wasi-lifecycle", "zlib-wasi"
]


@dataclass(frozen=True)
class SemanticImage:
    """One explicitly owned libtest; empty cases keep the runtime partition."""

    package: str
    target: str
    kind: Literal["rlib", "lib"]
    cases: tuple[str, ...] = ()


# Dependency rlibs do not contain their crates' cfg(test) bodies. Build these
# libtest roots in the same Cargo solve, without expanding to every satellite.
# Only the declared satellite cases execute; other discovered tests stay unrun.
SEMANTIC_IMAGES = (
    SemanticImage("molt-runtime", "molt_runtime", "rlib"),
    SemanticImage(
        "molt-runtime-core",
        "molt_runtime_core",
        "lib",
        (
            "numeric_ratio::tests::ratio_to_f64_matches_cpython_bit_exact",
            "numeric_ratio::tests::ratio_to_f64_overflows_past_f64_max",
            "numeric_ratio::tests::binary64_rounding_boundaries",
        ),
    ),
    SemanticImage(
        "molt-runtime-math",
        "molt_runtime_math",
        "lib",
        (
            "math::statistics_reduction_tests::exact_reduction_oracles_and_primitive_cost",
        ),
    ),
)


def argument_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", default="release-output")
    parser.add_argument(
        "--policy",
        choices=("semantic", "allocation-observer", "wasi-lifecycle", "zlib-wasi"),
        default="semantic",
    )
    parser.add_argument(
        "--parallel-threads",
        type=int,
        default=max(2, min(8, host_capacity.cpu_count())),
    )
    parser.add_argument("--build-timeout-seconds", type=float, default=6300)
    parser.add_argument("--child-timeout-seconds", type=float, default=120)
    parser.add_argument("--receipt-root", type=Path)
    return parser


def parse_arguments(arguments: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argument_parser()
    args = parser.parse_args(arguments)
    if (args.policy == "semantic" and args.parallel_threads < 2) or any(
        not math.isfinite(value) or value <= 0
        for value in (args.build_timeout_seconds, args.child_timeout_seconds)
    ):
        parser.error("positive timeouts and at least two parallel threads are required")
    return args


@dataclass(frozen=True)
class CargoOperation:
    role: Literal["runtime", "host", "provider"]
    command: tuple[str, ...]
    environment: Literal["cargo", "runtime-wasi", "zlib-wasi"]
    c_units: Literal["native", "managed-wasi", "none"]

    @property
    def native_requirements(self) -> dict[str, list[str]]:
        # ABI and ABI-test-support build scripts compile C in runtime libtests;
        # the native Wasmtime host also has C build dependencies. zlib is Rust.
        return {} if self.c_units == "none" else {"target": ["c"]}

    def project_native_requirements(
        self, declared: Mapping[str, Sequence[str]]
    ) -> dict[str, list[str]]:
        if self.c_units == "none":
            return {}
        if "c" not in declared.get("target", ()):
            raise ValueError("runtime Cargo operation lacks its C source declaration")
        return {unit: list(languages) for unit, languages in declared.items()}


def project_native_requirements(
    operations: Sequence[CargoOperation], declared: Mapping[str, Sequence[str]]
) -> tuple[dict[str, list[str]], ...]:
    """Bind outer C custody once, then project only actual C-owning operations."""
    if declared and not any(operation.native_requirements for operation in operations):
        raise ValueError(
            "runtime Cargo policy has no operation owning declared C units"
        )
    return tuple(
        operation.project_native_requirements(declared) for operation in operations
    )


def cargo_operations(
    policy: RuntimeGatePolicy,
    profile: str,
    *,
    cargo: str,
    compiler_host: str | None = None,
) -> tuple[CargoOperation, ...]:
    """Construct the actual child commands before any build or artifact effect."""
    base = (cargo, "test", "--locked", "--profile", profile)
    tail = ("--no-run", "--message-format=json")
    if policy == "zlib-wasi":
        if profile != "dev-fast":
            raise ValueError("zlib-wasi requires the checked dev-fast profile")
        return (
            CargoOperation(
                "provider",
                (
                    *base,
                    "-p",
                    "zlib-rs@0.6.8",
                    "--lib",
                    "--target",
                    WASI_TARGET,
                    "--no-default-features",
                    "--features",
                    "rust-allocator",
                    *tail,
                ),
                "zlib-wasi",
                "none",
            ),
        )
    if policy == "wasi-lifecycle":
        if not compiler_host:
            raise ValueError(
                "WASI lifecycle Cargo operations require the compiler host"
            )
        return (
            CargoOperation(
                "runtime",
                (
                    *base,
                    "-p",
                    "molt-runtime",
                    "--target",
                    WASI_TARGET,
                    *tail,
                    "--lib",
                    "--no-default-features",
                    "--features",
                    "stdlib_micro",
                ),
                "runtime-wasi",
                "managed-wasi",
            ),
            CargoOperation(
                "host",
                (
                    *base,
                    "-p",
                    "molt-wasm-host",
                    "--target",
                    compiler_host,
                    *tail,
                    "--test",
                    "wasi_runtime_lifecycle",
                ),
                "cargo",
                "native",
            ),
            CargoOperation(
                "provider",
                (
                    *base,
                    "-p",
                    "molt-runtime-platform",
                    "--lib",
                    "--target",
                    WASI_TARGET,
                    "--no-default-features",
                    *tail,
                ),
                "cargo",
                "none",
            ),
        )
    if policy not in {"semantic", "allocation-observer"}:
        raise ValueError(f"unknown runtime gate policy: {policy!r}")
    packages = (
        tuple(image.package for image in SEMANTIC_IMAGES)
        if policy == "semantic"
        else ("molt-runtime",)
    )
    selectors = tuple(word for package in packages for word in ("-p", package))
    command = (*base, *selectors, "--lib", *tail)
    if policy == "allocation-observer":
        command += (
            "--test",
            "l7_numeric_runtime_perf_attestation",
            "--features",
            OBSERVER_FEATURE,
        )
    return (CargoOperation("runtime", command, "cargo", "native"),)
