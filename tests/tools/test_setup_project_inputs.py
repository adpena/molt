from __future__ import annotations

from pathlib import Path
import os
import shlex
import sys
import sysconfig
import tomllib

import pytest

from molt import tool_releases
from molt.python_environment_identity import python_identity_verifier_arguments

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


def _python_bootstrap(tmp_path: Path, *, failure: str = "", pin: str = "3.15.1\n"):
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
        "import os, pathlib, platform, sys, sysconfig\n"
        "args = sys.argv[1:]\n"
        "events = open(os.environ['EVENTS'], 'a')\n"
        "if '-m' in args:\n"
        "    module_at = args.index('-m')\n"
        "    target = args[-1]\n"
        "    options = ' '.join(args[module_at + 2:-1])\n"
        "    stdlib = target == os.environ['FAKE_STDLIB']\n"
        "    events.write(f\"{args[module_at + 1]}:{os.environ['ALIAS']}:{options}:{stdlib}\\n\")\n"
        "    raise SystemExit(45 if os.environ['FAILURE'] == 'compile' else 0)\n"
        "code_at = args.index('-c')\n"
        "code = args[code_at + 1]\n"
        "sys.argv = ['-c', *args[code_at + 2:]]\n"
        "sys.executable = os.environ['FAKE_SELECTED']\n"
        "if os.environ['FAILURE'] == 'alias' and os.environ['ALIAS'] == 'python3':\n"
        "    sys.executable = os.environ['FAKE_OTHER']\n"
        "platform.python_version = lambda: '3.15.0' if os.environ['FAILURE'] == 'version' else '3.15.1'\n"
        "platform.python_implementation = lambda: 'PyPy' if os.environ['FAILURE'] == 'implementation' else 'CPython'\n"
        "sysconfig.get_path = lambda name, *_a, **_k: os.environ['FAKE_STDLIB']\n"
        "kind = 'stdlib' if 'sysconfig' in code else 'validate'\n"
        "events.write(kind + ':' + os.environ['ALIAS'] + '\\n')\n"
        "events.close()\n"
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
        "FAKE_STDLIB": (install / "lib" / "python3.15").as_posix(),
        "EVENTS": events.as_posix(),
        "FAILURE": failure,
    }
    if failure == "injected-root":
        env["MOLT_CI_EPHEMERAL_CUSTODY_ROOT"] += "\nEVIL=value"
    result = run_guarded_test_process(
        [str(BASH), str(ROOT / ".github/actions/setup-project/provision-python.sh")],
        prefix="MOLT_PYTEST",
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
        "python install --default 3.15.1",
        "python find --managed-python --no-project --no-python-downloads 3.15.1",
        "python dir --bin",
        "validate:selected",
        "validate:python",
        "validate:python3",
        "stdlib:selected",
        "compileall:selected:-q -j 0 -o 0 -o 1 -o 2 --invalidation-mode checked-hash:True",
    ]
    assert "UV_PYTHON_DOWNLOADS=never\n" in exports["GITHUB_ENV"]
    assert "UV_MANAGED_PYTHON=true\n" in exports["GITHUB_ENV"]
    assert "python-version=3.15.1\n" in exports["GITHUB_OUTPUT"]
    assert exports["GITHUB_PATH"].rstrip().endswith("/custody/python-bin")


def test_python_bootstrap_bytecode_does_not_depend_on_install_time(
    tmp_path: Path,
) -> None:
    """Every job installs the interpreter afresh; uv stamps the files with the
    install time. The bootstrap's own compile options must still produce the
    same bytecode, or jobs disagree on the runtime build identity."""
    result, events, _exports = _python_bootstrap(tmp_path)
    assert result.returncode == 0, result.stderr
    (compile_event,) = [event for event in events if event.startswith("compileall:")]
    options = compile_event.split(":")[2].split()
    source_root = Path(sysconfig.get_path("stdlib"))
    source = (source_root / "_strptime.py").read_bytes()
    active = tmp_path / "active"
    module = active / "stdlib" / "_strptime.py"
    compiled: list[dict[str, bytes]] = []
    for install_time in (1_700_000_000, 1_800_000_000):
        assert not active.exists()
        module.parent.mkdir(parents=True)
        module.write_bytes(source)
        os.utime(module, (install_time, install_time))
        # Compileall's parallel workers need not inherit the parent's cache
        # prefix. Give every explicit output one fresh test-owned tree while
        # keeping the source path stable for the independent bytecode oracle.
        cache = active / "imports"
        cache.mkdir()
        completed = run_guarded_test_process(
            [
                sys.executable,
                *python_identity_verifier_arguments(cache),
                "-m",
                "compileall",
                *options,
                str(module.parent),
            ],
            prefix="MOLT_PYTEST",
            capture_output=True,
            text=True,
            check=False,
        )
        assert completed.returncode == 0, completed.stderr
        bytecode = tuple(active.rglob("*.pyc"))
        assert all(path.name.startswith(f"{module.stem}.") for path in bytecode)
        compiled.append({path.name: path.read_bytes() for path in bytecode})
        assert len(bytecode) == 3
        active.rename(tmp_path / f"cache-{install_time}")
    assert len(compiled[0]) == 3
    assert compiled[0] == compiled[1]


