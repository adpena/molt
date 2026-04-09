import base64
import json
import os
import platform
import shutil
import socketserver
import subprocess
import sys
import threading
import tempfile
import time
import zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest
import molt.cli as cli


ROOT = Path(__file__).resolve().parents[2]


def _base_env() -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(ROOT / "src")
    # Route nested CLI calls through one deterministic smoke-test session so
    # they do not inherit a developer session's daemon/target state while still
    # reusing the same isolated build artifacts across smoke cases.
    env["MOLT_SESSION_ID"] = "tests-cli-smoke"
    env.pop("CARGO_TARGET_DIR", None)
    env.pop("MOLT_DIFF_CARGO_TARGET_DIR", None)
    return env


def _python_executable() -> str:
    exe = sys.executable
    if exe and os.path.exists(exe) and os.access(exe, os.X_OK):
        return exe
    fallback = shutil.which("python3") or shutil.which("python")
    if fallback:
        return fallback
    return exe


def _cli_timeout() -> float | None:
    raw = os.environ.get("MOLT_CLI_TEST_TIMEOUT")
    if not raw:
        return None
    try:
        timeout = float(raw)
    except ValueError as exc:
        raise RuntimeError(f"Invalid MOLT_CLI_TEST_TIMEOUT value: {raw}") from exc
    if timeout <= 0:
        raise RuntimeError("MOLT_CLI_TEST_TIMEOUT must be greater than zero.")
    return timeout


def _run_cli(args: list[str]) -> subprocess.CompletedProcess[str]:
    cmd = [_python_executable(), "-m", "molt.cli", *args]
    return subprocess.run(
        cmd,
        cwd=ROOT,
        env=_base_env(),
        capture_output=True,
        text=True,
        timeout=_cli_timeout(),
    )


