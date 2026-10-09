"""Real supervisor execution/verification; never selected by the pure unit lane."""

from functools import lru_cache, partial
import os
from pathlib import Path
import tempfile

import pytest

from tests.process_guard_common import run_custody_subject_process
from tests import proof_queue_owned_roots
from tests.proof_queue_custody_test_support import (
    assert_execution_context_rejects_substitutions,
    publish_receipt_custody,
)
from tools.proof_queue_pkg import supervisor_custody, supervisor_generation
from tools.proof_queue_pkg import toolchain_capture
from molt.toolchain_identity import find_executable

pytestmark = pytest.mark.slow


@pytest.mark.parametrize("native_c", [False, True])
def test_rust_link_capture_owns_workspace_inside_owner_selected_scratch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, native_c: bool
) -> None:
    """Real Cargo must accept both probe crates below an enclosing workspace."""
    tmp_path = proof_queue_owned_roots.native_case_path(
        tmp_path,
        source=Path(__file__),
        nodeid="rust-link-capture-workspace",
    )
    workspace = tmp_path / "workspace"
    scratch = workspace / "scratch"
    scratch.mkdir(parents=True)
    manifest = workspace / "Cargo.toml"
    original = '[workspace]\nmembers=[]\nresolver="3"\n'
    manifest.write_text(original, encoding="utf-8")
    monkeypatch.setattr(tempfile, "tempdir", str(scratch))
    env = dict(os.environ)
    rustc = find_executable("rustc", environment=env)
    cargo = find_executable("cargo", environment=env)
    assert rustc is not None and cargo is not None, "native proof requires Rust tools"
    from molt.rust_toolchain import resolve_rustup_proxy

    rustc = resolve_rustup_proxy(rustc, role="rustc", root=workspace, env=env)
    cargo = resolve_rustup_proxy(cargo, role="cargo", root=workspace, env=env)

    from tools import proof_plan

    command = (
        list(
            next(
                row.argv
                for row in proof_plan.ProofPlan.load().commands
                if row.id == "wasm.build.host"
            )
        )
        if native_c
        else ["cargo", "build", "--release"]
    )
    images, telemetry = toolchain_capture.capture_rust_link_process_images(
        rustc=rustc,
        cargo=cargo,
        cwd=workspace,
        env=env,
        target=None,
        command_argv=command,
        admitted_command=command,
        native_c_units=["target"] if native_c else [],
    )
    toolchain_capture.validate_rust_link_selection(
        {"process_images": images, "link_selection": telemetry}
    )
    assert bool(telemetry["native_c"]) is native_c

    assert {row["role"] for row in images} >= {"rust-linker"}
    assert [unit["unit"] for unit in telemetry["units"]] == [
        "target",
        "host-proc-macro",
    ]
    assert all(
        Path(unit["compiler_cwd"]).is_relative_to(scratch)
        and unit["metadata_probe_count"] == 1
        and unit["selection_probe_count"] == 1
        for unit in telemetry["units"]
    )
    assert manifest.read_text(encoding="utf-8") == original
    assert not list(scratch.iterdir()), "probe scratch must be released after capture"


@lru_cache(maxsize=1)
def _native_supervisor_binary() -> Path:
    binary, _receipt = supervisor_generation.provision(
        cwd=Path(supervisor_custody.__file__).resolve().parents[2],
        env=proof_queue_owned_roots.native_build_environment(source=Path(__file__)),
    )
    return binary


def test_native_execution_context_rehashes_nonce_custody_and_transcript_artifacts(
    tmp_path: Path,
) -> None:
    """Prove the same binding assertions through the actual native verifier."""
    tmp_path = proof_queue_owned_roots.native_case_path(
        tmp_path,
        source=Path(__file__),
        nodeid="native-context-receipt",
    )

    def execute(command: list[str]) -> None:
        run_custody_subject_process(command, check=True)

    binary = _native_supervisor_binary()
    required_environment = supervisor_custody.required_execution_environment(
        binary=binary, mode="leaf", cwd=tmp_path, env={}
    )
    factory = partial(
        publish_receipt_custody,
        supervisor_binary=binary,
        execute_supervisor=execute,
        required_environment=required_environment,
    )
    assert_execution_context_rejects_substitutions(tmp_path, factory)
