from __future__ import annotations

from types import SimpleNamespace

from molt.exact_json import canonical_json_sha256


def compiler_build_admission(features=(), profile="release", environment=None):
    command = [
        "cargo",
        "build",
        "--locked",
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
    plan.environment = dict(environment or {})
    # This fixture models a fixed toolchain and features/profile/Rust flags.
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
            }
        ),
    )


def stub_compiler_admission(monkeypatch):
    from molt.cli import backend_binary, compiler_identity

    def admission(root, features, profile, env):
        return compiler_build_admission(features, profile, env)

    monkeypatch.setattr(backend_binary, "backend_build_admission", admission)
    monkeypatch.setattr(compiler_identity, "backend_build_admission", admission)


def write_compiler_lock(root):
    manifest = root / "runtime/molt-backend/Cargo.toml"
    if not manifest.exists():
        manifest.parent.mkdir(parents=True, exist_ok=True)
        manifest.write_text('[package]\nname = "molt-backend"\nversion = "0.1.0"\n')
    lock = root / "Cargo.lock"
    if not lock.exists():
        lock.write_text(
            'version = 4\n[[package]]\nname = "molt-backend"\nversion = "0.1.0"\n'
        )