def _run_cli_with_timeout(
    args: list[str], timeout: float
) -> subprocess.CompletedProcess[str]:
    cmd = [_python_executable(), "-m", "molt.cli", *args]
    return subprocess.run(
        cmd,
        cwd=ROOT,
        env=_base_env(),
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def _write_trust_policy(tmp_path: Path, key_sha: str) -> Path:
    policy_path = tmp_path / "trust_policy.toml"
    policy_path.write_text(
        "\n".join(
            [
                "[cosign]",
                "keys = [",
                f'  "sha256:{key_sha}",',
                "]",
                "",
            ]
        )
    )
    return policy_path


def _write_pgo_profile(tmp_path: Path, entrypoint: str = "script.py") -> Path:
    profile_path = tmp_path / "molt_profile.json"
    payload = {
        "molt_profile_version": "0.1",
        "created_at_utc": "2026-02-03T00:00:00Z",
        "python_implementation": "CPython",
        "python_version": "3.12.0",
        "platform": {
            "os": platform.system().lower(),
            "arch": platform.machine().lower() or "unknown",
        },
        "run_metadata": {
            "entrypoint": entrypoint,
            "argv": [],
            "env_fingerprint": "sha256:deadbeef",
            "inputs_fingerprint": "sha256:deadbeef",
            "duration_ms": 1,
        },
        "modules": {},
        "symbols": {},
        "call_sites": [],
        "types": {},
        "containers": {},
        "hotspots": [{"symbol": "molt_init___main__", "score": 1}],
        "events": [],
        "redactions": {},
    }
    profile_path.write_text(json.dumps(payload))
    return profile_path


def _write_runtime_feedback(tmp_path: Path) -> Path:
    feedback_path = tmp_path / "molt_runtime_feedback.json"
    payload = {
        "schema_version": 1,
        "kind": "runtime_feedback",
        "profile": {
            "call_dispatch": 0,
            "attr_lookup": 0,
            "layout_guard": 0,
            "layout_guard_fail": 0,
            "alloc_count": 0,
            "async_polls": 0,
        },
        "hot_paths": {
            "call_bind_ic_hit": 0,
            "call_bind_ic_miss": 0,
            "split_ws_ascii": 0,
            "split_ws_unicode": 0,
            "dict_str_int_prehash_deopt": 0,
            "taq_ingest_calls": 0,
        },
        "deopt_reasons": {
            "call_indirect_noncallable": 0,
            "invoke_ffi_bridge_capability_denied": 0,
            "guard_tag_type_mismatch": 0,
            "guard_dict_shape_layout_mismatch": 0,
            "guard_dict_shape_layout_fail_null_obj": 0,
            "guard_dict_shape_layout_fail_non_object": 0,
            "guard_dict_shape_layout_fail_class_mismatch": 0,
            "guard_dict_shape_layout_fail_non_type_class": 0,
            "guard_dict_shape_layout_fail_expected_version_invalid": 0,
            "guard_dict_shape_layout_fail_version_mismatch": 0,
        },
        "hot_functions": [
            {"symbol": "molt_init___main__", "count": 11},
            {"symbol": "helper", "count": 3},
        ],
    }
    feedback_path.write_text(json.dumps(payload))
    return feedback_path


def _inject_signature_metadata(package_path: Path, key_sha: str) -> None:
    signature_meta: dict[str, object] = {}
    with zipfile.ZipFile(package_path) as zf:
        entries = {
            name: zf.read(name) for name in zf.namelist() if name != "signature.json"
        }
    manifest = json.loads(entries["manifest.json"].decode("utf-8"))
    checksum = manifest.get("checksum")
    artifact_name = None
    artifact_entries = [name for name in entries if name.startswith("artifact/")]
    if artifact_entries:
        artifact_name = artifact_entries[0].split("/", 1)[1]
    signature_meta = {
        "schema_version": 1,
        "artifact": {
            "path": artifact_name or "artifact.bin",
            "sha256": checksum or "",
        },
        "status": "signed",
        "signature": {"status": "signed", "algorithm": "sha256"},
        "signer": {
            "tool": {"name": "cosign"},
            "key": {"sha256": f"sha256:{key_sha}"},
        },
    }
    with tempfile.NamedTemporaryFile(
        prefix=package_path.stem,
        suffix=".moltpkg",
        dir=package_path.parent,
        delete=False,
    ) as tmp_handle:
        tmp_path = Path(tmp_handle.name)
    try:
        with zipfile.ZipFile(tmp_path, mode="w") as zf:
            for name, data in entries.items():
                zf.writestr(name, data)
            zf.writestr("signature.json", json.dumps(signature_meta))
        tmp_path.replace(package_path)
    finally:
        if tmp_path.exists():
            tmp_path.unlink(missing_ok=True)
    sidecar = package_path.with_name(package_path.stem + ".sig.json")
    sidecar.write_text(json.dumps(signature_meta))


def _start_registry_server() -> tuple[socketserver.TCPServer, list[dict[str, object]]]:
    uploads: list[dict[str, object]] = []

    class RegistryHandler(BaseHTTPRequestHandler):
        def do_PUT(self) -> None:  # noqa: N802
            length = int(self.headers.get("Content-Length", "0"))
            body = self.rfile.read(length)
            uploads.append(
                {
                    "path": self.path,
                    "headers": dict(self.headers),
                    "body": body,
                }
            )
            self.send_response(200)
            self.end_headers()

        def log_message(self, format: str, *args: object) -> None:  # noqa: A003
            return

    server = socketserver.TCPServer(("127.0.0.1", 0), RegistryHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, uploads


def _cross_target_triple() -> str | None:
    system = platform.system()
    arch = platform.machine().lower()
    arch_map = {
        "arm64": "aarch64",
        "aarch64": "aarch64",
        "x86_64": "x86_64",
        "amd64": "x86_64",
    }
    mapped = arch_map.get(arch)
    if mapped is None:
        return None
    if system == "Darwin":
        return f"{mapped}-apple-darwin"
    if system == "Linux":
        return f"{mapped}-unknown-linux-gnu"
    return None


@pytest.mark.parametrize(
    ("triple", "expected"),
    [
        ("x86_64-unknown-linux-gnu", "x86_64-linux-gnu"),
        ("aarch64-apple-darwin", "aarch64-macos"),
        ("armv7-unknown-linux-gnueabihf", "armv7-linux-gnueabihf"),
        ("x86_64-w64-mingw32", "x86_64-windows-gnu"),
        ("x86_64-pc-windows-msvc", "x86_64-windows-msvc"),
        ("wasm32-unknown-unknown", "wasm32-freestanding"),
        ("wasm32-wasi", "wasm32-wasi"),
        ("arm64-apple-ios-simulator", "aarch64-ios-simulator"),
    ],
)
def test_zig_target_query_mapping(triple: str, expected: str) -> None:
    assert cli._zig_target_query(triple) == expected


def test_resolve_output_path_directory(tmp_path: Path) -> None:
    default = tmp_path / "default.bin"
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    resolved = cli._resolve_output_path(
        "out",
        default,
        out_dir=None,
        project_root=tmp_path,
    )
    assert resolved == out_dir / default.name


def test_resolve_output_path_trailing_sep(tmp_path: Path) -> None:
    default = tmp_path / "output.wasm"
    resolved = cli._resolve_output_path(
        "dist" + os.sep,
        default,
        out_dir=None,
        project_root=tmp_path,
    )
    assert resolved == tmp_path / "dist" / default.name


def test_resolve_output_path_uses_out_dir(tmp_path: Path) -> None:
    default = tmp_path / "output.o"
    out_dir = tmp_path / "artifacts"
    out_dir.mkdir()
    resolved = cli._resolve_output_path(
        "obj",
        default,
        out_dir=out_dir,
        project_root=tmp_path,
    )
    assert resolved == out_dir / "obj"


def test_resolve_output_roots_defaults_final_outputs_to_dist(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / ".molt_cache"))

    _artifacts_root, bin_root, output_root = cli._resolve_output_roots(
        tmp_path, None, "app"
    )

    assert output_root == tmp_path / "dist"
    assert bin_root == cli._default_molt_bin()


def test_resolve_build_output_layout_defaults_wasm_outputs_to_dist(
    tmp_path: Path,
) -> None:
    layout = cli._resolve_build_output_layout(
        target="wasm",
        trusted=False,
        require_linked=False,
        linked=False,
        linked_output=None,
        emit=None,
        output=None,
        emit_ir=None,
        artifacts_root=tmp_path / "artifacts",
        bin_root=tmp_path / "bin",
        output_root=tmp_path / "dist",
        output_base="app",
        out_dir_path=None,
        project_root=tmp_path,
    )

    assert layout.output_artifact == tmp_path / "dist" / "output.wasm"
    assert layout.linked_output_path == tmp_path / "dist" / "output_linked.wasm"


def test_resolve_build_output_layout_defaults_object_output_to_dist(
    tmp_path: Path,
) -> None:
    layout = cli._resolve_build_output_layout(
        target="native",
        trusted=False,
        require_linked=False,
        linked=False,
        linked_output=None,
        emit="obj",
        output=None,
        emit_ir=None,
        artifacts_root=tmp_path / "artifacts",
        bin_root=tmp_path / "bin",
        output_root=tmp_path / "dist",
        output_base="app",
        out_dir_path=None,
        project_root=tmp_path,
    )

    assert layout.output_artifact == tmp_path / "dist" / "output.o"


def test_resolve_build_output_layout_allows_linked_output_for_default_wasm_linking(
    tmp_path: Path,
) -> None:
    layout = cli._resolve_build_output_layout(
        target="wasm-freestanding",
        trusted=False,
        require_linked=False,
        linked=False,
        linked_output=str(tmp_path / "linked.wasm"),
        emit=None,
        output=None,
        emit_ir=None,
        artifacts_root=tmp_path / "artifacts",
        bin_root=tmp_path / "bin",
        output_root=tmp_path / "out",
        output_base="app",
        out_dir_path=None,
        project_root=tmp_path,
    )
    assert layout.is_wasm is True
    assert layout.is_wasm_freestanding is True
    assert layout.linked is True
    assert layout.linked_output_path == tmp_path / "linked.wasm"


def test_default_molt_home_uses_cache_root_when_unset(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    cache_root = tmp_path / "cache_root"
    monkeypatch.delenv("MOLT_HOME", raising=False)
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    assert cli._default_molt_home() == cache_root / "home"


def test_default_molt_home_prefers_explicit_override(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    cache_root = tmp_path / "cache_root"
    explicit_home = tmp_path / "custom_home"
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.setenv("MOLT_HOME", str(explicit_home))
    assert cli._default_molt_home() == explicit_home


def test_cli_doctor_json() -> None:
    res = _run_cli(["doctor", "--json"])
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["schema_version"]
    assert payload["status"] in {"ok", "error"}
    checks = payload["data"].get("checks")
    assert isinstance(checks, list)
    names = {entry.get("name") for entry in checks if isinstance(entry, dict)}
    assert "sccache" in names
    assert "backend-daemon" in names
    assert "cargo-target-dir" in names
    assert "molt-cache-dir" in names
    assert "cargo-upgrade" in names
    assert "llvm-backend-toolchain" in names


def test_cli_update_check_json() -> None:
    res = _run_cli(["update", "--check", "--all", "--json"])
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["command"] == "update"
    assert payload["status"] == "ok"
    data = payload["data"]
    assert data["check_only"] is True
    steps = data.get("steps")
    assert isinstance(steps, list)
    names = {entry.get("name") for entry in steps if isinstance(entry, dict)}
    assert "rustup-update-stable" in names
    assert "rustup-target-add-wasm32-unknown-unknown" in names
    assert "rustup-target-add-wasm32-wasip1" in names
    assert "cargo-update-root" in names
    assert "cargo-update-runtime" in names
    assert "cargo-update-fuzz" in names
    assert "uv-lock-upgrade" in names
    assert "cargo-upgrade-root" in names
    assert "cargo-upgrade-runtime" in names
    assert "cargo-upgrade-fuzz" in names


def test_required_llvm_backend_major_matches_manifest() -> None:
    assert cli._required_llvm_backend_major(ROOT) == 21


def test_planned_update_steps_bootstrap_cargo_edit_when_missing(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    real_which = shutil.which

    def fake_which(name: str) -> str | None:
        if name == "cargo-upgrade":
            return None
        return real_which(name)

    monkeypatch.setattr(cli.shutil, "which", fake_which, raising=True)
    steps, warnings = cli._planned_update_steps(
        ROOT,
        include_toolchains=False,
        include_locks=False,
        include_manifests=True,
    )
    assert warnings == []
    names = [step.name for step in steps]
    assert names[0] == "cargo-edit-bootstrap"
    assert "cargo-upgrade-root" in names
    assert "cargo-upgrade-runtime" in names
    assert "cargo-upgrade-fuzz" in names


def test_cli_run_json(tmp_path: Path) -> None:
    script = tmp_path / "hello.py"
    script.write_text("print('ok')\n")
    res = _run_cli(["run", "--json", str(script)])
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["data"]["returncode"] == 0
    assert "ok" in payload["data"].get("stdout", "")


@pytest.mark.parametrize("profile", ["dev", "release"])
def test_cli_build_json_binary_executes_for_native_profiles(
    tmp_path: Path, profile: str
) -> None:
    script = tmp_path / "hello.py"
    script.write_text("print('ok')\n")

    build = _run_cli(
        [
            "build",
            "--json",
            "--build-profile",
            profile,
            str(script),
            "--out-dir",
            str(tmp_path),
        ]
    )
    assert build.returncode == 0, build.stderr
    payload = json.loads(build.stdout)
    output = Path(payload["data"]["output"])
    assert output.exists()

    run = subprocess.run(
        [str(output)],
        cwd=ROOT,
        env=_base_env(),
        capture_output=True,
        text=True,
        timeout=_cli_timeout(),
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "ok"


def test_cli_compare_json(tmp_path: Path) -> None:
    script = tmp_path / "hello.py"
    script.write_text("print('ok')\n")

    res = _run_cli(["compare", "--json", str(script)])
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["command"] == "compare"
    assert payload["data"]["returncodes"] == {"cpython": 0, "molt": 0, "build": 0}
    assert payload["data"]["match"] == {
        "stdout": True,
        "stderr": True,
        "exitcode": True,
    }
    assert payload["data"]["molt_stdout"] == "ok\n"


def test_cli_parity_run_json(tmp_path: Path) -> None:
    script = tmp_path / "hello.py"
    script.write_text("print('ok')\n")
    res = _run_cli(["parity-run", "--json", str(script)])
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["command"] == "parity-run"
    assert payload["data"]["returncode"] == 0
    assert "ok" in payload["data"].get("stdout", "")


def test_cli_parity_run_timing_json(tmp_path: Path) -> None:
    script = tmp_path / "hello.py"
    script.write_text("print('ok')\n")
    res = _run_cli(["parity-run", "--timing", "--json", str(script)])
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["command"] == "parity-run"
    assert payload["data"]["returncode"] == 0
    assert payload["data"]["timing"]["cpython_run_s"] >= 0


@pytest.mark.parametrize(
    ("symbol", "invocation"),
    [("exec", "exec('value = 1')"), ("eval", "eval('1 + 1')")],
)
def test_cli_run_exec_eval_raise_runtime_error(
    tmp_path: Path, symbol: str, invocation: str
) -> None:
    script = tmp_path / f"{symbol}_direct.py"
    script.write_text(
        "\n".join(
            [
                invocation,
                "",
            ]
        ),
        encoding="utf-8",
    )
    res = _run_cli(["run", str(script)])
    assert res.returncode != 0
    assert "RuntimeError: MOLT_COMPAT_ERROR:" in res.stderr
    assert f"{symbol}() is unsupported in compiled Molt binaries" in res.stderr
    assert "NameError" not in res.stderr
    assert "NameError" not in res.stdout


def test_cli_vendor_dry_run_json() -> None:
    res = _run_cli(
        ["vendor", "--dry-run", "--allow-non-tier-a", "--deterministic-warn", "--json"]
    )
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["command"] == "vendor"
    assert "vendor" in payload["data"]


def test_cli_check_deterministic_warn_json(tmp_path: Path) -> None:
    script = tmp_path / "hello.py"
    script.write_text("print('ok')\n")
    res = _run_cli(
        [
            "check",
            "--deterministic",
            "--deterministic-warn",
            "--json",
            str(script),
        ]
    )
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["command"] == "check"
    assert payload["data"]["deterministic"] is True


def test_cli_vendor_deterministic_warn_json() -> None:
    res = _run_cli(
        [
            "vendor",
            "--dry-run",
            "--allow-non-tier-a",
            "--deterministic",
            "--deterministic-warn",
            "--json",
        ]
    )
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["command"] == "vendor"
    assert payload["data"]["deterministic"] is True


def test_cli_package_verify_roundtrip(tmp_path: Path) -> None:
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"molt")
    manifest = {
        "name": "molt_test_pkg",
        "version": "0.0.1",
        "abi_version": "0.1",
        "target": "test",
        "capabilities": ["net"],
        "deterministic": True,
        "effects": ["nondet"],
        "exports": ["entry"],
    }
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(json.dumps(manifest))
    capabilities_path = tmp_path / "caps.json"
    capabilities_path.write_text(
        json.dumps(
            {
                "allow": ["net"],
                "deny": ["fs.write"],
                "effects": ["nondet"],
                "fs": {"read": ["/tmp/data"], "write": []},
                "packages": {
                    "molt_test_pkg": {"allow": ["net"], "effects": ["nondet"]}
                },
            }
        )
    )
    package_path = tmp_path / "pkg.moltpkg"

    res = _run_cli(
        [
            "package",
            str(artifact),
            str(manifest_path),
            "--output",
            str(package_path),
            "--capabilities",
            str(capabilities_path),
            "--json",
        ]
    )
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    sbom_path = Path(payload["data"]["sbom"])
    signature_meta_path = Path(payload["data"]["signature_metadata"])
    assert sbom_path.exists()
    assert signature_meta_path.exists()
    signature_meta = json.loads(signature_meta_path.read_text())
    assert signature_meta["status"] == "unsigned"
    assert package_path.exists()

    res = _run_cli(
        [
            "verify",
            "--package",
            str(package_path),
            "--require-checksum",
            "--require-deterministic",
            "--capabilities",
            str(capabilities_path),
            "--json",
        ]
    )
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["status"] == "ok"


def test_cli_verify_requires_capabilities_allowlist(tmp_path: Path) -> None:
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"molt")
    manifest = {
        "name": "molt_caps_pkg",
        "version": "0.0.1",
        "abi_version": "0.1",
        "target": "test",
        "capabilities": ["net"],
        "deterministic": True,
        "effects": ["nondet"],
        "exports": ["entry"],
    }
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(json.dumps(manifest))
    package_path = tmp_path / "pkg.moltpkg"

    res = _run_cli(
        [
            "package",
            str(artifact),
            str(manifest_path),
            "--output",
            str(package_path),
            "--json",
        ]
    )
    assert res.returncode == 0

    res = _run_cli(
        [
            "verify",
            "--package",
            str(package_path),
            "--json",
        ]
    )
    assert res.returncode != 0
    payload = json.loads(res.stdout)
    assert payload["status"] == "error"
    assert "capabilities allowlist required" in payload["errors"][0]


def test_cli_package_emits_sbom_and_signature(tmp_path: Path) -> None:
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"molt")
    manifest = {
        "name": "molt_test_pkg",
        "version": "0.0.1",
        "abi_version": "0.1",
        "target": "test",
        "capabilities": [],
        "deterministic": True,
        "effects": [],
        "exports": ["entry"],
    }
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(json.dumps(manifest))
    package_path = tmp_path / "pkg.moltpkg"

    res = _run_cli(
        [
            "package",
            str(artifact),
            str(manifest_path),
            "--output",
            str(package_path),
            "--json",
        ]
    )
    assert res.returncode == 0
    sbom_path = package_path.with_name(package_path.stem + ".sbom.json")
    sig_meta_path = package_path.with_name(package_path.stem + ".sig.json")
    assert sbom_path.exists()
    assert sig_meta_path.exists()
    with zipfile.ZipFile(package_path) as zf:
        assert "sbom.json" in zf.namelist()
        assert "signature.json" in zf.namelist()


def test_cli_package_spdx_sbom(tmp_path: Path) -> None:
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"molt")
    manifest = {
        "name": "molt_test_pkg",
        "version": "0.0.1",
        "abi_version": "0.1",
        "target": "test",
        "capabilities": [],
        "deterministic": True,
        "effects": [],
        "exports": ["entry"],
    }
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(json.dumps(manifest))
    package_path = tmp_path / "pkg.moltpkg"

    res = _run_cli(
        [
            "package",
            str(artifact),
            str(manifest_path),
            "--output",
            str(package_path),
            "--sbom-format",
            "spdx",
            "--json",
        ]
    )
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    sbom_path = Path(payload["data"]["sbom"])
    sbom = json.loads(sbom_path.read_text())
    assert sbom["spdxVersion"] == "SPDX-2.3"
    assert sbom["dataLicense"] == "CC0-1.0"
    assert sbom["packages"]


def test_cli_publish_remote_with_auth(tmp_path: Path) -> None:
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"molt")
    manifest = {
        "name": "molt_test_pkg",
        "version": "0.0.1",
        "abi_version": "0.1",
        "target": "test",
        "capabilities": [],
        "deterministic": True,
        "effects": [],
        "exports": ["entry"],
    }
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(json.dumps(manifest))
    package_path = tmp_path / "pkg.moltpkg"
    res = _run_cli(
        [
            "package",
            str(artifact),
            str(manifest_path),
            "--output",
            str(package_path),
            "--json",
        ]
    )
    assert res.returncode == 0
    key_sha = "0" * 64
    _inject_signature_metadata(package_path, key_sha)
    trust_policy = _write_trust_policy(tmp_path, key_sha)

    received: list[dict[str, str]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_PUT(self) -> None:  # noqa: N802 - http.server naming convention
            length = int(self.headers.get("Content-Length", "0") or "0")
            if length:
                _ = self.rfile.read(length)
            received.append(
                {
                    "path": self.path,
                    "auth": self.headers.get("Authorization", ""),
                    "ctype": self.headers.get("Content-Type", ""),
                }
            )
            self.send_response(201)
            self.end_headers()

        def log_message(self, format: str, *args: object) -> None:
            return

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        registry_url = f"http://127.0.0.1:{server.server_port}/registry/"
        res = _run_cli(
            [
                "publish",
                str(package_path),
                "--registry",
                registry_url,
                "--registry-token",
                "test-token",
                "--require-signature",
                "--no-verify-signature",
                "--trusted-signers",
                str(trust_policy),
                "--json",
            ]
        )
        assert res.returncode == 0
        payload = json.loads(res.stdout)
        assert payload["data"]["remote"] is True
        assert payload["data"]["auth"]["mode"] == "bearer"
        expected_paths = {
            "/registry/pkg.moltpkg",
            "/registry/pkg.sbom.json",
            "/registry/pkg.sig.json",
        }
        for _ in range(50):
            if len(received) >= len(expected_paths):
                break
            time.sleep(0.02)
        assert {entry["path"] for entry in received} == expected_paths
        for entry in received:
            assert entry["auth"] == "Bearer test-token"
        content_types = {entry["path"]: entry["ctype"] for entry in received}
        assert content_types["/registry/pkg.moltpkg"] == "application/zip"
        assert content_types["/registry/pkg.sbom.json"] == "application/json"
        assert content_types["/registry/pkg.sig.json"] == "application/json"
    finally:
        server.shutdown()


def test_cli_publish_remote_basic_auth(tmp_path: Path) -> None:
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"molt")
    manifest = {
        "name": "molt_test_pkg",
        "version": "0.0.1",
        "abi_version": "0.1",
        "target": "test",
        "capabilities": [],
        "deterministic": True,
        "effects": [],
        "exports": ["entry"],
    }
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(json.dumps(manifest))
    package_path = tmp_path / "pkg.moltpkg"
    res = _run_cli(
        [
            "package",
            str(artifact),
            str(manifest_path),
            "--output",
            str(package_path),
            "--json",
        ]
    )
    assert res.returncode == 0
    key_sha = "1" * 64
    _inject_signature_metadata(package_path, key_sha)
    trust_policy = _write_trust_policy(tmp_path, key_sha)

    received: list[str] = []

    class Handler(BaseHTTPRequestHandler):
        def do_PUT(self) -> None:  # noqa: N802 - http.server naming convention
            length = int(self.headers.get("Content-Length", "0") or "0")
            if length:
                _ = self.rfile.read(length)
            received.append(self.headers.get("Authorization", ""))
            self.send_response(200)
            self.end_headers()

        def log_message(self, format: str, *args: object) -> None:
            return

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        registry_url = f"http://127.0.0.1:{server.server_port}/registry/"
        res = _run_cli(
            [
                "publish",
                str(package_path),
                "--registry",
                registry_url,
                "--registry-user",
                "molt",
                "--registry-password",
                "secret",
                "--require-signature",
                "--no-verify-signature",
                "--trusted-signers",
                str(trust_policy),
                "--json",
            ]
        )
        assert res.returncode == 0
        payload = json.loads(res.stdout)
        assert payload["data"]["remote"] is True
        assert payload["data"]["auth"]["mode"] == "basic"
        expected = base64.b64encode(b"molt:secret").decode("ascii")
        for _ in range(50):
            if len(received) >= 1:
                break
            time.sleep(0.02)
        assert all(header == f"Basic {expected}" for header in received)
    finally:
        server.shutdown()


