from __future__ import annotations

from pathlib import Path
import os
import shlex
import sys
import tomllib

import pytest

from molt import tool_releases

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


def test_python_cannot_disable_its_managed_provisioner(tmp_path: Path) -> None:
    code, error, outputs = _run_normalizer(tmp_path, toolchain="", uv="false")
    assert code == 2
    assert "python requires uv" in error
    assert outputs == {}


@pytest.mark.parametrize(
    "inputs", [{"toolchain": "pinned"}, {"toolchain": "", "sync": "true"}]
)
def test_repository_consumers_cannot_disable_verified_python(tmp_path, inputs):
    code, error, outputs = _run_normalizer(tmp_path, python="false", **inputs)
    assert code == 2
    assert "requires the verified repository Python" in error
    assert outputs == {}


@pytest.mark.parametrize(
    "inputs",
    [{"components": "--force"}, {"targets": "--force"}, {"namespace": "--force"}],
)
def test_bootstrap_rejects_option_shaped_atoms(tmp_path, inputs):
    code, error, outputs = _run_normalizer(tmp_path, toolchain="pinned", **inputs)
    assert code == 2
    assert "invalid" in error
    assert outputs == {}


def _python_bootstrap(tmp_path: Path, *, failure: str = "", pin: str = "3.12.15\n"):
    """Exercise the real shell/bootstrap validator with finite external tools.

    The fake installer never downloads Python. Its interpreter driver executes
    the actual validator against explicitly authored implementation/path facts;
    real uv archives and Windows aliases remain the CI cold-provision oracle.
    """
    directory = tmp_path / "bootstrap space ü"
    directory.mkdir()
    (directory / ".python-version").write_text(pin, encoding="utf-8")
    custody = directory / "custody"
    install = custody / "python"
    binaries = custody / "python-bin"
    fake_tools = directory / "tools"
    for path in (install, binaries, fake_tools):
        path.mkdir(parents=True)
    selected = install / "selected-python"
    other = install / "other-python"
    other.write_text("unselected interpreter", encoding="utf-8")
    events = directory / "events"
    driver = directory / "interpreter-driver.py"
    driver.write_text(
        "import os, pathlib, platform, sys\n"
        "args = sys.argv[1:]\n"
        "code_at = args.index('-c')\n"
        "code = args[code_at + 1]\n"
        "sys.argv = ['-c', *args[code_at + 2:]]\n"
        "sys.executable = os.environ['FAKE_SELECTED']\n"
        "if os.environ['FAILURE'] == 'alias' and os.environ['ALIAS'] == 'python3':\n"
        "    sys.executable = os.environ['FAKE_OTHER']\n"
        "platform.python_version = lambda: '3.12.14' if os.environ['FAILURE'] == 'version' else '3.12.15'\n"
        "platform.python_implementation = lambda: 'PyPy' if os.environ['FAILURE'] == 'implementation' else 'CPython'\n"
        "with open(os.environ['EVENTS'], 'a') as out: out.write('validate:' + os.environ['ALIAS'] + '\\n')\n"
        "exec(compile(code, '<bootstrap validator>', 'exec'))\n",
        encoding="utf-8",
    )
    interpreter = shlex.quote(Path(sys.executable).as_posix())
    for alias, path in (
        ("selected", selected),
        ("python", binaries / "python"),
        ("python3", binaries / "python3"),
    ):
        path.write_text(
            "#!/usr/bin/env bash\n"
            f"export ALIAS={shlex.quote(alias)}\n"
            f'exec {interpreter} -I -S -B {shlex.quote(driver.as_posix())} "$@"\n',
            encoding="utf-8",
        )
        path.chmod(0o755)
    uv = fake_tools / "uv"
    uv.write_text(
        "#!/usr/bin/env bash\nset -eu\n"
        'printf \'%s\\n\' "$*" >> "$EVENTS"\n'
        'case "$1 $2" in\n'
        ' "python install") [[ "$FAILURE" != install ]] || exit 41 ;;\n'
        ' "python find") [[ "$FAILURE" != find ]] || exit 42; printf "%s\\n" "$FAKE_SELECTED"; [[ "$FAILURE" != multiline ]] || printf "EVIL=value\\n" ;;\n'
        ' "python dir") printf "%s\\n" "$UV_PYTHON_BIN_DIR" ;;\n'
        " *) exit 43 ;;\nesac\n",
        encoding="utf-8",
    )
    uv.chmod(0o755)
    exports = {
        name: directory / name
        for name in ("GITHUB_ENV", "GITHUB_PATH", "GITHUB_OUTPUT")
    }
    for path in exports.values():
        path.write_text("", encoding="utf-8")
    env = {
        **os.environ,
        **{name: path.as_posix() for name, path in exports.items()},
        "PATH": str(fake_tools) + os.pathsep + os.environ["PATH"],
        "MOLT_CI_EPHEMERAL_CUSTODY_ROOT": custody.as_posix(),
        "RUNNER_OS": "Windows"
        if os.name == "nt"
        else "macOS"
        if sys.platform == "darwin"
        else "Linux",
        "FAKE_SELECTED": selected.as_posix(),
        "FAKE_OTHER": other.as_posix(),
        "EVENTS": events.as_posix(),
        "FAILURE": failure,
    }
    if failure == "injected-root":
        env["MOLT_CI_EPHEMERAL_CUSTODY_ROOT"] += "\nEVIL=value"
    result = run_guarded_test_process(
        [str(BASH), str(ROOT / ".github/actions/setup-project/provision-python.sh")],
        prefix="MOLT_SETUP_PYTHON_BOOTSTRAP_TEST",
        cwd=directory,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    return (
        result,
        events.read_text(encoding="utf-8").splitlines() if events.exists() else [],
        {name: path.read_text(encoding="utf-8") for name, path in exports.items()},
    )


def test_python_bootstrap_selects_and_validates_before_publishing(
    tmp_path: Path,
) -> None:
    result, events, exports = _python_bootstrap(tmp_path)
    assert result.returncode == 0, result.stderr
    assert events == [
        "python install --default 3.12.15",
        "python find --managed-python --no-project --no-python-downloads 3.12.15",
        "python dir --bin",
        "validate:selected",
        "validate:python",
        "validate:python3",
    ]
    assert "UV_PYTHON_DOWNLOADS=never\n" in exports["GITHUB_ENV"]
    assert "UV_MANAGED_PYTHON=true\n" in exports["GITHUB_ENV"]
    assert "python-version=3.12.15\n" in exports["GITHUB_OUTPUT"]
    assert exports["GITHUB_PATH"].rstrip().endswith("/custody/python-bin")


@pytest.mark.parametrize(
    ("failure", "last_event"),
    [
        ("install", "python install --default 3.12.15"),
        (
            "find",
            "python find --managed-python --no-project --no-python-downloads 3.12.15",
        ),
        ("multiline", "python dir --bin"),
        ("version", "validate:selected"),
        ("implementation", "validate:selected"),
        ("alias", "validate:python3"),
        ("injected-root", None),
    ],
)
def test_python_bootstrap_failure_publishes_nothing(
    tmp_path: Path, failure, last_event
) -> None:
    result, events, exports = _python_bootstrap(tmp_path, failure=failure)
    assert result.returncode != 0
    assert (events[-1] if events else None) == last_event
    assert all(value == "" for value in exports.values())


@pytest.mark.parametrize(
    "pin",
    ["3.12\n", "3.12.15\n3.13.0\n", "3.12.15\n\n", " 3.12.15\n", "3.12.15\nEVIL=value"],
)
def test_python_bootstrap_rejects_malformed_pin_before_install(
    tmp_path: Path, pin
) -> None:
    result, events, exports = _python_bootstrap(tmp_path, pin=pin)
    assert result.returncode == 2
    assert events == []
    assert all(value == "" for value in exports.values())


def _run_normalizer(
    tmp_path: Path,
    *,
    toolchain: str,
    node_version: str = "",
    node_cache_dependency_path: str = "",
    components: str = "",
    targets: str = "",
    namespace: str = "project",
    sync: str = "false",
    sync_frozen: str = "false",
    sync_dev: str = "false",
    sync_groups: str = "",
    job: str = "fixture-job",
    python: str = "true",
    uv: str = "true",
    target_pythons: str = "false",
) -> dict[str, str]:
    tmp_path.mkdir(parents=True, exist_ok=True)
    output = tmp_path / "github-output"
    env = {
        **os.environ,
        "INPUT_PYTHON": python,
        "INPUT_UV": uv,
        "INPUT_CACHE_UV": "true",
        "INPUT_CACHE_CARGO": "false",
        "INPUT_CACHE_LEAN": "false",
        "INPUT_CACHE_NAMESPACE": namespace,
        "INPUT_ACTIONLINT": "false",
        "INPUT_TARGET_PYTHONS": target_pythons,
        "INPUT_RUST_TOOLCHAIN": toolchain,
        "INPUT_NODE_VERSION": node_version,
        "INPUT_NODE_CACHE_DEPENDENCY_PATH": node_cache_dependency_path,
        "INPUT_RUST_COMPONENTS": components,
        "INPUT_RUST_TARGETS": targets,
        "INPUT_SYNC": sync,
        "INPUT_SYNC_FROZEN": sync_frozen,
        "INPUT_SYNC_DEV": sync_dev,
        "INPUT_SYNC_GROUPS": sync_groups,
        "GITHUB_JOB": job,
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
    outputs = (
        dict(
            line.split("=", 1)
            for line in output.read_text(encoding="utf-8").splitlines()
        )
        if output.is_file()
        else {}
    )
    return completed.returncode, completed.stderr, outputs


def _normalize(tmp_path: Path, **inputs: str) -> dict[str, str]:
    returncode, stderr, outputs = _run_normalizer(tmp_path, **inputs)
    assert returncode == 0, stderr
    return outputs


def _pinned_rust_channel() -> str:
    return tomllib.loads((ROOT / "rust-toolchain.toml").read_text(encoding="utf-8"))[
        "toolchain"
    ]["channel"]


def test_stable_lists_are_sorted_deduplicated_and_cache_safe(tmp_path: Path) -> None:
    normalized = _normalize(
        tmp_path,
        toolchain="pinned",
        components="rustfmt, clippy, rustfmt",
        targets="wasm32-wasip1, aarch64-unknown-linux-gnu",
    )

    assert normalized["rust-toolchain"] == _pinned_rust_channel()
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
        toolchain="pinned",
        components="rustfmt, clippy",
        targets="wasm32-wasip1,aarch64-unknown-linux-gnu",
    )
    second = _normalize(
        tmp_path / "second",
        toolchain="pinned",
        components=" clippy ,rustfmt ",
        targets="aarch64-unknown-linux-gnu, wasm32-wasip1",
    )

    assert first["rust-cache-token"] == second["rust-cache-token"]


def test_cache_identity_separates_jobs(tmp_path: Path) -> None:
    # Jobs build different crate sets into different target directories; a
    # shared key would let one job's cache shadow every other job's.
    first = _normalize(tmp_path / "first", toolchain="pinned", job="rust-build")
    second = _normalize(tmp_path / "second", toolchain="pinned", job="llvm-backend")
    again = _normalize(tmp_path / "again", toolchain="pinned", job="rust-build")

    assert first["rust-cache-token"] != second["rust-cache-token"]
    assert first["rust-cache-token"] == again["rust-cache-token"]


def test_control_characters_and_empty_atoms_fail_closed(tmp_path: Path) -> None:
    for components in ("rustfmt,,clippy", "rustfmt\nclippy", "rustfmt\tclippy"):
        output = tmp_path / components.encode().hex()
        env = {
            **os.environ,
            "GITHUB_JOB": "fixture-job",
            "INPUT_PYTHON": "true",
            "INPUT_UV": "true",
            "INPUT_CACHE_UV": "true",
            "INPUT_CACHE_CARGO": "false",
            "INPUT_CACHE_LEAN": "false",
            "INPUT_CACHE_NAMESPACE": "project",
            "INPUT_ACTIONLINT": "false",
            "INPUT_TARGET_PYTHONS": "false",
            "INPUT_RUST_TOOLCHAIN": "pinned",
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
        "GITHUB_JOB": "fixture-job",
        "INPUT_PYTHON": "true",
        "INPUT_UV": "true",
        "INPUT_CACHE_UV": "true",
        "INPUT_CACHE_CARGO": "false",
        "INPUT_CACHE_LEAN": "false",
        "INPUT_CACHE_NAMESPACE": "project",
        "INPUT_ACTIONLINT": "false",
        "INPUT_TARGET_PYTHONS": "false",
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


def test_target_pythons_install_only_after_sync(tmp_path: Path) -> None:
    # The target-line authority imports project dependencies, so the install
    # step needs the synchronized environment.
    normalized = _normalize(
        tmp_path / "valid", toolchain="", sync="true", target_pythons="true"
    )
    assert normalized["target-pythons"] == "true"
    returncode, stderr, outputs = _run_normalizer(
        tmp_path / "invalid", toolchain="", target_pythons="true"
    )
    assert returncode == 2
    assert "target-pythons requires sync" in stderr
    assert outputs == {}


def test_shell_metacharacters_never_execute_before_validation(tmp_path: Path) -> None:
    marker = tmp_path / "executed"
    malicious = f'project"; touch "{marker}"; #'
    output = tmp_path / "github-output"
    env = {
        **os.environ,
        "GITHUB_JOB": "fixture-job",
        "INPUT_PYTHON": "true",
        "INPUT_UV": "true",
        "INPUT_CACHE_UV": "true",
        "INPUT_CACHE_CARGO": "false",
        "INPUT_CACHE_LEAN": "false",
        "INPUT_CACHE_NAMESPACE": malicious,
        "INPUT_ACTIONLINT": "false",
        "INPUT_TARGET_PYTHONS": "false",
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


def test_pinned_roles_resolve_from_their_one_authority(tmp_path: Path) -> None:
    normalized = _normalize(tmp_path, toolchain="pinned", node_version="pinned")

    assert normalized["rust-toolchain"] == _pinned_rust_channel()
    assert normalized["node-version"] == tool_releases.tool_release("node").version


def test_literal_versions_fail_closed(tmp_path: Path) -> None:
    channel = _pinned_rust_channel()
    node = tool_releases.tool_release("node").version
    for name, inputs, message in (
        ("rust", {"toolchain": channel}, "rust-toolchain must be 'pinned'"),
        ("stable", {"toolchain": "stable"}, "rust-toolchain must be 'pinned'"),
        (
            "node",
            {"toolchain": "", "node_version": node},
            "node-version must be 'pinned'",
        ),
        (
            "cache",
            {"toolchain": "", "node_cache_dependency_path": "package-lock.json"},
            "node-cache-dependency-path requires node-version",
        ),
    ):
        returncode, stderr, outputs = _run_normalizer(tmp_path / name, **inputs)
        assert returncode == 2, (name, stderr)
        assert message in stderr, (name, stderr)
        assert outputs == {}, name
