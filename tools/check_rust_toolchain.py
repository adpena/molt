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
import os
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from molt.cargo_execution_policy import cargo_subprocess_environment  # noqa: E402
from molt.cargo_workspace import workspace_member_manifests  # noqa: E402
from molt.rust_toolchain import rust_channel  # noqa: E402

RUST_EDITION = "2024"
# rust-toolchain.toml is the one Rust version authority; every other pin
# (Cargo rust-version, CI setup, the proof plan's preflight) is checked
# against it here.
RUST_VERSION = rust_channel((ROOT / "rust-toolchain.toml").read_bytes())
RUST_TARGETS = ["wasm32-wasip1"]
WORKFLOW_RUST_TOOLCHAIN_ROLES = frozenset({"pinned", "sanitizer-nightly"})
RUST_NIGHTLY_CONFIG = Path("config/rust_nightly_toolchain.txt")
VENDOR_PREFIX = "vendor/"
SELF_EXCLUDES = {
    "tools/check_rust_toolchain.py",
    "tests/test_ci_workflow_topology.py",
    "tests/tools/test_rust_toolchain_contract.py",
}
BAD_FRAGMENTS = (
    "dtolnay/rust-toolchain@",
    "rust-toolchain: nightly",
    "cargo +nightly ",
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


@dataclass(frozen=True)
class RustInstallationPlan:
    channel: str
    components: tuple[str, ...]
    targets: tuple[str, ...]
    nightly: bool


def _atoms(label: str, values: object) -> tuple[str, ...]:
    if not isinstance(values, list) or any(
        not isinstance(value, str)
        or re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._+-]*", value) is None
        for value in values
    ):
        raise ValueError(f"invalid {label} list")
    return tuple(sorted(set(values)))


def installation_plan(
    role: str, *, components: str = "", targets: str = ""
) -> RustInstallationPlan:
    """Resolve complete install inputs from their one declarative authority."""
    extra_components = _atoms("component", components.split(",") if components else [])
    extra_targets = _atoms("target", targets.split(",") if targets else [])
    if role == "pinned":
        table = _read_toml(Path("rust-toolchain.toml"))["toolchain"]
        channel = table["channel"]
        if (
            not isinstance(channel, str)
            or re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", channel) is None
        ):
            raise ValueError("Rust channel must be an exact stable release")
        base_components = _atoms("manifest component", table["components"])
        base_targets = _atoms("manifest target", table["targets"])
    elif role == "sanitizer-nightly":
        channel = (ROOT / RUST_NIGHTLY_CONFIG).read_text(encoding="utf-8").strip()
        if re.fullmatch(r"nightly-[0-9]{4}-[0-9]{2}-[0-9]{2}", channel) is None:
            raise ValueError("Rust nightly must be an exact dated release")
        base_components = base_targets = ()
    else:
        raise ValueError("Rust role must be pinned or sanitizer-nightly")
    return RustInstallationPlan(
        channel,
        tuple(sorted(set(base_components) | set(extra_components))),
        tuple(sorted(set(base_targets) | set(extra_targets))),
        role == "sanitizer-nightly",
    )


def _run(args: list[str], *, timeout: float = 15.0) -> subprocess.CompletedProcess[str]:
    # A version/identity check must never mutate rustup while another command is
    # using its installation. Explicit setup remains a separate caller action.
    env, _cargo_policies = cargo_subprocess_environment(
        args, {**os.environ, "RUSTUP_AUTO_INSTALL": "0"}
    )
    return subprocess.run(
        args,
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
        timeout=timeout,
        env=env,
        encoding="utf-8",
    )


def _git_files(*patterns: str) -> tuple[Path, ...]:
    proc = _run(["git", "ls-files", "-z", "--", *patterns])
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip() or "git ls-files failed")
    paths = (Path(raw) for raw in proc.stdout.split("\0") if raw)
    return tuple(path for path in paths if (ROOT / path).is_file())


def _read_toml(path: Path) -> dict:
    return tomllib.loads((ROOT / path).read_text(encoding="utf-8"))


def _is_vendor(path: Path) -> bool:
    return path.as_posix().startswith(VENDOR_PREFIX)


def workflow_rust_toolchain_errors(workflow: Path, text: str) -> list[str]:
    """Workflows name a toolchain role, never a version.

    setup-project resolves each role from its one authority file, so a Rust
    bump never edits a workflow.
    """
    errors: list[str] = []
    for line_number, line in enumerate(text.splitlines(), start=1):
        match = re.match(r"\s*rust-toolchain:\s*(\S+)", line)
        if match and match[1] not in WORKFLOW_RUST_TOOLCHAIN_ROLES:
            errors.append(
                f"{workflow}:{line_number}: rust-toolchain must name a role "
                f"({', '.join(sorted(WORKFLOW_RUST_TOOLCHAIN_ROLES))}), "
                f"got {match[1]!r}"
            )
    return errors