def test_cli_publish_remote_missing_password(tmp_path: Path) -> None:
    package_path = tmp_path / "pkg.moltpkg"
    package_path.write_bytes(b"molt")
    res = _run_cli(
        [
            "publish",
            str(package_path),
            "--registry",
            "http://127.0.0.1:1/registry/",
            "--registry-user",
            "molt",
            "--no-deterministic",
            "--no-require-signature",
            "--no-verify-signature",
            "--json",
        ]
    )
    assert res.returncode != 0
    payload = json.loads(res.stdout)
    assert payload["status"] == "error"
    assert any("Registry password is required" in msg for msg in payload["errors"])


def test_cli_publish_remote_requires_trust_policy(tmp_path: Path) -> None:
    package_path = tmp_path / "pkg.moltpkg"
    package_path.write_bytes(b"molt")
    res = _run_cli(
        [
            "publish",
            str(package_path),
            "--registry",
            "http://127.0.0.1:1/registry/",
            "--no-deterministic",
            "--json",
        ]
    )
    assert res.returncode != 0
    payload = json.loads(res.stdout)
    assert payload["status"] == "error"
    assert any(
        "Remote publish requires --trusted-signers" in msg for msg in payload["errors"]
    )


def test_cli_package_respects_denies(tmp_path: Path) -> None:
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"molt")
    manifest = {
        "name": "molt_test_pkg",
        "version": "0.0.1",
        "abi_version": "0.1",
        "target": "test",
        "capabilities": ["fs.write"],
        "deterministic": True,
        "effects": [],
        "exports": ["entry"],
    }
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(json.dumps(manifest))
    capabilities_path = tmp_path / "caps.json"
    capabilities_path.write_text(json.dumps({"allow": ["fs"], "deny": ["fs.write"]}))
    package_path = tmp_path / "pkg.moltpkg"

    res = _run_cli(
        [
            "package",
            str(artifact),
            str(manifest_path),
            "--output",
            str(package_path),
            "--capabilities",
            str(capabilities_path),
            "--json",
        ]
    )
    assert res.returncode != 0
    payload = json.loads(res.stdout)
    assert payload["status"] == "error"
    assert "Capabilities missing from allowlist" in payload["errors"][0]