@pytest.mark.parametrize(
    ("failure", "last_event"),
    [
        ("install", "python install --default 3.15.1"),
        (
            "find",
            "python find --managed-python --no-project --no-python-downloads 3.15.1",
        ),
        ("multiline", "python dir --bin"),
        ("version", "validate:selected"),
        ("implementation", "validate:selected"),
        ("alias", "validate:python3"),
        (
            "compile",
            "compileall:selected:-q -j 0 -o 0 -o 1 -o 2 --invalidation-mode checked-hash:True",
        ),
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
    sync_project_groups: str = "false",
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
        "INPUT_SYNC_PROJECT_GROUPS": sync_project_groups,
        "GITHUB_JOB": job,
    }
    completed = run_guarded_test_process(
        [str(BASH), str(NORMALIZER), str(output)],
        prefix="MOLT_PYTEST",
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
            "INPUT_SYNC_PROJECT_GROUPS": "false",
        }
        completed = run_guarded_test_process(
            [str(BASH), str(NORMALIZER), str(output)],
            prefix="MOLT_PYTEST",
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
        "INPUT_SYNC_PROJECT_GROUPS": "false",
    }
    completed = run_guarded_test_process(
        [str(BASH), str(NORMALIZER), str(output)],
        prefix="MOLT_PYTEST",
        cwd=ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert completed.returncode == 2
    assert not output.exists()


def test_target_pythons_install_only_after_sync(tmp_path: Path) -> None:
    # Keep target-line provisioning after repository environment synchronization.
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


def _run_target_python_setup(tmp_path: Path, failure: str):
    # Run the actual action step, not a reconstruction of its shell commands.
    action = (ROOT / ".github/actions/setup-project/action.yml").read_text(
        encoding="utf-8"
    )
    step = action.split("    - name: Install supported target CPython lines\n", 1)[1]
    step = step.split("\n    - ", 1)[0]
    body = step.split("      run: |\n", 1)[1]
    script = tmp_path / "target-python-setup.sh"
    script.write_text(
        "\n".join(line.removeprefix("        ") for line in body.splitlines()) + "\n",
        encoding="utf-8",
    )
    source = tmp_path / "catalog-source"
    package = source / "molt"
    package.mkdir(parents=True)
    (package / "__init__.py").write_text("", encoding="utf-8")
    (package / "target_python_policy.py").write_text(
        (
            "import sys\nsys.stdout.reconfigure(newline='\\r\\n')\n"
            if failure == "crlf"
            else ""
        )
        + "SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS = ('3.14', '3.12')\n",
        encoding="utf-8",
    )
    (package / "target_python.py").write_text(
        "raise AssertionError('retired catalog import')\n", encoding="utf-8"
    )
    tools = tmp_path / "tools"
    tools.mkdir()
    events, installed, selection = (
        tmp_path / name for name in ("events", "installed", "selection")
    )
    selected = Path(sys.executable).absolute().as_posix()
    query_driver = (
        "import sys; sys.path.insert(0, sys.argv[1]); "
        "exec(compile(sys.argv[2], '<catalog query>', 'exec'), "
        "{'__name__': '__main__'})"
    )
    uv = tools / "uv"
    uv.write_text(
        "#!/usr/bin/env bash\nset -euo pipefail\n"
        'case "${1-} ${2-}" in\n'
        ' "run --no-sync")\n'
        '  [[ "${3-} ${4-}" == "python3 -c" ]] || exit 44\n'
        '  printf "query\\n" >> "$EVENTS"\n'
        '  printf "%s\\n" "$UV_PYTHON" "$UV_PYTHON_DOWNLOADS" >> "$SELECTION"\n'
        '  if [[ "$FAILURE" == query ]]; then printf "3.14 3.12\\n"; exit 43; fi\n'
        '  [[ "$FAILURE" != empty ]] || exit 0\n'
        f"  exec {shlex.quote(Path(sys.executable).as_posix())} -I -S -B -c "
        f'{shlex.quote(query_driver)} "$CATALOG_SOURCE" "$5" ;;\n'
        ' "python install")\n'
        '  printf "install\\n" >> "$EVENTS"\n'
        '  printf "%s\\n" "$@" >> "$INSTALLED"\n'
        '  printf "%s\\n" "$UV_PYTHON" "$UV_PYTHON_DOWNLOADS" >> "$SELECTION" ;;\n'
        " *) exit 44 ;;\nesac\n",
        encoding="utf-8",
    )
    uv.chmod(0o755)
    result = run_guarded_test_process(
        [str(BASH), str(script)],
        prefix="MOLT_PYTEST",
        cwd=tmp_path,
        env={
            **os.environ,
            "PATH": str(tools) + os.pathsep + os.environ["PATH"],
            "CATALOG_SOURCE": source.as_posix(),
            "FAILURE": failure,
            "EVENTS": events.as_posix(),
            "INSTALLED": installed.as_posix(),
            "SELECTION": selection.as_posix(),
            "UV_PYTHON": selected,
            "UV_PYTHON_DOWNLOADS": "never",
        },
        capture_output=True,
        text=True,
        check=False,
    )
    # Preserve literal CR bytes in the install argv; splitlines would hide them.
    records = [
        path.read_bytes().decode("utf-8").removesuffix("\n").split("\n")
        if path.exists()
        else []
        for path in (events, installed, selection)
    ]
    return result, records, selected


@pytest.mark.parametrize("failure", ["", "crlf", "query", "empty"])
def test_target_python_setup_requires_catalog_before_install(tmp_path: Path, failure):
    result, (events, installed, selection), selected = _run_target_python_setup(
        tmp_path, failure
    )
    assert selection[:2] == [selected, "never"]
    if failure in {"query", "empty"}:
        assert result.returncode == (43 if failure == "query" else 2), result.stderr
        assert events == ["query"]
        assert installed == []
        assert selection == [selected, "never"]
    else:
        assert result.returncode == 0, result.stderr
        assert events == ["query", "install"]
        assert installed == ["python", "install", "3.14", "3.12"]
        assert selection == [selected, "never", selected, "manual"]


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
        "INPUT_SYNC_PROJECT_GROUPS": "false",
    }
    completed = run_guarded_test_process(
        [str(BASH), str(NORMALIZER), str(output)],
        prefix="MOLT_PYTEST",
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
    assert normalized["uv-version"] == tool_releases.tool_release("uv").version


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


@pytest.mark.parametrize("version", ["99.88.77", "latest", "1.2", "1.2.3\n4.5.6"])
def test_uv_normalization_reads_exact_manifest_coordinate(
    tmp_path, monkeypatch, version
):
    # Run the actual shell consumer against an independent authority fixture;
    # a literal workflow pin or first-line-only read cannot pass this oracle.
    config = tmp_path / "config"
    config.mkdir()
    (config / "tool_releases.toml").write_text(
        f'[tools.uv]\nversion = "{version}"\n', encoding="utf-8"
    )
    monkeypatch.setattr(sys.modules[__name__], "ROOT", tmp_path)
    code, error, outputs = _run_normalizer(tmp_path / "run", toolchain="")
    if version == "99.88.77":
        assert code == 0, error
        assert outputs["uv-version"] == version
    else:
        assert code == 2
        assert "must declare exactly one X.Y.Z" in error
        assert outputs == {}


@pytest.mark.parametrize("value", ["TRUE", "1", ""])
def test_project_group_selector_is_a_strict_boolean(tmp_path: Path, value: str):
    code, error, outputs = _run_normalizer(
        tmp_path, toolchain="", sync="true", sync_project_groups=value
    )
    assert code != 0
    assert outputs == {}
    if value:
        assert "sync-project-groups must be exactly true or false" in error


@pytest.mark.parametrize(
    "options",
    [{}, {"sync": "true", "sync_dev": "true"}, {"sync": "true", "sync_groups": "dev"}],
)
def test_project_group_selector_refuses_ambiguous_or_disabled_sync(tmp_path, options):
    code, error, outputs = _run_normalizer(
        tmp_path, toolchain="", sync_project_groups="true", **options
    )
    assert code == 2
    assert "sync" in error
    assert outputs == {}


def test_project_group_selector_normalizes_without_resolving_before_bootstrap(tmp_path):
    normalized = _normalize(
        tmp_path, toolchain="", sync="true", sync_project_groups="true"
    )
    assert normalized["sync-project-groups"] == "true"
    assert normalized["sync-groups"] == ""


def _run_project_group_sync(tmp_path: Path, mode: str):
    action = (ROOT / ".github/actions/setup-project/action.yml").read_text(
        encoding="utf-8"
    )
    step = action.split("    - name: Synchronize project dependencies\n", 1)[1]
    body = step.split("\n    - ", 1)[0].split("      run: |\n", 1)[1]
    script = tmp_path / "sync.sh"
    script.write_text(
        "\n".join(line.removeprefix("        ") for line in body.splitlines()) + "\n",
        encoding="utf-8",
    )
    tools = tmp_path / "bound-python-and-uv"
    tools.mkdir()
    query, synced = tmp_path / "query", tmp_path / "synced"
    selected = shlex.quote(Path(sys.executable).absolute().as_posix())
    python = tools / "python3"
    python.write_text(
        "#!/usr/bin/env bash\nset -euo pipefail\n"
        'printf "%s\\n" "$@" > "$QUERY"\n'
        'if [[ "$MODE" == query ]]; then printf "plausible-group\\n"; exit 43; fi\n'
        '[[ "$MODE" != explicit ]] || exit 44\n'
        f'exec {selected} "$@"\n',
        encoding="utf-8",
    )
    python.chmod(0o755)
    uv = tools / "uv"
    uv.write_text(
        '#!/usr/bin/env bash\nset -euo pipefail\nprintf "%s\\n" "$@" > "$SYNCED"\n',
        encoding="utf-8",
    )
    uv.chmod(0o755)
    cwd = ROOT
    if mode in {"changed", "invalid", "framing", "crlf"}:
        cwd = tmp_path / "different project"
        package = cwd / "src/molt"
        package.mkdir(parents=True)
        (package / "__init__.py").write_text("", encoding="utf-8")
        # A distinguishable authority proves the consumer calls its method,
        # rather than reading a copied constant or importing installed Molt.
        (package / "dx.py").write_text(
            "import sys\nfrom pathlib import Path\n"
            "assert sys.flags.isolated and sys.flags.no_site and sys.dont_write_bytecode\n"
            # Model Windows pipe translation on every host. The actual action
            # producer must emit LF bytes after calling this authority.
            + ("sys.stdout.reconfigure(newline='\\r\\n')\n" if mode == "crlf" else "")
            + "class DxProject:\n"
            "    def __init__(self, root): assert root == Path.cwd()\n"
            "    def dependency_groups(self):\n"
            + (
                "        raise ValueError('invalid dependency groups')\n"
                if mode == "invalid"
                else "        return ('bad\\ngroup',)\n"
                if mode == "framing"
                else "        return ('fixture-a', 'fixture-z')\n"
            ),
            encoding="utf-8",
        )
    result = run_guarded_test_process(
        [str(BASH), str(script)],
        prefix="MOLT_PYTEST",
        cwd=cwd,
        env={
            **os.environ,
            "PATH": str(tools) + os.pathsep + os.environ["PATH"],
            "QUERY": query.as_posix(),
            "SYNCED": synced.as_posix(),
            "MODE": mode,
            "SYNC_FROZEN": "true",
            "SYNC_DEV": "false",
            "SYNC_GROUPS": "fixture-a,fixture-z" if mode == "explicit" else "",
            "SYNC_PROJECT_GROUPS": "false" if mode == "explicit" else "true",
        },
        capture_output=True,
        text=True,
        check=False,
    )
    # Universal-newline decoding would hide a literal CR in an argv value.
    records = [
        path.read_bytes().decode("utf-8").removesuffix("\n").split("\n")
        if path.exists()
        else []
        for path in (query, synced)
    ]
    return result, records


@pytest.mark.parametrize(
    "mode", ["repository", "changed", "crlf", "query", "invalid", "framing", "explicit"]
)
def test_project_group_sync_uses_bound_python_and_preserves_query_failure(
    tmp_path: Path, mode: str
):
    result, (query, synced) = _run_project_group_sync(tmp_path, mode)
    if mode == "explicit":
        assert result.returncode == 0, result.stderr
        assert query == []
        assert synced == [
            "sync",
            "--frozen",
            "--group",
            "fixture-a",
            "--group",
            "fixture-z",
        ]
        return
    assert query[:4] == ["-I", "-S", "-B", "-c"]
    if mode in {"query", "invalid", "framing"}:
        assert result.returncode == (43 if mode == "query" else 1), result.stderr
        assert synced == [], "failed group selection must not install any dependency"
        return
    assert result.returncode == 0, result.stderr
    if mode == "repository":
        project = tomllib.loads((ROOT / "pyproject.toml").read_text(encoding="utf-8"))
        groups = sorted(set(project["tool"]["molt"]["dx"]["dependency_groups"]))
    else:
        groups = ["fixture-a", "fixture-z"]
    assert synced == [
        "sync",
        "--frozen",
        "--no-default-groups",
        *(argument for group in groups for argument in ("--group", group)),
    ]