def check_repository_contract() -> CheckReport:
    errors: list[str] = []

    nightly = (ROOT / RUST_NIGHTLY_CONFIG).read_text(encoding="utf-8").strip()
    if re.fullmatch(r"nightly-\d{4}-\d{2}-\d{2}", nightly) is None:
        errors.append(
            f"{RUST_NIGHTLY_CONFIG}: expected one dated nightly pin, got {nightly!r}"
        )

    toolchain = _read_toml(Path("rust-toolchain.toml")).get("toolchain", {})
    if toolchain.get("components") != ["rustfmt", "clippy"]:
        errors.append("rust-toolchain.toml components must be ['rustfmt', 'clippy']")
    if toolchain.get("targets") != RUST_TARGETS:
        errors.append(f"rust-toolchain.toml targets must be {RUST_TARGETS!r}")

    for obsolete in ("runtime/Cargo.toml", "runtime/Cargo.lock"):
        if (ROOT / obsolete).exists():
            errors.append(
                f"{obsolete}: ordinary crates must use the root Cargo workspace and lock"
            )
    try:
        workspace_manifests = {
            manifest.relative_to(ROOT) for manifest in workspace_member_manifests(ROOT)
        }
    except ValueError as exc:
        return CheckReport((*errors, str(exc)))

    for manifest in _git_files("*Cargo.toml"):
        if _is_vendor(manifest):
            continue
        data = _read_toml(manifest)
        if manifest == Path("Cargo.toml"):
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

    for workflow in _git_files(".github/workflows/*.yml"):
        errors.extend(
            workflow_rust_toolchain_errors(
                workflow, (ROOT / workflow).read_text(encoding="utf-8")
            )
        )

    sanitizer = (ROOT / ".github/workflows/sanitizers.yml").read_text(encoding="utf-8")
    setup_project = (
        ROOT / ".github/actions/setup-project/normalize-inputs.sh"
    ).read_text(encoding="utf-8")
    runtime_safety = (ROOT / "tools/runtime_safety.py").read_text(encoding="utf-8")
    if sanitizer.count("rust-toolchain: sanitizer-nightly") != 2:
        errors.append(
            "sanitizers workflow must project the canonical nightly alias twice"
        )
    if sanitizer.count("steps.project.outputs.rust-toolchain") != 4:
        errors.append(
            "every sanitizer command must consume setup-project's exact toolchain output"
        )
    if "toolchain=$(< config/rust_nightly_toolchain.txt)" not in setup_project:
        errors.append(
            "setup-project must resolve sanitizer-nightly from the config authority"
        )
    if '"config" / "rust_nightly_toolchain.txt"' not in runtime_safety:
        errors.append("runtime_safety must read the canonical dated-nightly authority")

    return CheckReport(tuple(errors))