def test_cli_package_rejects_abi_mismatch(tmp_path: Path) -> None:
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"molt")
    manifest = {
        "name": "molt_test_pkg",
        "version": "0.0.1",
        "abi_version": "0.2",
        "target": "test",
        "capabilities": [],
        "deterministic": True,
        "effects": [],
        "exports": ["entry"],
    }
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(json.dumps(manifest))
    package_path = tmp_path / "pkg.moltpkg"

    res = _run_cli(
        [
            "package",
            str(artifact),
            str(manifest_path),
            "--output",
            str(package_path),
            "--json",
        ]
    )
    assert res.returncode != 0
    payload = json.loads(res.stdout)
    assert payload["status"] == "error"
    assert "unsupported abi_version" in payload["errors"][0]


def test_cli_verify_requires_signature(tmp_path: Path) -> None:
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"molt")
    manifest = {
        "name": "molt_test_pkg",
        "version": "0.0.1",
        "abi_version": "0.1",
        "target": "test",
        "capabilities": [],
        "deterministic": True,
        "effects": [],
        "exports": ["entry"],
    }
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(json.dumps(manifest))
    package_path = tmp_path / "pkg.moltpkg"

    res = _run_cli(
        [
            "package",
            str(artifact),
            str(manifest_path),
            "--output",
            str(package_path),
            "--json",
        ]
    )
    assert res.returncode == 0

    res = _run_cli(
        [
            "verify",
            "--package",
            str(package_path),
            "--require-signature",
            "--json",
        ]
    )
    assert res.returncode != 0
    payload = json.loads(res.stdout)
    assert payload["status"] == "error"
    assert "signature required" in payload["errors"][0]


