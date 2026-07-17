from __future__ import annotations

from pathlib import Path
import json
import os
import sys
from types import SimpleNamespace

import pytest

from molt import llvm_toolchain
from molt.llvm_toolchain import (
    LlvmToolchainConfigError,
    llvm_sys_prefix_env_var,
    mlir_sys_prefix_env_var,
    mlir_toolchain_environment,
    required_llvm_backend_pin,
    resolve_llvm_toolchain_prefix,
    tablegen_prefix_env_var,
    verify_llvm_toolchain_prefix,
    write_llvm_toolchain_attestation,
)


ROOT = Path(__file__).resolve().parents[1]


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _write_facade(root: Path, feature_values: str) -> None:
    _write(
        root / "runtime/molt-backend/Cargo.toml",
        f"""
[package]
name = "molt-backend"
version = "0.1.0"
edition = "2024"

[features]
llvm = [{feature_values}]
""".lstrip(),
    )


def _write_native(root: Path, inkwell_features: str, llvm_sys_version: str) -> None:
    _write(
        root / "runtime/molt-backend-native/Cargo.toml",
        f"""
[package]
name = "molt-backend-native"
version = "0.1.0"
edition = "2024"

[dependencies]
llvm-sys = {{ version = "{llvm_sys_version}", optional = true }}
inkwell = {{ version = "0.9", features = [{inkwell_features}], optional = true }}
""".lstrip(),
    )


def test_required_llvm_backend_pin_matches_current_manifest() -> None:
    pin = required_llvm_backend_pin(ROOT)

    assert pin is not None
    assert pin.major == 22
    assert pin.minor == 1
    assert pin.inkwell_feature == "llvm22-1"
    assert pin.env_var == "LLVM_SYS_221_PREFIX"
    assert pin.default_release == "22.1.8"


def test_llvm_sys_prefix_env_var_uses_major_and_minor() -> None:
    assert llvm_sys_prefix_env_var(22, 1) == "LLVM_SYS_221_PREFIX"
    assert llvm_sys_prefix_env_var(19, 0) == "LLVM_SYS_190_PREFIX"
    assert mlir_sys_prefix_env_var(22) == "MLIR_SYS_220_PREFIX"
    assert tablegen_prefix_env_var(22) == "TABLEGEN_220_PREFIX"


def test_mlir_environment_projects_one_prefix_to_every_binding(
    tmp_path: Path,
    monkeypatch,
) -> None:
    _write_facade(tmp_path, '"molt-backend-native/llvm"')
    _write_native(tmp_path, '"llvm22-1"', "221.0.1")
    prefix = tmp_path / "llvm 22"
    llvm_config = prefix / "bin" / "llvm-config.exe"
    _write(llvm_config, "")
    monkeypatch.setattr(
        "molt.llvm_toolchain.discover_llvm_toolchain",
        lambda *_args, **_kwargs: SimpleNamespace(
            llvm_config=llvm_config.resolve(),
            prefix=prefix.resolve(),
            version="22.1.8",
        ),
    )
    monkeypatch.setattr(
        "molt.llvm_toolchain.verify_llvm_toolchain_prefix",
        lambda *_args, **_kwargs: SimpleNamespace(
            llvm_config=llvm_config,
            prefix=prefix.resolve(),
            version="22.1.8",
        ),
    )
    monkeypatch.setattr(
        "molt.llvm_toolchain.required_llvm_targets_for_host",
        lambda _root: ("X86", "WebAssembly"),
    )

    env = mlir_toolchain_environment(
        tmp_path,
        environ={"MOLT_LLVM_PREFIX": str(prefix), "PATH": "system-bin"},
    )

    assert env["MOLT_LLVM_PREFIX"] == str(prefix.resolve())
    assert env["LLVM_SYS_221_PREFIX"] == str(prefix.resolve())
    assert env["MLIR_SYS_220_PREFIX"] == str(prefix.resolve())
    assert env["TABLEGEN_220_PREFIX"] == str(prefix.resolve())
    assert env["LLVM_CONFIG_PATH"] == str(llvm_config.resolve())
    assert env["PATH"].split(os.pathsep)[0] == str((prefix / "bin").resolve())


def test_mlir_environment_rejects_wrong_llvm_version(
    tmp_path: Path,
    monkeypatch,
) -> None:
    _write_facade(tmp_path, '"molt-backend-native/llvm"')
    _write_native(tmp_path, '"llvm22-1"', "221.0.1")
    prefix = tmp_path / "llvm 21"
    llvm_config = prefix / "bin" / "llvm-config.exe"
    _write(llvm_config, "")
    monkeypatch.setattr(
        "molt.llvm_toolchain.discover_llvm_toolchain",
        lambda *_args, **_kwargs: SimpleNamespace(
            llvm_config=llvm_config.resolve(),
            prefix=prefix.resolve(),
            version="21.1.8",
        ),
    )

    def reject_version(*_args, **_kwargs):
        raise LlvmToolchainConfigError("does not match")

    monkeypatch.setattr(
        "molt.llvm_toolchain.verify_llvm_toolchain_prefix",
        reject_version,
    )
    monkeypatch.setattr(
        "molt.llvm_toolchain.required_llvm_targets_for_host",
        lambda _root: ("X86", "WebAssembly"),
    )

    with pytest.raises(LlvmToolchainConfigError, match="does not match"):
        mlir_toolchain_environment(
            tmp_path,
            environ={"MOLT_LLVM_PREFIX": str(prefix)},
        )


