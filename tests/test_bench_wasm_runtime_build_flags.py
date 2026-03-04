from __future__ import annotations

from pathlib import Path

import tools.bench_wasm as bench_wasm


def _fake_runtime_build(cmd: list[str], env: dict[str, str]) -> None:
    profile = cmd[3]
    profile_dir = bench_wasm._cargo_profile_dir(profile)
    target_root = Path(env["CARGO_TARGET_DIR"])
    src = target_root / "wasm32-wasip1" / profile_dir / "molt_runtime.wasm"
    src.parent.mkdir(parents=True, exist_ok=True)
    src.write_bytes(b"\x00asm\x01\x00\x00\x00ok")


def test_build_runtime_wasm_uses_wasm_release_profile_and_aggressive_features(
    monkeypatch,
    tmp_path: Path,
) -> None:
    target_root = tmp_path / "target"
    monkeypatch.setattr(bench_wasm, "_cargo_target_root", lambda: target_root)
    monkeypatch.setattr(bench_wasm, "_repo_root", lambda: tmp_path)
    monkeypatch.delenv("MOLT_WASM_RUNTIME_TARGET_FEATURES", raising=False)
    monkeypatch.delenv("MOLT_WASM_RUNTIME_TARGET_FEATURE_MODE", raising=False)
    monkeypatch.delenv("MOLT_WASM_RUNTIME_TARGET_FEATURES_EXTRA", raising=False)
    monkeypatch.delenv("MOLT_WASM_RUNTIME_TARGET_CPU", raising=False)
    monkeypatch.delenv("MOLT_WASM_LEGACY_LINK_FLAGS", raising=False)

    captured: list[tuple[list[str], dict[str, str]]] = []

    def _fake_run_cmd(  # type: ignore[no-untyped-def]
        cmd: list[str],
        *,
        env: dict[str, str],
        capture: bool,
        tty: bool,
        log,
        timeout_s: float | None = None,
    ):
        del capture, tty, log, timeout_s
        captured.append((list(cmd), dict(env)))
        _fake_runtime_build(cmd, env)
        return bench_wasm._RunResult(returncode=0)

    monkeypatch.setattr(bench_wasm, "_run_cmd", _fake_run_cmd)
    output = tmp_path / "runtime.wasm"
    assert bench_wasm.build_runtime_wasm(
        reloc=False,
        output=output,
        cargo_profile="release",
        tty=False,
        log=None,
    )
    assert output.exists()
    assert output.read_bytes().startswith(b"\x00asm")
    cmd, env = captured[0]
    assert cmd[:4] == ["cargo", "build", "--profile", "wasm-release"]
    rustflags = env.get("RUSTFLAGS", "")
    target_features = bench_wasm._rustflags_codegen_values(rustflags, "target-feature")
    assert target_features
    merged = target_features[-1]
    assert "+simd128" in merged
    assert "+bulk-memory" in merged
    assert "+sign-ext" in merged


def test_build_runtime_wasm_honors_baseline_mode_and_legacy_shared_link_flags(
    monkeypatch,
    tmp_path: Path,
) -> None:
    target_root = tmp_path / "target"
    monkeypatch.setattr(bench_wasm, "_cargo_target_root", lambda: target_root)
    monkeypatch.setattr(bench_wasm, "_repo_root", lambda: tmp_path)
    monkeypatch.setenv("MOLT_WASM_RUNTIME_TARGET_FEATURE_MODE", "baseline")
    monkeypatch.setenv("MOLT_WASM_RUNTIME_TARGET_FEATURES_EXTRA", "+sign-ext")
    monkeypatch.setenv("MOLT_WASM_LEGACY_LINK_FLAGS", "1")

    captured: list[tuple[list[str], dict[str, str]]] = []

    def _fake_run_cmd(  # type: ignore[no-untyped-def]
        cmd: list[str],
        *,
        env: dict[str, str],
        capture: bool,
        tty: bool,
        log,
        timeout_s: float | None = None,
    ):
        del capture, tty, log, timeout_s
        captured.append((list(cmd), dict(env)))
        _fake_runtime_build(cmd, env)
        return bench_wasm._RunResult(returncode=0)

    monkeypatch.setattr(bench_wasm, "_run_cmd", _fake_run_cmd)
    output = tmp_path / "runtime_legacy.wasm"
    assert bench_wasm.build_runtime_wasm(
        reloc=False,
        output=output,
        cargo_profile="release",
        tty=False,
        log=None,
    )
    cmd, env = captured[0]
    assert cmd[:4] == ["cargo", "build", "--profile", "wasm-release"]
    rustflags = env.get("RUSTFLAGS", "")
    assert "--import-memory" in rustflags
    target_features = bench_wasm._rustflags_codegen_values(rustflags, "target-feature")
    assert target_features
    merged = target_features[-1]
    assert merged.startswith("+simd128")
    assert "+sign-ext" in merged