def test_cli_verify_accepts_signature_file(tmp_path: Path) -> None:
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"molt")
    manifest = {
        "name": "molt_test_pkg",
        "version": "0.0.1",
        "abi_version": "0.1",
        "target": "test",
        "capabilities": [],
        "deterministic": True,
        "effects": [],
        "exports": ["entry"],
    }
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(json.dumps(manifest))
    signature = tmp_path / "artifact.sig"
    signature.write_text("signed")
    package_path = tmp_path / "pkg.moltpkg"

    res = _run_cli(
        [
            "package",
            str(artifact),
            str(manifest_path),
            "--signature",
            str(signature),
            "--output",
            str(package_path),
            "--json",
        ]
    )
    assert res.returncode == 0
    key_sha = "2" * 64
    _inject_signature_metadata(package_path, key_sha)
    _write_trust_policy(tmp_path, key_sha)

    res = _run_cli(
        [
            "verify",
            "--package",
            str(package_path),
            "--require-signature",
            "--json",
        ]
    )
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["status"] == "ok"


def test_cli_publish_remote_registry(tmp_path: Path) -> None:
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"molt")
    manifest = {
        "name": "molt_test_pkg",
        "version": "0.0.1",
        "abi_version": "0.1",
        "target": "test",
        "capabilities": [],
        "deterministic": True,
        "effects": [],
        "exports": ["entry"],
    }
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(json.dumps(manifest))
    signature = tmp_path / "artifact.sig"
    signature.write_text("signed")
    package_path = tmp_path / "pkg.moltpkg"

    res = _run_cli(
        [
            "package",
            str(artifact),
            str(manifest_path),
            "--signature",
            str(signature),
            "--output",
            str(package_path),
            "--json",
        ]
    )
    assert res.returncode == 0
    key_sha = "2" * 64
    _inject_signature_metadata(package_path, key_sha)
    trust_policy = _write_trust_policy(tmp_path, key_sha)

    server, uploads = _start_registry_server()
    try:
        port = server.server_address[1]
        registry_url = f"http://127.0.0.1:{port}/registry/"
        res = _run_cli(
            [
                "publish",
                str(package_path),
                "--registry",
                registry_url,
                "--registry-token",
                "tok",
                "--require-signature",
                "--no-verify-signature",
                "--trusted-signers",
                str(trust_policy),
                "--json",
            ]
        )
        assert res.returncode == 0
        payload = json.loads(res.stdout)
        assert payload["data"]["remote"] is True
        assert payload["data"]["auth"]["mode"] == "bearer"
    finally:
        server.shutdown()

    assert uploads
    expected_suffixes = {".moltpkg", ".sbom.json", ".sig.json", ".sig"}
    seen_suffixes: set[str] = set()
    for entry in uploads:
        headers = entry["headers"]
        assert headers.get("Authorization") == "Bearer tok"
        path = entry["path"]
        suffix = "".join(Path(str(path)).suffixes[-2:]) or Path(str(path)).suffix
        if suffix in expected_suffixes:
            seen_suffixes.add(suffix)
    assert expected_suffixes.issubset(seen_suffixes)