def test_llvm_prefix_resolution_rejects_split_binding_families(
    tmp_path: Path,
) -> None:
    _write_facade(tmp_path, '"molt-backend-native/llvm"')
    _write_native(tmp_path, '"llvm22-1"', "221.0.1")

    with pytest.raises(LlvmToolchainConfigError, match="prefix authorities disagree"):
        resolve_llvm_toolchain_prefix(
            tmp_path,
            environ={
                "MOLT_LLVM_PREFIX": str(tmp_path / "llvm-a"),
                "MLIR_SYS_220_PREFIX": str(tmp_path / "llvm-b"),
            },
        )


def test_discovery_accepts_versioned_llvm_config_outside_matching_prefix(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _write_facade(tmp_path, '"molt-backend-native/llvm"')
    _write_native(tmp_path, '"llvm22-1"', "221.0.1")
    prefix = tmp_path / "usr" / "lib" / "llvm-22"
    external_config = tmp_path / "usr" / "bin" / "llvm-config-22"
    _write(external_config, "")
    monkeypatch.setattr(
        llvm_toolchain.shutil,
        "which",
        lambda name, **_kwargs: (
            str(external_config) if name == "llvm-config-22" else None
        ),
    )
    monkeypatch.setattr(
        llvm_toolchain,
        "_llvm_config_version",
        lambda _path: (22, 1, "22.1.8"),
    )
    monkeypatch.setattr(
        llvm_toolchain,
        "_llvm_config_prefix",
        lambda _path: prefix.resolve(),
    )

    discovery = llvm_toolchain.discover_llvm_toolchain(
        tmp_path,
        environ={
            "LLVM_SYS_221_PREFIX": str(prefix),
            "PATH": str(external_config.parent),
        },
    )

    assert discovery is not None
    assert discovery.prefix == prefix.resolve()
    assert discovery.llvm_config == external_config.resolve()
    assert discovery.source == "PATH"


def test_discovery_rejects_unrelated_llvm_sys_search_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _write_facade(tmp_path, '"molt-backend-native/llvm"')
    _write_native(tmp_path, '"llvm22-1"', "221.0.1")
    configured = tmp_path / "configured"
    other = tmp_path / "other"
    external_config = tmp_path / "bin" / "llvm-config-22"
    _write(external_config, "")
    monkeypatch.setattr(
        llvm_toolchain.shutil,
        "which",
        lambda name, **_kwargs: (
            str(external_config) if name == "llvm-config-22" else None
        ),
    )
    monkeypatch.setattr(
        llvm_toolchain,
        "_llvm_config_version",
        lambda _path: (22, 1, "22.1.8"),
    )
    monkeypatch.setattr(
        llvm_toolchain,
        "_llvm_config_prefix",
        lambda _path: other.resolve(),
    )

    with pytest.raises(LlvmToolchainConfigError, match="is neither the SDK prefix"):
        llvm_toolchain.discover_llvm_toolchain(
            tmp_path,
            environ={
                "LLVM_SYS_221_PREFIX": str(configured),
                "PATH": str(external_config.parent),
            },
        )


def test_required_llvm_backend_pin_follows_facade_to_native(tmp_path: Path) -> None:
    _write_facade(tmp_path, '"molt-backend-native/llvm"')
    _write_native(tmp_path, '"llvm22-1"', "221.0.1")

    pin = required_llvm_backend_pin(tmp_path)

    assert pin is not None
    assert pin.major == 22
    assert pin.minor == 1
    assert pin.env_var == "LLVM_SYS_221_PREFIX"


def test_required_llvm_backend_pin_rejects_facade_without_native_route(
    tmp_path: Path,
) -> None:
    _write_facade(tmp_path, '"dep:molt-backend-native"')
    _write_native(tmp_path, '"llvm22-1"', "221.0.1")

    with pytest.raises(LlvmToolchainConfigError, match="molt-backend-native/llvm"):
        required_llvm_backend_pin(tmp_path)


def test_required_llvm_backend_pin_rejects_conflicting_inkwell_features(
    tmp_path: Path,
) -> None:
    _write_facade(tmp_path, '"molt-backend-native/llvm"')
    _write_native(tmp_path, '"llvm21-1", "llvm22-1"', "221.0.1")

    with pytest.raises(LlvmToolchainConfigError, match="conflicting LLVM pins"):
        required_llvm_backend_pin(tmp_path)


def test_required_llvm_backend_pin_rejects_llvm_sys_mismatch(
    tmp_path: Path,
) -> None:
    _write_facade(tmp_path, '"molt-backend-native/llvm"')
    _write_native(tmp_path, '"llvm22-1"', "211.0.0")

    with pytest.raises(LlvmToolchainConfigError, match="does not match"):
        required_llvm_backend_pin(tmp_path)


def _write_complete_llvm_prefix(prefix: Path) -> None:
    suffix = ".exe" if os.name == "nt" else ""
    for name in (
        "llvm-config",
        "clang",
        "clang++",
        "ld.lld",
        "ld64.lld",
        "lld-link",
        "wasm-ld",
        "mlir-opt",
        "mlir-tblgen",
        "llvm-tblgen",
    ):
        _write(prefix / "bin" / f"{name}{suffix}", "")
    for directory in ("include/llvm", "include/mlir", "include/mlir-c"):
        (prefix / directory).mkdir(parents=True, exist_ok=True)
    _write(prefix / "include" / "llvm" / "Test.h", "header-a")
    _write(
        prefix / "include" / "llvm" / "IR" / "LLVMContext.h",
        "namespace llvm { class LLVMContext {}; }",
    )
    _write(prefix / "include" / "llvm-c" / "Core.h", "llvm-c-header-a")
    _write(prefix / "include" / "clang" / "Test.h", "clang-header-a")
    _write(prefix / "include" / "clang" / "Basic" / "Version.h", "clang-version-a")
    _write(prefix / "include" / "lld" / "Common" / "Driver.h", "lld-driver-a")
    _write(prefix / "include" / "mlir" / "IR" / "MLIRContext.h", "mlir-context-a")
    _write(prefix / "include" / "mlir-c" / "IR.h", "mlir-c-ir-a")
    _write(
        prefix / "lib" / "clang" / "22" / "include" / "stddef.h", "resource-header-a"
    )
    _write(prefix / "lib" / "LLVMCore.lib", "library-a")
    _write(prefix / "lib" / "MLIR-C.lib", "mlir-lib-a")
    _write(prefix / "lib" / "Polly.lib", "polly-lib-a")
    _write(prefix / "lib" / "lldCommon.lib", "lld-lib-a")


def _mock_tool_process_versions(monkeypatch: pytest.MonkeyPatch) -> None:
    def run(command, **_kwargs):
        name = Path(command[0]).name.lower()
        if name.startswith("llvm-config"):
            output = "22.1.8\n"
        elif name.startswith("clang"):
            output = "clang version 22.1.8\n"
        elif "lld" in name:
            output = "LLD 22.1.8\n"
        else:
            output = "LLVM (test):\n  LLVM version 22.1.8\n"
        return SimpleNamespace(returncode=0, stdout=output, stderr="")

    monkeypatch.setattr(
        "molt.llvm_toolchain.subprocess.run",
        run,
    )


def _mock_llvm_config(
    prefix: Path,
    monkeypatch: pytest.MonkeyPatch,
    *,
    version: str = "22.1.8",
    targets: str = "X86 WebAssembly",
) -> None:
    def run(_executable: Path, *arguments: str) -> str:
        if arguments == ("--version",):
            return version
        if arguments == ("--targets-built",):
            return targets
        if arguments == ("--link-static", "--libs", "core", "support"):
            return str(prefix / "lib" / "LLVMCore.lib")
        if arguments == ("--system-libs",):
            return "kernel32.lib"
        raise AssertionError(arguments)

    monkeypatch.setattr("molt.llvm_toolchain._run_llvm_config", run)


def test_complete_prefix_verifier_and_attestation_share_one_contract(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    _mock_tool_process_versions(monkeypatch)

    _mock_llvm_config(prefix, monkeypatch)
    verification = verify_llvm_toolchain_prefix(
        ROOT,
        prefix,
        expected_targets=("X86", "WebAssembly"),
    )
    attestation = write_llvm_toolchain_attestation(
        ROOT,
        verification,
        projects=("clang", "lld", "mlir", "polly"),
    )

    assert attestation.is_file()
    payload = json.loads(attestation.read_text(encoding="utf-8"))
    assert payload["custody"] == "manifest-release-noncanonical-prefix"
    assert payload["build_config"] == {
        "projects": ["clang", "lld", "mlir", "polly"],
        "targets": ["WebAssembly", "X86"],
        "build_type": "Release",
    }
    assert (
        payload["release_manifest_sha256"]
        == llvm_toolchain.load_llvm_releases(ROOT).digest
    )
    assert payload["release"] == {
        "version": "22.1.8",
        "url": (
            "https://github.com/llvm/llvm-project/releases/download/"
            "llvmorg-22.1.8/llvm-project-22.1.8.src.tar.xz"
        ),
        "size": 167061596,
        "source_sha256": (
            "922f1817a0df7b1489272d18134ee0087a8b068828f87ac63b9861b1a9965888"
        ),
        "provenance_url": (
            "https://api.github.com/repos/llvm/llvm-project/releases/tags/llvmorg-22.1.8"
        ),
        "record_sha256": payload["release"]["record_sha256"],
    }
    assert payload["link_probe"][:2] == [
        "language:c++17",
        f"linker:bin/{'lld-link.exe' if os.name == 'nt' else 'ld64.lld' if sys.platform == 'darwin' else 'ld.lld'}",
    ]
    assert (
        verify_llvm_toolchain_prefix(
            ROOT,
            prefix,
            expected_targets=("X86", "WebAssembly"),
            require_attestation=True,
        )
        == verification
    )
    full_verification = verify_llvm_toolchain_prefix(
        ROOT,
        prefix,
        expected_targets=("X86", "WebAssembly"),
        require_attestation=True,
        content_policy="full",
    )
    assert full_verification.content_digest
    assert all(fact.sha256 for fact in full_verification.content_facts)


def test_prefix_verifier_preserves_external_llvm_config_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "usr" / "lib" / "llvm-22"
    llvm_config = tmp_path / "usr" / "bin" / "llvm-config-22"
    _write_complete_llvm_prefix(prefix)
    _write(llvm_config, "external-config")
    _mock_tool_process_versions(monkeypatch)
    _mock_llvm_config(prefix, monkeypatch)

    verification = verify_llvm_toolchain_prefix(
        ROOT,
        prefix,
        expected_targets=("X86", "WebAssembly"),
        llvm_config_override=llvm_config,
    )

    fact = next(
        item for item in verification.tool_versions if item.role == "llvm-config"
    )
    assert verification.llvm_config == llvm_config.resolve()
    assert fact.path == f"external:{llvm_config.resolve()}"
    assert f"external:{llvm_config.resolve()}" in verification.assets
    projected = llvm_toolchain.project_llvm_toolchain_environment(
        ROOT, verification, environ={"PATH": str(llvm_config.parent)}
    )
    assert projected["LLVM_SYS_221_PREFIX"] == str(llvm_config.parent.parent)
    assert projected["MLIR_SYS_220_PREFIX"] == str(prefix)


def test_debian_packages_cover_manifest_owned_sdk_components() -> None:
    assert llvm_toolchain.llvm_debian_dev_packages(ROOT, 22) == (
        "clang-22",
        "libclang-22-dev",
        "liblld-22-dev",
        "libmlir-22-dev",
        "libpolly-22-dev",
        "lld-22",
        "llvm-22-dev",
        "mlir-22-tools",
    )


def test_debian_installer_identity_is_manifest_owned(tmp_path: Path) -> None:
    installer = llvm_toolchain.load_llvm_releases(ROOT).debian_installer

    assert installer.url == "https://apt.llvm.org/llvm.sh"
    assert installer.sha256 == (
        "9474ecd78b52aba6e923976b1e9773f5613027cc7e237b9956986cb536e02a36"
    )
    github_output = tmp_path / "github-output"
    assert (
        llvm_toolchain.main(
            ["--root", str(ROOT), "--github-output", str(github_output)]
        )
        == 0
    )
    projected = github_output.read_text(encoding="utf-8")
    assert f"apt_installer_url={installer.url}\n" in projected
    assert f"apt_installer_sha256={installer.sha256}\n" in projected


def test_cli_projects_verified_sdk_identity_to_github_environment(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    prefix = tmp_path / "llvm-22"
    llvm_config = tmp_path / "bin" / "llvm-config-22"
    verification = llvm_toolchain.LlvmPrefixVerification(
        prefix=prefix,
        llvm_config=llvm_config,
        version="22.1.8",
        targets=("WebAssembly", "X86"),
        assets=(),
        tool_versions=(),
        library_facts=(),
        content_facts=(),
        content_digest=None,
        link_closure=(),
        link_probe=(),
        release=None,
    )
    monkeypatch.setattr(
        llvm_toolchain,
        "verify_available_llvm_toolchain",
        lambda _root: verification,
    )
    github_env = tmp_path / "github-env"

    assert (
        llvm_toolchain.main(
            [
                "--root",
                str(ROOT),
                "--verify",
                "--format",
                "json",
                "--github-env",
                str(github_env),
            ]
        )
        == 0
    )

    projected = github_env.read_text(encoding="utf-8")
    assert f"MOLT_LLVM_PREFIX={prefix}" in projected
    assert f"LLVM_SYS_221_PREFIX={llvm_config.parent.parent}" in projected
    assert f"LLVM_CONFIG_PATH={llvm_config}" in projected
    assert json.loads(capsys.readouterr().out)["llvm_config"] == str(llvm_config)


def test_cached_attestation_projects_compile_link_proof_without_rerunning_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    _mock_tool_process_versions(monkeypatch)
    _mock_llvm_config(prefix, monkeypatch)
    verification = verify_llvm_toolchain_prefix(
        ROOT,
        prefix,
        expected_targets=("X86", "WebAssembly"),
        content_policy="full",
    )
    write_llvm_toolchain_attestation(
        ROOT,
        verification,
        projects=("clang", "lld", "mlir", "polly"),
    )
    monkeypatch.setattr(
        "molt.llvm_toolchain._compile_link_probe",
        lambda *_args, **_kwargs: pytest.fail("cached proof reran compile-link probe"),
    )

    cached = verify_llvm_toolchain_prefix(
        ROOT,
        prefix,
        expected_targets=("X86", "WebAssembly"),
        require_attestation=True,
        content_policy="cached",
    )
    assert cached.link_probe == verification.link_probe


def test_attestation_projects_one_full_content_verification_without_rehashing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    _mock_tool_process_versions(monkeypatch)
    _mock_llvm_config(prefix, monkeypatch)
    calls: list[bool] = []
    real_content_manifest = llvm_toolchain._content_manifest

    def counted(prefix_arg, paths, *, hash_contents):
        calls.append(hash_contents)
        return real_content_manifest(prefix_arg, paths, hash_contents=hash_contents)

    monkeypatch.setattr("molt.llvm_toolchain._content_manifest", counted)
    verification = verify_llvm_toolchain_prefix(
        ROOT,
        prefix,
        expected_targets=("X86", "WebAssembly"),
        content_policy="full",
    )
    write_llvm_toolchain_attestation(
        ROOT,
        verification,
        projects=("clang", "lld", "mlir", "polly"),
    )

    assert calls.count(True) == 1
    assert calls.count(False) == 1


def test_attestation_publication_rejects_tool_drift_after_verification(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    _mock_tool_process_versions(monkeypatch)
    _mock_llvm_config(prefix, monkeypatch)
    verification = verify_llvm_toolchain_prefix(
        ROOT,
        prefix,
        expected_targets=("X86", "WebAssembly"),
    )
    suffix = ".exe" if os.name == "nt" else ""
    _write(prefix / "bin" / f"clang{suffix}", "substituted")

    with pytest.raises(LlvmToolchainConfigError, match="changed before attestation"):
        write_llvm_toolchain_attestation(
            ROOT,
            verification,
            projects=("clang", "lld", "mlir", "polly"),
        )


def test_attestation_publication_rejects_header_drift_after_full_verification(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    _mock_tool_process_versions(monkeypatch)
    _mock_llvm_config(prefix, monkeypatch)
    verification = verify_llvm_toolchain_prefix(
        ROOT,
        prefix,
        expected_targets=("X86", "WebAssembly"),
        content_policy="full",
    )
    header = prefix / "include" / "llvm" / "Test.h"
    original = header.stat()
    _write(header, "header-b")
    os.utime(header, ns=(original.st_atime_ns, original.st_mtime_ns))

    with pytest.raises(LlvmToolchainConfigError, match="changed before attestation"):
        write_llvm_toolchain_attestation(
            ROOT,
            verification,
            projects=("clang", "lld", "mlir", "polly"),
        )


def test_verifier_refuses_manifest_release_with_mixed_patch_tools(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    _mock_llvm_config(prefix, monkeypatch)

    def run(command, **_kwargs):
        if "-std=c++17" in command:
            return SimpleNamespace(returncode=0, stdout="", stderr="")
        name = Path(command[0]).name.lower()
        output = (
            "22.1.8\n"
            if name.startswith("llvm-config")
            else "clang version 22.1.9\n"
            if name.startswith("clang")
            else "LLD 22.1.8\n"
            if "lld" in name
            else "LLVM version 22.1.8\n"
        )
        return SimpleNamespace(returncode=0, stdout=output, stderr="")

    monkeypatch.setattr("molt.llvm_toolchain.subprocess.run", run)
    with pytest.raises(LlvmToolchainConfigError, match="expected exactly 22.1.8"):
        verify_llvm_toolchain_prefix(
            ROOT,
            prefix,
            expected_targets=("X86", "WebAssembly"),
            content_policy="full",
        )


def test_staged_attestation_targets_published_prefix(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    staging = tmp_path / ".llvm.staging"
    published = tmp_path / "llvm"
    _write_complete_llvm_prefix(staging)
    _mock_tool_process_versions(monkeypatch)
    _mock_llvm_config(staging, monkeypatch)
    verification = verify_llvm_toolchain_prefix(
        ROOT,
        staging,
        expected_targets=("X86", "WebAssembly"),
        content_policy="full",
    )

    attestation = write_llvm_toolchain_attestation(
        ROOT,
        verification,
        projects=("clang", "lld", "mlir", "polly"),
        published_prefix=published,
    )
    payload = json.loads(attestation.read_text(encoding="utf-8"))

    assert payload["prefix"] == str(published.resolve())
    assert payload["custody"] == "manifest-release-noncanonical-prefix"
    assert payload["llvm_config"] == str(
        published.resolve() / verification.llvm_config.relative_to(staging.resolve())
    )


def test_complete_prefix_verifier_rejects_missing_linker(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    _mock_tool_process_versions(monkeypatch)
    suffix = ".exe" if os.name == "nt" else ""
    host_linker = (
        "lld-link"
        if os.name == "nt"
        else "ld64.lld"
        if sys.platform == "darwin"
        else "ld.lld"
    )
    (prefix / "bin" / f"{host_linker}{suffix}").unlink()
    _mock_llvm_config(prefix, monkeypatch)

    with pytest.raises(LlvmToolchainConfigError, match="missing required tool"):
        verify_llvm_toolchain_prefix(
            ROOT,
            prefix,
            expected_targets=("X86", "WebAssembly"),
        )


def test_complete_prefix_verifier_rejects_missing_mlir_c_sdk_header(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    (prefix / "include" / "mlir-c" / "IR.h").unlink()
    _mock_tool_process_versions(monkeypatch)
    _mock_llvm_config(prefix, monkeypatch)

    with pytest.raises(LlvmToolchainConfigError, match="missing required SDK file"):
        verify_llvm_toolchain_prefix(
            ROOT,
            prefix,
            expected_targets=("X86", "WebAssembly"),
        )


def test_complete_prefix_verifier_rejects_mismatched_tool_version(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    _mock_llvm_config(prefix, monkeypatch)
    monkeypatch.setattr(
        "molt.llvm_toolchain.subprocess.run",
        lambda command, **_kwargs: SimpleNamespace(
            returncode=0,
            stdout=(
                "21.1.7\n"
                if Path(command[0]).name.lower().startswith("llvm-config")
                else "clang version 22.1.8\n"
            ),
            stderr="",
        ),
    )

    with pytest.raises(LlvmToolchainConfigError, match="reports 21.1.7"):
        verify_llvm_toolchain_prefix(
            ROOT,
            prefix,
            expected_targets=("X86", "WebAssembly"),
        )


def test_canonical_prefix_requires_exact_patch_for_every_companion_tool(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    _mock_llvm_config(prefix, monkeypatch)
    monkeypatch.setattr(
        "molt.llvm_toolchain.managed_llvm_paths",
        lambda *_args, **_kwargs: SimpleNamespace(prefix=prefix),
    )

    def run(command, **_kwargs):
        if "-std=c++17" in command:
            return SimpleNamespace(returncode=0, stdout="", stderr="")
        name = Path(command[0]).name.lower()
        output = (
            "22.1.8\n"
            if name.startswith("llvm-config")
            else "clang version 22.1.9\n"
            if name.startswith("clang")
            else "LLD 22.1.8\n"
            if "lld" in name
            else "LLVM version 22.1.8\n"
        )
        return SimpleNamespace(returncode=0, stdout=output, stderr="")

    monkeypatch.setattr("molt.llvm_toolchain.subprocess.run", run)
    with pytest.raises(LlvmToolchainConfigError, match="expected exactly 22.1.8"):
        verify_llvm_toolchain_prefix(
            ROOT,
            prefix,
            expected_targets=("X86", "WebAssembly"),
        )


def test_compile_link_probe_uses_verified_host_linker_by_absolute_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    clangxx = prefix / "bin" / ("clang++.exe" if os.name == "nt" else "clang++")
    linker = (
        prefix
        / "bin"
        / (
            "lld-link.exe"
            if os.name == "nt"
            else "ld64.lld"
            if sys.platform == "darwin"
            else "ld.lld"
        )
    )
    library = prefix / "lib" / "LLVMCore.lib"
    for path in (clangxx, linker, library):
        _write(path, "")
    commands: list[list[str]] = []

    def run(command, **_kwargs):
        commands.append(command)
        return SimpleNamespace(returncode=0, stdout="", stderr="")

    monkeypatch.setattr("molt.llvm_toolchain.subprocess.run", run)
    proof = llvm_toolchain._compile_link_probe(
        prefix,
        clangxx,
        linker,
        ("lib/LLVMCore.lib",),
    )

    assert f"--ld-path={linker}" in commands[0]
    assert proof[1] == f"linker:bin/{linker.name}"


def test_windows_style_llvm_config_link_closure_resolves_dot_lib(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    library = prefix / "lib" / "LLVMCore.lib"
    _write(library, "library")

    def run(_executable: Path, *arguments: str) -> str:
        if arguments == ("--link-static", "--libs", "core", "support"):
            return "-lLLVMCore"
        if arguments == ("--system-libs",):
            return ""
        raise AssertionError(arguments)

    monkeypatch.setattr("molt.llvm_toolchain._run_llvm_config", run)
    rendered, paths = llvm_toolchain._llvm_link_closure(
        prefix, prefix / "bin" / "llvm-config"
    )
    assert rendered == ("lib/LLVMCore.lib",)
    assert paths == (library.resolve(),)


@pytest.mark.skipif(os.name != "nt", reason="Windows llvm-config quoting policy")
def test_windows_llvm_config_link_closure_preserves_quoted_absolute_paths(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "prefix with spaces"
    library = prefix / "lib" / "LLVMCore.lib"
    _write(library, "library")

    def run(_executable: Path, *arguments: str) -> str:
        if arguments == ("--link-static", "--libs", "core", "support"):
            return f'"{library}"'
        if arguments == ("--system-libs",):
            return '"C:\\Program Files\\Windows Kits\\kernel32.lib"'
        raise AssertionError(arguments)

    monkeypatch.setattr("molt.llvm_toolchain._run_llvm_config", run)
    rendered, paths = llvm_toolchain._llvm_link_closure(
        prefix, prefix / "bin" / "llvm-config.exe"
    )
    assert rendered == (
        "lib/LLVMCore.lib",
        r"system:C:\Program Files\Windows Kits\kernel32.lib",
    )
    assert paths == (library.resolve(),)


def test_complete_prefix_verifier_requires_real_compile_link_probe(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    _mock_llvm_config(prefix, monkeypatch)

    def run(command, **_kwargs):
        if "-std=c++17" in command:
            return SimpleNamespace(
                returncode=1,
                stdout="",
                stderr="undefined reference to llvm::LLVMContext",
            )
        name = Path(command[0]).name.lower()
        output = (
            "22.1.8\n"
            if name.startswith("llvm-config")
            else "LLD 22.1.8\n"
            if "lld" in name
            else "clang version 22.1.8\n"
            if name.startswith("clang")
            else "LLVM version 22.1.8\n"
        )
        return SimpleNamespace(returncode=0, stdout=output, stderr="")

    monkeypatch.setattr("molt.llvm_toolchain.subprocess.run", run)
    with pytest.raises(LlvmToolchainConfigError, match="compile and link together"):
        verify_llvm_toolchain_prefix(
            ROOT,
            prefix,
            expected_targets=("X86", "WebAssembly"),
        )


def test_canonical_prefix_requires_exact_manifest_patch_release(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    _mock_tool_process_versions(monkeypatch)
    _mock_llvm_config(prefix, monkeypatch, version="22.1.9")
    monkeypatch.setattr(
        "molt.llvm_toolchain.managed_llvm_paths",
        lambda *_args, **_kwargs: SimpleNamespace(prefix=prefix),
    )

    with pytest.raises(LlvmToolchainConfigError, match="expected exactly 22.1.8"):
        verify_llvm_toolchain_prefix(
            ROOT,
            prefix,
            version="22.1.8",
            expected_targets=("X86", "WebAssembly"),
        )


def test_canonical_prefix_rejects_extra_built_target_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    _mock_tool_process_versions(monkeypatch)
    _mock_llvm_config(prefix, monkeypatch, targets="AArch64 WebAssembly X86")
    monkeypatch.setattr(
        "molt.llvm_toolchain.managed_llvm_paths",
        lambda *_args, **_kwargs: SimpleNamespace(prefix=prefix),
    )

    with pytest.raises(LlvmToolchainConfigError, match="target set must match exactly"):
        verify_llvm_toolchain_prefix(
            ROOT,
            prefix,
            version="22.1.8",
            expected_targets=("X86", "WebAssembly"),
        )


def test_all_explicit_prefix_authorities_reject_retired_d_drive() -> None:
    pin = required_llvm_backend_pin(ROOT)
    assert pin is not None
    names = (
        "MOLT_LLVM_PREFIX",
        pin.env_var,
        mlir_sys_prefix_env_var(pin.major),
        tablegen_prefix_env_var(pin.major),
        "MOLT_TARGET_ROOT",
        "LLVM_CONFIG_PATH",
    )
    for name in names:
        for poisoned in (r"D:\poison", r"d:/poison", r"\\?\D:\poison"):
            with pytest.raises(
                LlvmToolchainConfigError, match="retired D: canonical custody"
            ):
                resolve_llvm_toolchain_prefix(ROOT, environ={name: poisoned})


def test_path_discovery_rejects_retired_d_drive_before_execution(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _write_facade(tmp_path, '"molt-backend-native/llvm"')
    _write_native(tmp_path, '"llvm22-1"', "221.0.1")
    monkeypatch.setattr(
        llvm_toolchain,
        "managed_llvm_prefix",
        lambda *_args, **_kwargs: tmp_path / "missing",
    )
    monkeypatch.setattr(
        llvm_toolchain.shutil,
        "which",
        lambda name, **_kwargs: (
            r"D:\poison\llvm-config-22.exe"
            if name.startswith("llvm-config-22")
            else None
        ),
    )
    with pytest.raises(LlvmToolchainConfigError, match="retired D: canonical custody"):
        llvm_toolchain.discover_llvm_toolchain(tmp_path, environ={"PATH": r"D:\poison"})


def test_managed_attestation_rejects_live_asset_drift(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    _mock_tool_process_versions(monkeypatch)
    _mock_llvm_config(prefix, monkeypatch)
    verification = verify_llvm_toolchain_prefix(
        ROOT,
        prefix,
        expected_targets=("X86", "WebAssembly"),
    )
    write_llvm_toolchain_attestation(
        ROOT,
        verification,
        projects=("clang", "lld", "mlir", "polly"),
    )
    _write(prefix / "lib" / "MLIRDrift.lib", "drift")

    with pytest.raises(LlvmToolchainConfigError, match="attestation drift"):
        verify_llvm_toolchain_prefix(
            ROOT,
            prefix,
            expected_targets=("X86", "WebAssembly"),
            require_attestation=True,
        )


def test_content_manifest_hashes_distinct_paths_when_inode_identity_is_zero(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    left = prefix / "include" / "left.h"
    right = prefix / "include" / "right.h"
    _write(left, "left")
    _write(right, "rght")
    real_stat = Path.stat
    targets = {str(left.absolute()), str(right.absolute())}

    def zero_inode(path: Path, *args, **kwargs):
        observed = real_stat(path, *args, **kwargs)
        if str(path.absolute()) not in targets:
            return observed
        return SimpleNamespace(
            st_dev=0,
            st_ino=0,
            st_size=observed.st_size,
            st_mtime_ns=observed.st_mtime_ns,
            st_ctime_ns=observed.st_ctime_ns,
        )

    monkeypatch.setattr(Path, "stat", zero_inode)
    facts, digest = llvm_toolchain._content_manifest(
        prefix,
        (left, right),
        hash_contents=True,
    )
    assert digest is not None
    assert facts[0].sha256 != facts[1].sha256


def test_managed_attestation_rejects_substituted_tool(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    _mock_tool_process_versions(monkeypatch)
    _mock_llvm_config(prefix, monkeypatch)
    verification = verify_llvm_toolchain_prefix(
        ROOT,
        prefix,
        expected_targets=("X86", "WebAssembly"),
    )
    write_llvm_toolchain_attestation(
        ROOT,
        verification,
        projects=("clang", "lld", "mlir", "polly"),
    )
    suffix = ".exe" if os.name == "nt" else ""
    _write(prefix / "bin" / f"clang{suffix}", "substituted")

    with pytest.raises(LlvmToolchainConfigError, match="attestation drift"):
        verify_llvm_toolchain_prefix(
            ROOT,
            prefix,
            expected_targets=("X86", "WebAssembly"),
            require_attestation=True,
        )


@pytest.mark.parametrize(
    ("relative_path", "replacement"),
    (
        ("include/llvm/Test.h", "header-b"),
        ("include/clang/Test.h", "clang-header-b"),
        ("lib/clang/22/include/stddef.h", "resource-header-b"),
        ("lib/LLVMCore.lib", "library-b"),
    ),
)
def test_managed_attestation_rejects_same_size_content_substitution_with_restored_mtime(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    relative_path: str,
    replacement: str,
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    _mock_tool_process_versions(monkeypatch)
    _mock_llvm_config(prefix, monkeypatch)
    verification = verify_llvm_toolchain_prefix(
        ROOT,
        prefix,
        expected_targets=("X86", "WebAssembly"),
    )
    write_llvm_toolchain_attestation(
        ROOT,
        verification,
        projects=("clang", "lld", "mlir", "polly"),
    )
    target = prefix / relative_path
    original = target.stat()
    _write(target, replacement)
    os.utime(target, ns=(original.st_atime_ns, original.st_mtime_ns))

    with pytest.raises(LlvmToolchainConfigError, match="content digest drift"):
        verify_llvm_toolchain_prefix(
            ROOT,
            prefix,
            expected_targets=("X86", "WebAssembly"),
            require_attestation=True,
        )


def test_unavailable_ntfs_change_time_forces_content_hashing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    _write_complete_llvm_prefix(prefix)
    _mock_tool_process_versions(monkeypatch)
    _mock_llvm_config(prefix, monkeypatch)
    verification = verify_llvm_toolchain_prefix(
        ROOT,
        prefix,
        expected_targets=("X86", "WebAssembly"),
    )
    write_llvm_toolchain_attestation(
        ROOT,
        verification,
        projects=("clang", "lld", "mlir", "polly"),
    )
    target = prefix / "include" / "llvm" / "Test.h"
    original = target.stat()
    _write(target, "header-b")
    os.utime(target, ns=(original.st_atime_ns, original.st_mtime_ns))
    monkeypatch.setattr(
        "molt.llvm_toolchain._content_change_time_ns", lambda *_args: None
    )

    with pytest.raises(LlvmToolchainConfigError, match="content digest drift"):
        verify_llvm_toolchain_prefix(
            ROOT,
            prefix,
            expected_targets=("X86", "WebAssembly"),
            require_attestation=True,
            content_policy="cached",
        )