def check_installed_toolchain(plan: RustInstallationPlan | None = None) -> CheckReport:
    plan = installation_plan("pinned") if plan is None else plan
    errors: list[str] = []
    compiler = _run(["rustc", f"+{plan.channel}", "--version", "--verbose"])
    if compiler.returncode:
        return CheckReport(("selected rustc failed: " + compiler.stderr.strip(),))
    fields = dict(
        line.split(": ", 1) for line in compiler.stdout.splitlines() if ": " in line
    )
    host = fields.get("host", "")
    if re.fullmatch(r"[A-Za-z0-9._+-]+", host) is None:
        return CheckReport(("selected rustc has no valid host triple",))
    release = fields.get("release", "")
    if plan.nightly:
        if not release.endswith("-nightly"):
            errors.append("selected nightly compiler is not a nightly release")
        errors.extend(check_compiler_version(compiler.stdout.splitlines()[0]).errors)
    elif release != plan.channel:
        errors.append(
            f"rustc must report exact release {plan.channel}, got {release!r}"
        )
    cargo = _run(["cargo", f"+{plan.channel}", "--version"])
    pattern = (
        r"cargo \d+\.\d+\.\d+-nightly(?:\s.*)?"
        if plan.nightly
        else rf"cargo {re.escape(plan.channel)}(?:\s.*)?"
    )
    if cargo.returncode or re.fullmatch(pattern, cargo.stdout.strip()) is None:
        errors.append(
            f"selected cargo has invalid version: {cargo.stdout!r} {cargo.stderr}"
        )
    sysroot = _run(["rustc", f"+{plan.channel}", "--print", "sysroot"])
    selected_root = Path(sysroot.stdout.strip())
    if (
        sysroot.returncode
        or not selected_root.is_absolute()
        or not selected_root.is_dir()
        or selected_root.name != f"{plan.channel}-{host}"
    ):
        return CheckReport(
            (*errors, "selected Rust sysroot is unavailable or mismatched")
        )
    tools = ["rustc", "cargo"]
    for component, executables in (
        ("rustfmt", ("rustfmt",)),
        ("clippy", ("cargo-clippy", "clippy-driver")),
        ("miri", ("cargo-miri", "miri")),
    ):
        if component in plan.components:
            tools.extend(executables)
    for tool in tools:
        selected = _run(["rustup", "which", "--toolchain", plan.channel, tool])
        path = Path(selected.stdout.strip())
        if (
            selected.returncode
            or not path.is_absolute()
            or not path.is_file()
            or not path.stat().st_size
        ):
            errors.append(f"selected {tool} executable is unavailable")
            continue
        try:
            path.resolve(strict=True).relative_to(selected_root.resolve(strict=True))
        except (OSError, ValueError):
            errors.append(f"selected {tool} is outside the admitted sysroot")
            continue
        if tool in {"rustc", "cargo"}:
            continue
        # Cargo plugins have their own argv dialect. Their version branches do
        # not load the compiler driver, so admit each driver independently too.
        # rustup run supplies the selected sysroot's dynamic-library search path
        # (including DLL lookup on Windows) while retaining the admitted path.
        arguments = {
            "rustfmt": ["--version"],
            "cargo-clippy": ["--version"],
            "clippy-driver": ["--rustc", "--version", "--verbose"],
            "cargo-miri": ["miri", "--version"],
            "miri": ["--version", "--verbose"],
        }[tool]
        version = _run(["rustup", "run", plan.channel, str(path), *arguments])
        if tool in {"clippy-driver", "miri"}:
            driver_fields = dict(
                line.split(": ", 1)
                for line in version.stdout.splitlines()
                if ": " in line
            )
            valid_version = all(
                fields.get(key) and driver_fields.get(key) == fields[key]
                for key in ("host", "release", "commit-hash")
            )
        else:
            identity = {
                "rustfmt": "rustfmt",
                "cargo-clippy": "clippy",
                "cargo-miri": "miri",
            }[tool]
            valid_version = (
                re.fullmatch(
                    rf"{identity} [0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?"
                    r"(?: \([0-9a-f]{7,40} [0-9]{4}-[0-9]{2}-[0-9]{2}\))?",
                    version.stdout.strip(),
                )
                is not None
            )
        if version.returncode or not valid_version:
            errors.append(f"selected {tool} version probe failed")
    if "rust-src" in plan.components:
        library = selected_root / "lib" / "rustlib" / "src" / "rust" / "library"
        for crate in ("core", "std"):
            source = library / crate / "src" / "lib.rs"
            if not source.is_file() or not source.stat().st_size:
                errors.append(f"Rust source component is missing {crate}")
    components = _run(
        ["rustup", "component", "list", "--installed", "--toolchain", plan.channel]
    )
    if components.returncode:
        errors.append("rustup component inventory failed: " + components.stderr.strip())
    else:
        installed = set(components.stdout.splitlines())
        for component in ("rustc", "cargo", "rust-std", *plan.components):
            if component not in installed and f"{component}-{host}" not in installed:
                errors.append(f"Rust component {component} is missing")
    targets = _run(
        ["rustup", "target", "list", "--installed", "--toolchain", plan.channel]
    )
    if targets.returncode != 0:
        errors.append(
            "rustup target list failed: "
            + (targets.stderr.strip() or targets.stdout.strip())
        )
    else:
        installed = set(targets.stdout.split())
        for target in (host, *plan.targets):
            if target not in installed:
                errors.append(f"Rust target {target} is missing")
                continue
            library = selected_root / "lib" / "rustlib" / target / "lib"
            if any(
                not any(
                    path.is_file() and path.stat().st_size
                    for path in library.glob(pattern)
                )
                for pattern in ("libcore-*.rlib", "libstd-*.rlib")
            ):
                errors.append(f"Rust target {target} has incomplete standard libraries")
    return CheckReport(tuple(errors))


def check_compiler_version(version: str) -> CheckReport:
    """Check the selected compiler against Cargo's workspace minimum."""
    match = re.fullmatch(
        r"rustc (\d+)\.(\d+)\.(\d+)(-[^ ]+)?(?:\s.*)?", version.strip()
    )
    if match is None:
        return CheckReport((f"unrecognized rustc version: {version!r}",))
    actual = tuple(int(match[i]) for i in (1, 2, 3))
    minimum_text = _read_toml(Path("Cargo.toml"))["workspace"]["package"][
        "rust-version"
    ]
    minimum = tuple(int(part) for part in minimum_text.split("."))
    if actual < minimum or (actual == minimum and match[4]):
        return CheckReport(
            (f"{version} is below workspace rust-version {minimum_text}",)
        )
    return CheckReport(())


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--skip-installed",
        action="store_true",
        help="check only checked-in repository contracts, not local rustup tools",
    )
    parser.add_argument(
        "--compiler-version",
        help="validate an explicit rustc --version output against the workspace minimum",
    )
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)

    if args.compiler_version is not None:
        report = check_compiler_version(args.compiler_version)
        for error in report.errors:
            print(error, file=sys.stderr)
        return 0 if report.ok else 1

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
                    "rust_nightly": (ROOT / RUST_NIGHTLY_CONFIG)
                    .read_text(encoding="utf-8")
                    .strip(),
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
