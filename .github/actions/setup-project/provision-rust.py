"""Provision the complete declared Rust installation before CI fanout."""

from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import sys


ROOT = Path(__file__).resolve().parents[3]


def provision(plan, tool) -> None:
    """Install once, validate the whole installation, then select the default."""
    command = ["rustup", "toolchain", "install", plan.channel, "--profile", "minimal"]
    for component in plan.components:
        command.extend(("--component", component))
    for target in plan.targets:
        command.extend(("--target", target))
    result = tool._run(command, timeout=600.0)
    print(result.stdout, end="")
    print(result.stderr, end="", file=sys.stderr)
    if result.returncode:
        raise RuntimeError("explicit Rust installation failed")
    report = tool.check_installed_toolchain(plan)
    if not report.ok:
        raise RuntimeError(
            "Rust installation is incomplete: " + "; ".join(report.errors)
        )
    result = tool._run(["rustup", "default", plan.channel])
    if result.returncode:
        raise RuntimeError("Rust default selection failed: " + result.stderr)
    print(
        json.dumps(
            {
                "channel": plan.channel,
                "components": plan.components,
                "targets": plan.targets,
            }
        )
    )


def main() -> None:
    # This check precedes repository imports, including the existing Rust
    # contract checker. The Python provisioner owns selection and aliases.
    text = (ROOT / ".python-version").read_text(encoding="utf-8")
    match = re.fullmatch(r"([0-9]+\.[0-9]+\.[0-9]+)\n?", text)
    if match is None:
        raise RuntimeError("repository Python pin must be exactly one X.Y.Z version")
    pin = match[1]
    if (
        platform.python_implementation() != "CPython"
        or platform.python_version() != pin
    ):
        raise RuntimeError("Rust setup requires the exact repository CPython")
    if not os.path.samefile(sys.executable, os.environ["UV_PYTHON"]):
        raise RuntimeError("Rust setup Python differs from the admitted interpreter")
    os.environ["RUSTUP_AUTO_INSTALL"] = "0"
    path = ROOT / "tools/check_rust_toolchain.py"
    spec = importlib.util.spec_from_file_location("molt_ci_rust_contract", path)
    assert spec is not None
    tool = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = tool
    # Execute the current checker source, never an unrelated cached bytecode.
    exec(compile(path.read_bytes(), str(path), "exec"), tool.__dict__)
    plan = tool.installation_plan(
        os.environ["RUST_TOOLCHAIN_ROLE"],
        components=os.environ.get("RUST_COMPONENTS", ""),
        targets=os.environ.get("RUST_TARGETS", ""),
    )
    if plan.channel != os.environ["RUST_TOOLCHAIN"]:
        raise RuntimeError(
            "normalized Rust channel differs from its declared authority"
        )
    # Explicit installation is the only mutation. No cleanup or retry follows a
    # failed transaction; consumers are never allowed to repair partial state.
    provision(plan, tool)


if __name__ == "__main__":
    main()