def test_cli_build_cross_target_with_zig(tmp_path: Path) -> None:
    target_triple = _cross_target_triple()
    if target_triple is None:
        pytest.skip("Cross-target triples are only defined for Darwin/Linux here.")
    if shutil.which("zig") is None:
        pytest.skip("zig is required for cross-target linking.")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for backend compilation.")

    script = tmp_path / "hello.py"
    script.write_text("print('ok')\n")
    output = tmp_path / "hello_molt"

    try:
        res = _run_cli_with_timeout(
            [
                "build",
                "--target",
                target_triple,
                "--profile",
                "dev",
                "--out-dir",
                str(tmp_path),
                "--output",
                str(output),
                "--json",
                str(script),
            ],
            timeout=300,
        )
    except subprocess.TimeoutExpired:
        pytest.skip(
            "Cross-target build exceeded 300s; warm cargo cache or prebuild runtime."
        )
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["status"] == "ok"
    assert payload["data"]["target_triple"] == target_triple
    assert Path(payload["data"]["output"]).exists()


def test_cli_build_sysroot_json(tmp_path: Path) -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for backend compilation.")

    script = tmp_path / "hello.py"
    script.write_text("print('ok')\n")
    sysroot = tmp_path / "sysroot"
    sysroot.mkdir()
    profile_path = _write_pgo_profile(tmp_path, entrypoint=str(script))

    res = _run_cli(
        [
            "build",
            "--emit",
            "obj",
            "--out-dir",
            str(tmp_path),
            "--sysroot",
            str(sysroot),
            "--pgo-profile",
            str(profile_path),
            "--json",
            str(script),
        ]
    )
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["status"] == "ok"
    assert payload["data"]["sysroot"] == str(sysroot)
    assert payload["data"]["pgo_profile"]["path"] == str(profile_path)
    assert payload["data"]["pgo_profile"]["version"] == "0.1"
    assert "molt_init___main__" in payload["data"]["pgo_profile"]["hot_functions"]
    assert Path(payload["data"]["output"]).exists()


