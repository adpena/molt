#!/usr/bin/env python3
"""Check Molt's Rust toolchain and edition authority.

This is the single repo-owned guard for Rust version drift. It checks the
checked-in contract, CI/workflow pins, Cargo manifests, and optionally the local
installed tools. It performs only bounded metadata/version probes; it never
builds.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RUST_EDITION = "2024"
RUST_VERSION = "1.96.1"
RUST_TARGETS = ["wasm32-wasip1"]
VENDOR_PREFIX = "vendor/"
SELF_EXCLUDES = {
    "tools/check_rust_toolchain.py",
    "tests/tools/test_rust_toolchain_contract.py",
}
BAD_FRAGMENTS = (
    "dtolnay/rust-toolchain@stable",
    "rustup toolchain install stable",
    "rustup default stable",
    "stable-x86_64-pc-windows-msvc",
    "--edition=2021",
    'edition = "2021"',
    'rust-version = "1.85"',
    'rust-version = "1.72"',
)


@dataclass(frozen=True)
class CheckReport:
    errors: tuple[str, ...]
    warnings: tuple[str, ...] = ()

    @property
    def ok(self) -> bool:
        return not self.errors


def _run(args: list[str], *, timeout: float = 15.0) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
        timeout=timeout,
    )


def _git_files(*patterns: str) -> tuple[Path, ...]:
    proc = _run(["git", "ls-files", "-z", "--", *patterns])
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip() or "git ls-files failed")
    return tuple(
        Path(raw.decode("utf-8")) for raw in proc.stdout.encode().split(b"\0") if raw
    )


def _read_toml(path: Path) -> dict:
    return tomllib.loads((ROOT / path).read_text(encoding="utf-8"))


def _is_vendor(path: Path) -> bool:
    return path.as_posix().startswith(VENDOR_PREFIX)


def _workspace_member_manifests(workspace: Path) -> set[Path]:
    data = _read_toml(workspace)
    base = workspace.parent
    manifests: set[Path] = set()
    for member in data.get("workspace", {}).get("members", []):
        if "*" in member:
            continue
        manifest = base / member / "Cargo.toml"
        if (ROOT / manifest).exists():
            manifests.add(manifest)
    return manifests


def check_repository_contract() -> CheckReport:
    errors: list[str] = []

    toolchain = _read_toml(Path("rust-toolchain.toml")).get("toolchain", {})
    if toolchain.get("channel") != RUST_VERSION:
        errors.append(
            f"rust-toolchain.toml channel must be {RUST_VERSION}, got {toolchain.get('channel')!r}"
        )
    if toolchain.get("components") != ["rustfmt", "clippy"]:
        errors.append("rust-toolchain.toml components must be ['rustfmt', 'clippy']")
    if toolchain.get("targets") != RUST_TARGETS:
        errors.append(f"rust-toolchain.toml targets must be {RUST_TARGETS!r}")

    workspace_manifests = _workspace_member_manifests(
        Path("Cargo.toml")
    ) | _workspace_member_manifests(Path("runtime/Cargo.toml"))

    for manifest in _git_files("*Cargo.toml"):
        if _is_vendor(manifest):
            continue
        data = _read_toml(manifest)
        if manifest in {Path("Cargo.toml"), Path("runtime/Cargo.toml")}:
            workspace_package = data.get("workspace", {}).get("package", {})
            if workspace_package.get("edition") != RUST_EDITION:
                errors.append(
                    f"{manifest}: workspace.package.edition must be {RUST_EDITION}"
                )
            if workspace_package.get("rust-version") != RUST_VERSION:
                errors.append(
                    f"{manifest}: workspace.package.rust-version must be {RUST_VERSION}"
                )
            continue

        package = data.get("package", {})
        if manifest in workspace_manifests:
            if package.get("edition") != {"workspace": True}:
                errors.append(f"{manifest}: edition must inherit from workspace")
            if package.get("rust-version") != {"workspace": True}:
                errors.append(f"{manifest}: rust-version must inherit from workspace")
        else:
            if package.get("edition") != RUST_EDITION:
                errors.append(f"{manifest}: edition must be {RUST_EDITION}")
            if package.get("rust-version") != RUST_VERSION:
                errors.append(f"{manifest}: rust-version must be {RUST_VERSION}")

    scan_files = _git_files("*.toml", "*.py", "*.md", "*.yml", "*.yaml")
    for path in scan_files:
        if _is_vendor(path) or path.as_posix() in SELF_EXCLUDES:
            continue
        text = (ROOT / path).read_text(encoding="utf-8")
        for fragment in BAD_FRAGMENTS:
            if fragment in text:
                errors.append(f"{path}: stale Rust toolchain fragment {fragment!r}")

    return CheckReport(tuple(errors))


def _toolchain_path_errors(tool: str) -> list[str]:
    proc = _run(["rustup", "which", tool])
    if proc.returncode != 0:
        return [
            f"rustup which {tool} failed: {proc.stderr.strip() or proc.stdout.strip()}"
        ]
    path = proc.stdout.strip().replace("\\", "/")
    if f"/toolchains/{RUST_VERSION}-" not in path:
        return [
            f"{tool} must resolve through rustup toolchain {RUST_VERSION}, got {path}"
        ]
    return []


def _version_errors(tool: str, pattern: str) -> list[str]:
    proc = _run([tool, "--version"])
    if proc.returncode != 0:
        return [
            f"{tool} --version failed: {proc.stderr.strip() or proc.stdout.strip()}"
        ]
    version = proc.stdout.strip()
    if re.match(pattern, version) is None:
        return [f"{tool} must report {RUST_VERSION}, got {version!r}"]
    return []


def check_installed_toolchain() -> CheckReport:
    errors: list[str] = []
    errors.extend(_version_errors("rustc", rf"^rustc {re.escape(RUST_VERSION)}\b"))
    errors.extend(_version_errors("cargo", rf"^cargo {re.escape(RUST_VERSION)}\b"))
    for tool in ("rustc", "cargo", "rustfmt", "cargo-clippy"):
        errors.extend(_toolchain_path_errors(tool))
    targets = _run(
        ["rustup", "target", "list", "--installed", "--toolchain", RUST_VERSION]
    )
    if targets.returncode != 0:
        errors.append(
            "rustup target list failed: "
            + (targets.stderr.strip() or targets.stdout.strip())
        )
    else:
        installed = set(targets.stdout.split())
        for target in RUST_TARGETS:
            if target not in installed:
                errors.append(
                    f"Rust target {target} is missing; run: "
                    f"rustup target add {target} --toolchain {RUST_VERSION}"
                )
    return CheckReport(tuple(errors))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--skip-installed",
        action="store_true",
        help="check only checked-in repository contracts, not local rustup tools",
    )
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)

    reports = [check_repository_contract()]
    if not args.skip_installed:
        reports.append(check_installed_toolchain())

    errors = tuple(error for report in reports for error in report.errors)
    warnings = tuple(warning for report in reports for warning in report.warnings)

    if args.json:
        print(
            json.dumps(
                {
                    "ok": not errors,
                    "rust_version": RUST_VERSION,
                    "edition": RUST_EDITION,
                    "targets": RUST_TARGETS,
                    "errors": errors,
                    "warnings": warnings,
                },
                indent=2,
            )
        )
    elif errors:
        print("Rust toolchain contract failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
    else:
        print(
            "Rust toolchain contract OK: "
            f"rust {RUST_VERSION}, edition {RUST_EDITION}, "
            f"targets {', '.join(RUST_TARGETS)}"
        )

    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
