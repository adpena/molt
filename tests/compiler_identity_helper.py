from __future__ import annotations

from pathlib import Path
import json
import os
import shutil
from types import SimpleNamespace

from molt.cli.cargo_environment_inputs import CargoEnvironmentInputs
from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.process_environment import ProcessEnvironment
from molt.exact_json import canonical_json_sha256


def compiler_build_admission(
    features=(), profile="release", environment=None, rust_cpu=RustCpuPolicy.BASELINE
):
    command = [
        "cargo",
        "build",
        "--locked",
        "--message-format=json-render-diagnostics",
        "--package",
        "molt-backend",
        "--bin",
        "molt-backend",
        "--profile",
        profile,
    ]
    if features:
        command += ["--no-default-features", "--features", ",".join(features)]

    class Plan(list):
        @property
        def command(self):
            return tuple(self)

        executable_custody = (
            SimpleNamespace(
                label="tool/rustc", content_record=lambda: {"sha256": "a" * 64}
            ),
        )

        def verify(self):
            pass

    plan = Plan(command)
    plan.environment = ProcessEnvironment(environment or {})
    plan.rust_resources = SimpleNamespace(files=())
    plan.link_resources = SimpleNamespace(files=())
    # This fixture models a fixed toolchain and features/profile/Rust flags,
    # including the build's Rust CPU policy.
    # Guest settings and process custody do not rename that host compiler.
    # Real Cargo environment/configuration projection is exercised through
    # backend_build_admission in test_compiler_identity.py.
    return SimpleNamespace(
        plan=plan,
        verify=lambda: None,
        fingerprint=canonical_json_sha256(
            {
                "features": sorted(features),
                "profile": profile,
                "rustflags": plan.environment.get("RUSTFLAGS", ""),
                "rust_cpu": rust_cpu.value,
            }
        ),
    )


def stub_compiler_admission(monkeypatch):
    from molt.cli import backend_binary, compiler_identity

    def admission(root, features, profile, env, *, rust_cpu):
        write_compiler_source(root)
        return compiler_build_admission(features, profile, env, rust_cpu)

    monkeypatch.setattr(backend_binary, "backend_build_admission", admission)
    monkeypatch.setattr(compiler_identity, "backend_build_admission", admission)


def write_compiler_lock(root):
    manifest = root / "runtime/molt-backend/Cargo.toml"
    if not manifest.exists():
        manifest.parent.mkdir(parents=True, exist_ok=True)
        manifest.write_text(
            '[package]\nname = "molt-backend"\nversion = "0.1.0"\n', encoding="utf-8"
        )
    lock = root / "Cargo.lock"
    if not lock.exists():
        lock.write_text(
            'version = 4\n[[package]]\nname = "molt-backend"\nversion = "0.1.0"\n',
            encoding="utf-8",
        )


def write_compiler_source(root: Path) -> None:
    """Complete source closure for tests that intercept Cargo execution.

    Keep generation capture and verification real, including the out-of-crate
    catalog. Existing inputs are preserved for deliberate mutation tests.
    """
    write_compiler_lock(root)
    for relative, content in {
        "Cargo.toml": '[workspace]\nmembers = ["runtime/molt-backend"]\n',
        "runtime/molt-backend/src/main.rs": "fn main() {}\n",
        "src/molt/backend_environment.json": "{}\n",
        "src/molt/backend_daemon_protocol_version.txt": "2\n",
    }.items():
        path = root / relative
        if not path.exists():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")


# Synthetic backend Cargo fixtures declare this arbitrary input in rustc's
# dep-info; published-receipt fixtures model that same local unit observation.
BACKEND_FIXTURE_INPUT = "MOLT_TEST_BACKEND_INPUT"


def compiler_environment_inputs(environment=None):
    return CargoEnvironmentInputs.capture(
        ProcessEnvironment(os.environ if environment is None else environment),
        (BACKEND_FIXTURE_INPUT,),
        local_unit_count=1,
    )


def compiler_cargo_stdout(root: Path, output: Path, environment) -> str:
    """Real Cargo/rustc protocol for a known local binary's exact output bytes."""
    root = root.resolve()
    write_compiler_source(root)
    package = root / "runtime/molt-backend"
    source = package / "src/main.rs"
    deps = output.parent / "deps"
    deps.mkdir(parents=True, exist_ok=True)
    unit = deps / (
        "molt_backend-fixture.exe" if os.name == "nt" else "molt_backend-fixture"
    )
    shutil.copyfile(output, unit)

    def escape(value):
        return (
            str(value)
            .replace("\\", "\\\\")
            .replace(" ", "\\ ")
            .replace("#", "\\#")
            .replace(":", "\\:")
        )

    value = environment.get(BACKEND_FIXTURE_INPUT)
    env_line = "# env-dep:" + BACKEND_FIXTURE_INPUT
    if value is not None:
        env_line += "=" + value.replace("\\", "\\\\").replace("\n", "\\n").replace(
            "\r", "\\r"
        )
    unit.with_suffix(".d").write_text(
        f"{escape(unit)}: {escape(source)}\n\n{escape(source)}:\n\n{env_line}\n",
        encoding="utf-8",
    )
    return (
        json.dumps(
            {
                "reason": "compiler-artifact",
                "package_id": f"path+{package.as_uri()}#molt-backend@0.1.0",
                "manifest_path": str(package / "Cargo.toml"),
                "target": {
                    "kind": ["bin"],
                    "name": "molt-backend",
                    "src_path": str(source),
                },
                "filenames": [str(unit), str(output)],
                "executable": str(output),
                "fresh": False,
            }
        )
        + "\n"
        + json.dumps({"reason": "build-finished", "success": True})
        + "\n"
    )