def test_cli_build_runtime_feedback_json(tmp_path: Path) -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for backend compilation.")

    script = tmp_path / "hello.py"
    script.write_text("def helper():\n    return 1\n\nprint(helper())\n")
    sysroot = tmp_path / "sysroot"
    sysroot.mkdir()
    feedback_path = _write_runtime_feedback(tmp_path)

    res = _run_cli(
        [
            "build",
            "--emit",
            "obj",
            "--out-dir",
            str(tmp_path),
            "--sysroot",
            str(sysroot),
            "--runtime-feedback",
            str(feedback_path),
            "--json",
            str(script),
        ]
    )
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["status"] == "ok"
    assert payload["data"]["sysroot"] == str(sysroot)
    assert payload["data"]["runtime_feedback"]["path"] == str(feedback_path)
    assert payload["data"]["runtime_feedback"]["schema_version"] == 1
    assert "molt_init___main__" in payload["data"]["runtime_feedback"]["hot_functions"]
    assert Path(payload["data"]["output"]).exists()


def test_cli_build_diagnostics_summary_verbosity_trims_stderr(tmp_path: Path) -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for backend compilation.")

    script = tmp_path / "hello.py"
    script.write_text("def helper():\n    return 1\n\nprint(helper())\n")
    sysroot = tmp_path / "sysroot"
    sysroot.mkdir()

    res = _run_cli(
        [
            "build",
            "--emit",
            "obj",
            "--out-dir",
            str(tmp_path),
            "--sysroot",
            str(sysroot),
            "--diagnostics",
            "--diagnostics-verbosity",
            "summary",
            str(script),
        ]
    )
    assert res.returncode == 0
    assert "Build diagnostics:" in res.stderr
    assert "frontend_parallel:" in res.stderr
    assert "frontend_parallel.layer.1:" not in res.stderr
    assert "midend.hotspot.1:" not in res.stderr


