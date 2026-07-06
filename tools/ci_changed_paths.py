from __future__ import annotations

import argparse
import os
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


CLASS_NAMES = (
    "python_tooling",
    "rust",
    "llvm",
    "kani",
    "python_security",
    "rust_security",
)


@dataclass(frozen=True)
class PathRule:
    exact: frozenset[str] = frozenset()
    prefixes: tuple[str, ...] = ()

    def matches(self, path: str) -> bool:
        if path in self.exact:
            return True
        return any(path == prefix[:-1] or path.startswith(prefix) for prefix in self.prefixes)


CI_AUTHORITY = PathRule(
    exact=frozenset(
        {
            ".github/workflows/ci.yml",
            "tools/ci_changed_paths.py",
            "tests/test_ci_changed_paths.py",
            "tests/test_ci_workflow_topology.py",
        }
    )
)

KANI_AUTHORITY = PathRule(
    exact=frozenset(
        {
            ".github/workflows/kani.yml",
            "tools/ci_changed_paths.py",
            "tests/test_ci_changed_paths.py",
            "tests/test_ci_workflow_topology.py",
        }
    )
)

SECURITY_AUTHORITY = PathRule(
    exact=frozenset(
        {
            ".github/workflows/security_hardening.yml",
            "tools/ci_changed_paths.py",
            "tests/test_ci_changed_paths.py",
            "tests/test_ci_workflow_topology.py",
        }
    )
)

PYTHON_TOOLING = PathRule(
    exact=frozenset(
        {
            ".pre-commit-config.yaml",
            ".python-version",
            "pyproject.toml",
            "uv.lock",
        }
    ),
    prefixes=("src/", "tests/", "tools/"),
)

RUST = PathRule(
    exact=frozenset({"Cargo.lock", "Cargo.toml", "rust-toolchain.toml"}),
    prefixes=(".cargo/", "runtime/"),
)

LLVM = PathRule(
    exact=frozenset({"Cargo.lock", "Cargo.toml", "rust-toolchain.toml"}),
    prefixes=(
        "runtime/molt-backend/",
        "runtime/molt-backend-mlir/",
        "runtime/molt-backend-native/",
        "runtime/molt-ir/",
        "runtime/molt-passes/",
        "runtime/molt-tir/",
    ),
)

KANI = PathRule(
    exact=frozenset({"Cargo.lock", "Cargo.toml", "rust-toolchain.toml"}),
    prefixes=("runtime/molt-obj-model/", "runtime/molt-runtime/"),
)

PYTHON_SECURITY = PathRule(
    exact=frozenset({".python-version", "pyproject.toml", "uv.lock"})
)

RUST_SECURITY = PathRule(
    exact=frozenset({"Cargo.lock", "Cargo.toml", "deny.toml", "rust-toolchain.toml"})
)


def normalize_path(path: str) -> str:
    return path.replace("\\", "/").removeprefix("./")


def _any_match(paths: tuple[str, ...], rule: PathRule) -> bool:
    return any(rule.matches(path) for path in paths)


def all_true() -> dict[str, bool]:
    return {name: True for name in CLASS_NAMES}


def classify_paths(paths: list[str] | tuple[str, ...]) -> dict[str, bool]:
    normalized = tuple(normalize_path(path) for path in paths)
    ci_authority = _any_match(normalized, CI_AUTHORITY)
    kani_authority = _any_match(normalized, KANI_AUTHORITY)
    security_authority = _any_match(normalized, SECURITY_AUTHORITY)

    return {
        "python_tooling": ci_authority or _any_match(normalized, PYTHON_TOOLING),
        "rust": ci_authority or _any_match(normalized, RUST),
        "llvm": ci_authority or _any_match(normalized, LLVM),
        "kani": kani_authority or _any_match(normalized, KANI),
        "python_security": security_authority
        or _any_match(normalized, PYTHON_SECURITY),
        "rust_security": security_authority or _any_match(normalized, RUST_SECURITY),
    }


def _run_git(args: list[str]) -> str:
    return subprocess.check_output(["git", *args], text=True, stderr=subprocess.STDOUT)


def changed_paths_for_pull_request(base_ref: str) -> list[str]:
    if not base_ref:
        raise RuntimeError("GITHUB_BASE_REF is not set")

    remote_ref = f"origin/{base_ref}"
    try:
        _run_git(["rev-parse", "--verify", remote_ref])
    except subprocess.CalledProcessError:
        _run_git(
            [
                "fetch",
                "--no-tags",
                "--prune",
                "origin",
                f"{base_ref}:refs/remotes/{remote_ref}",
            ]
        )

    output = _run_git(
        ["diff", "--name-only", "--diff-filter=ACMRTUXB", f"{remote_ref}...HEAD"]
    )
    return [line for line in output.splitlines() if line.strip()]


def write_github_outputs(path: Path, outputs: dict[str, bool]) -> None:
    with path.open("a", encoding="utf-8") as handle:
        for name in CLASS_NAMES:
            value = "true" if outputs[name] else "false"
            print(f"{name}={value}", file=handle)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--github-output", type=Path)
    parser.add_argument("--path", action="append", default=[])
    parser.add_argument(
        "--event-name",
        default=os.environ.get("GITHUB_EVENT_NAME", ""),
        help="GitHub event name; defaults to GITHUB_EVENT_NAME.",
    )
    parser.add_argument(
        "--base-ref",
        default=os.environ.get("GITHUB_BASE_REF", ""),
        help="Pull-request base ref; defaults to GITHUB_BASE_REF.",
    )
    args = parser.parse_args(argv)

    if args.path:
        outputs = classify_paths(args.path)
    elif args.event_name == "pull_request":
        try:
            outputs = classify_paths(changed_paths_for_pull_request(args.base_ref))
        except Exception as exc:  # pragma: no cover - exercised in GitHub fallback.
            print(f"::warning title=CI path classifier fallback::{exc}", file=sys.stderr)
            outputs = all_true()
    else:
        outputs = all_true()

    for name in CLASS_NAMES:
        print(f"{name}={'true' if outputs[name] else 'false'}")

    if args.github_output is not None:
        write_github_outputs(args.github_output, outputs)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
