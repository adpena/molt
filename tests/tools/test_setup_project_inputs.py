from __future__ import annotations

from pathlib import Path
import os

from tests.process_guard_common import run_guarded_test_process


ROOT = Path(__file__).resolve().parents[2]
NORMALIZER = ROOT / ".github" / "actions" / "setup-project" / "normalize-inputs.sh"
BASH = (
    Path(os.environ.get("ProgramFiles", "C:/Program Files"))
    / "Git"
    / "bin"
    / "bash.exe"
    if os.name == "nt"
    else Path("bash")
)


def _normalize(
    tmp_path: Path,
    *,
    toolchain: str,
    components: str = "",
    targets: str = "",
    namespace: str = "project",
    sync: str = "false",
    sync_frozen: str = "false",
    sync_dev: str = "false",
    sync_groups: str = "",
) -> dict[str, str]:
    tmp_path.mkdir(parents=True, exist_ok=True)
    output = tmp_path / "github-output"
    env = {
        **os.environ,
        "INPUT_PYTHON": "true",
        "INPUT_UV": "true",
        "INPUT_CACHE_UV": "true",
        "INPUT_CACHE_CARGO": "false",
        "INPUT_CACHE_LEAN": "false",
        "INPUT_CACHE_NAMESPACE": namespace,
        "INPUT_ACTIONLINT": "false",
        "INPUT_RUST_TOOLCHAIN": toolchain,
        "INPUT_RUST_COMPONENTS": components,
        "INPUT_RUST_TARGETS": targets,
        "INPUT_SYNC": sync,
        "INPUT_SYNC_FROZEN": sync_frozen,
        "INPUT_SYNC_DEV": sync_dev,
        "INPUT_SYNC_GROUPS": sync_groups,
    }
    completed = run_guarded_test_process(
        [str(BASH), str(NORMALIZER), str(output)],
        prefix="MOLT_SETUP_PROJECT_INPUT_TEST",
        cwd=ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
    return dict(
        line.split("=", 1) for line in output.read_text(encoding="utf-8").splitlines()
    )


def test_stable_lists_are_sorted_deduplicated_and_cache_safe(tmp_path: Path) -> None:
    normalized = _normalize(
        tmp_path,
        toolchain="1.96.1",
        components="rustfmt, clippy, rustfmt",
        targets="wasm32-wasip1, aarch64-unknown-linux-gnu",
    )

    assert normalized["rust-toolchain"] == "1.96.1"
    assert normalized["rust-components"] == "clippy,rustfmt"
    assert normalized["rust-targets"] == "aarch64-unknown-linux-gnu,wasm32-wasip1"
    assert len(normalized["rust-cache-token"]) == 40
    assert "," not in normalized["rust-cache-token"]


def test_nightly_components_select_nightly_identity(tmp_path: Path) -> None:
    nightly = (
        (ROOT / "config" / "rust_nightly_toolchain.txt")
        .read_text(encoding="utf-8")
        .strip()
    )
    normalized = _normalize(
        tmp_path,
        toolchain="sanitizer-nightly",
        components="miri, rust-src",
        namespace="sanitizers-miri",
    )

    assert normalized["rust-toolchain"] == nightly
    assert normalized["rust-components"] == "miri,rust-src"
    assert normalized["cache-namespace"] == "sanitizers-miri"


def test_list_order_and_whitespace_do_not_change_cache_identity(tmp_path: Path) -> None:
    first = _normalize(
        tmp_path / "first",
        toolchain="1.96.1",
        components="rustfmt, clippy",
        targets="wasm32-wasip1,aarch64-unknown-linux-gnu",
    )
    second = _normalize(
        tmp_path / "second",
        toolchain="1.96.1",
        components=" clippy ,rustfmt ",
        targets="aarch64-unknown-linux-gnu, wasm32-wasip1",
    )

    assert first["rust-cache-token"] == second["rust-cache-token"]


def test_control_characters_and_empty_atoms_fail_closed(tmp_path: Path) -> None:
    for components in ("rustfmt,,clippy", "rustfmt\nclippy", "rustfmt\tclippy"):
        output = tmp_path / components.encode().hex()
        env = {
            **os.environ,
            "INPUT_PYTHON": "true",
            "INPUT_UV": "true",
            "INPUT_CACHE_UV": "true",
            "INPUT_CACHE_CARGO": "false",
            "INPUT_CACHE_LEAN": "false",
            "INPUT_CACHE_NAMESPACE": "project",
            "INPUT_ACTIONLINT": "false",
            "INPUT_RUST_TOOLCHAIN": "1.96.1",
            "INPUT_RUST_COMPONENTS": components,
            "INPUT_RUST_TARGETS": "wasm32-wasip1",
            "INPUT_SYNC": "false",
            "INPUT_SYNC_FROZEN": "false",
            "INPUT_SYNC_DEV": "false",
            "INPUT_SYNC_GROUPS": "",
        }
        completed = run_guarded_test_process(
            [str(BASH), str(NORMALIZER), str(output)],
            prefix="MOLT_SETUP_PROJECT_INPUT_TEST",
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
        assert completed.returncode == 2
        assert not output.exists()


def test_sync_argv_is_typed_normalized_and_requires_sync(tmp_path: Path) -> None:
    normalized = _normalize(
        tmp_path / "valid",
        toolchain="",
        sync="true",
        sync_frozen="true",
        sync_groups=" dev,bench,dev ",
    )
    assert normalized["sync"] == "true"
    assert normalized["sync-frozen"] == "true"
    assert normalized["sync-groups"] == "bench,dev"

    output = tmp_path / "invalid" / "github-output"
    output.parent.mkdir()
    env = {
        **os.environ,
        "INPUT_PYTHON": "true",
        "INPUT_UV": "true",
        "INPUT_CACHE_UV": "true",
        "INPUT_CACHE_CARGO": "false",
        "INPUT_CACHE_LEAN": "false",
        "INPUT_CACHE_NAMESPACE": "project",
        "INPUT_ACTIONLINT": "false",
        "INPUT_RUST_TOOLCHAIN": "",
        "INPUT_RUST_COMPONENTS": "",
        "INPUT_RUST_TARGETS": "",
        "INPUT_SYNC": "false",
        "INPUT_SYNC_FROZEN": "true",
        "INPUT_SYNC_DEV": "false",
        "INPUT_SYNC_GROUPS": "",
    }
    completed = run_guarded_test_process(
        [str(BASH), str(NORMALIZER), str(output)],
        prefix="MOLT_SETUP_PROJECT_INPUT_TEST",
        cwd=ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert completed.returncode == 2
    assert not output.exists()


def test_shell_metacharacters_never_execute_before_validation(tmp_path: Path) -> None:
    marker = tmp_path / "executed"
    malicious = f'project"; touch "{marker}"; #'
    output = tmp_path / "github-output"
    env = {
        **os.environ,
        "INPUT_PYTHON": "true",
        "INPUT_UV": "true",
        "INPUT_CACHE_UV": "true",
        "INPUT_CACHE_CARGO": "false",
        "INPUT_CACHE_LEAN": "false",
        "INPUT_CACHE_NAMESPACE": malicious,
        "INPUT_ACTIONLINT": "false",
        "INPUT_RUST_TOOLCHAIN": "",
        "INPUT_RUST_COMPONENTS": "",
        "INPUT_RUST_TARGETS": "",
        "INPUT_SYNC": "false",
        "INPUT_SYNC_FROZEN": "false",
        "INPUT_SYNC_DEV": "false",
        "INPUT_SYNC_GROUPS": "",
    }
    completed = run_guarded_test_process(
        [str(BASH), str(NORMALIZER), str(output)],
        prefix="MOLT_SETUP_PROJECT_INPUT_TEST",
        cwd=ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert completed.returncode == 2
    assert not marker.exists()
    assert not output.exists()