def test_cli_build_json_diagnostics_include_midend_policy_config(tmp_path: Path) -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for backend compilation.")

    script = tmp_path / "hello.py"
    script.write_text("def helper(x=1):\n    return x\n\nprint(helper())\n")
    sysroot = tmp_path / "sysroot"
    sysroot.mkdir()

    env = _base_env()
    env["MOLT_BUILD_DIAGNOSTICS"] = "1"
    env["MOLT_MIDEND_BUDGET_SCALE"] = "1.5"
    res = subprocess.run(
        [
            _python_executable(),
            "-m",
            "molt.cli",
            "build",
            "--emit",
            "obj",
            "--out-dir",
            str(tmp_path),
            "--sysroot",
            str(sysroot),
            "--json",
            str(script),
        ],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=_cli_timeout(),
    )
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    diagnostics = payload["data"]["compile_diagnostics"]["midend"]
    policy = diagnostics["policy_config"]
    assert policy["hot_tier_promotion_enabled"] is True
    assert policy["budget_alpha"] == 0.03
    assert policy["budget_beta"] == 0.75
    assert policy["budget_scale"] == 1.5


def test_cli_completion_bash_json() -> None:
    res = _run_cli(["completion", "--shell", "bash", "--json"])
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["command"] == "completion"
    assert payload["data"]["shell"] == "bash"
    assert "complete -F _molt_complete" in payload["data"]["script"]


def test_cli_config_json() -> None:
    res = _run_cli(["config", "--json"])
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    assert payload["command"] == "config"
    assert payload["status"] == "ok"
    assert "root" in payload["data"]
    assert "sources" in payload["data"]


def test_cli_completion_includes_build_flags() -> None:
    res = _run_cli(["completion", "--shell", "bash", "--json"])
    assert res.returncode == 0
    payload = json.loads(res.stdout)
    script = payload["data"]["script"]
    assert "extension" in script
    assert "build audit" in script
    assert "parity-run" in script
    assert "--emit" in script
    assert "--rebuild" in script
    assert "--trusted" in script
    assert "--no-trusted" in script
    assert "--molt-abi" in script
    assert "--target" in script
    assert "--require-abi" in script
    assert "--require-checksum" in script
    assert "--extension-metadata" in script
    assert "--require-extension-capabilities" in script


def test_cli_extension_requires_subcommand() -> None:
    res = _run_cli(["extension"])
    assert res.returncode != 0
    assert "extension_command" in res.stderr
