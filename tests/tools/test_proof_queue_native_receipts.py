"""Real supervisor execution/verification; never selected by the pure unit lane."""

from functools import lru_cache, partial
import hashlib
import json
import os
import secrets
import sys
from pathlib import Path
import tempfile

import pytest

from tests.process_guard_common import run_custody_subject_process
from tests import proof_queue_owned_roots
from tests.proof_queue_custody_test_support import (
    assert_execution_context_rejects_substitutions,
    publish_receipt_custody,
    synthetic_python_toolchain,
)
from tools.proof_queue_pkg import supervisor_custody, supervisor_generation
from tools.proof_queue_pkg import (
    command_admission,
    process_image_capture,
    toolchain_capture,
)
from molt.toolchain_identity import find_executable

pytestmark = pytest.mark.slow


@pytest.mark.parametrize("operation", ["rust", "c", "c++", "both"])
def test_rust_link_capture_owns_workspace_inside_owner_selected_scratch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operation: str
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
    if operation == "c++" and sys.platform == "linux":
        # This cell proves GCC's separate C++ frontend, not an in-process
        # Clang frontend. Select its installed physical driver before capture.
        compiler = find_executable("g++", environment=env)
        assert compiler is not None, "Linux C++ custody proof requires g++"
        env = {
            name: value
            for name, value in env.items()
            if name not in {"CXX", "HOST_CXX", "TARGET_CXX"}
            and not name.startswith("CXX_")
        }
        env["CXX"] = str(compiler.resolve(strict=True))
    rustc = find_executable("rustc", environment=env)
    cargo = find_executable("cargo", environment=env)
    assert rustc is not None and cargo is not None, "native proof requires Rust tools"
    from molt.rust_toolchain import resolve_rustup_proxy

    rustc = resolve_rustup_proxy(rustc, role="rustc", root=workspace, env=env)
    cargo = resolve_rustup_proxy(cargo, role="cargo", root=workspace, env=env)

    from tools import proof_plan

    command_id, requirements = {
        "rust": (None, {}),
        "c": ("wasm.build.host", {"target": ["c"]}),
        "c++": ("mlir.test.backend", {"host": ["c++"]}),
        "both": ("rust.test.default-truth", {"target": ["c", "c++"]}),
    }[operation]
    command = (
        list(
            next(
                row.argv
                for row in proof_plan.ProofPlan.load().commands
                if row.id == command_id
            )
        )
        if command_id is not None
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
        native_units=requirements,
    )
    # Link capture owns descendants; the policy consumer also requires the
    # launcher's real byte identity. Rustup resolution above selected one
    # physical image for both launcher and content, so capture it once.
    launcher = process_image_capture.capture_image(
        "rustc-launcher", rustc, preserve_path=True
    )
    rustc_identity = {
        "path": launcher["path"],
        "launcher_sha256": launcher["sha256"],
        "content_path": launcher["path"],
        "executable_sha256": launcher["sha256"],
        "process_images": [launcher, *images],
        "link_selection": telemetry,
    }
    projected = process_image_capture.toolchain_images("rustc", rustc_identity)
    assert launcher in projected
    for field, label in (("path", "launcher"), ("content_path", "content")):
        incomplete = {
            key: value for key, value in rustc_identity.items() if key != field
        }
        with pytest.raises(
            ValueError, match=f"rustc toolchain has no {label} image identity"
        ):
            process_image_capture.toolchain_images("rustc", incomplete)
    assert bool(telemetry["native_build"]) is bool(requirements)

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

    if operation == "c++" and sys.platform == "linux":
        native = telemetry["native_build"][0]
        assert native["selection"]["units"] == {"host": ["c++"]}
        compiler = native["selection"]["compilers"]["c++"]
        frontend = next(
            Path(helper["path"])
            for phase in native["compilers"]["c++"]["phases"]
            for helper in phase["helpers"]
            if Path(helper["path"]).name == "cc1plus"
        )
        source = tmp_path / "frontend.cc"
        source.write_text(
            "template<int N> struct Answer { static constexpr int value = N; };\n"
            "static_assert(Answer<42>::value == 42);\n"
            'extern "C" int answer() { return Answer<42>::value; }\n',
            encoding="utf-8",
        )
        binary = _native_supervisor_binary()
        required = supervisor_custody.required_execution_environment(
            binary=binary, mode="declared-tree", cwd=tmp_path, env=env
        )
        execution_env = supervisor_custody.bind_required_environment(env, required)
        # Compilation produces data only. Do not grant the object directory
        # executable-image authority that could conceal a missing helper.
        execution_env.pop("CARGO_TARGET_DIR", None)
        execution_env.pop(supervisor_custody.PROOF_SCRATCH_ROOT_ENV, None)
        for admit_frontend in (True, False):
            label = "admitted" if admit_frontend else "missing-frontend"
            output = tmp_path / f"{label}.o"
            policy_path = tmp_path / f"{label}.policy.json"
            receipt_path = tmp_path / f"{label}.receipt.json"
            policy = supervisor_custody._supervisor_policy(
                envelope=command_admission.envelope_for_command(command),
                execution_command=[*compiler, "-c", str(source), "-o", str(output)],
                execution_env=execution_env,
                cwd=tmp_path,
                nonce=secrets.token_hex(32),
                toolchains={"rustc": rustc_identity},
                environment_executables={},
                platform_process_images=(),
            )
            if not admit_frontend:
                policy["fixed_images"] = [
                    row
                    for row in policy["fixed_images"]
                    if Path(row["path"]).resolve(strict=True) != frontend
                ]
            assert policy["derived_roots"] == []
            supervisor_custody.publish_supervisor_policy(policy_path, policy)
            completed = run_custody_subject_process(
                [
                    str(binary),
                    "run",
                    "--policy",
                    str(policy_path),
                    "--receipt",
                    str(receipt_path),
                ],
                cwd=tmp_path,
                env=execution_env,
                capture_output=True,
                text=True,
                check=False,
                timeout=30,
            )
            receipt = supervisor_custody._validated_supervisor_receipt(
                binary=binary,
                policy_path=policy_path,
                receipt_path=receipt_path,
                cwd=tmp_path,
                env=execution_env,
            )
            if admit_frontend:
                assert completed.returncode == 0, completed.stderr
                assert supervisor_custody.supervisor_receipt_is_complete(receipt)
                assert receipt["root_exit_code"] == 0
                assert output.read_bytes().startswith(b"\x7fELF")
                _, observed = supervisor_custody._verified_supervisor_event_artifact(
                    receipt_path=receipt_path,
                    descriptor=receipt["event_log"],
                    collect_images=True,
                )
                assert str(frontend) in {path for path, _digest, _size in observed}
            else:
                assert completed.returncode != 0
                assert not supervisor_custody.supervisor_receipt_is_complete(receipt)
                assert not output.exists(), (
                    "unadmitted cc1plus must not produce an object"
                )
                assert any(
                    "unadmitted executable image" in violation
                    and str(frontend) in violation
                    for violation in receipt["violations"]
                ), receipt


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
    try:
        required_environment = supervisor_custody.required_execution_environment(
            binary=binary, mode="leaf", cwd=tmp_path, env={}
        )
    except supervisor_custody.SupervisorPrelaunchRefused as refusal:
        # An actual planner refusal proves only prelaunch ineligibility. A
        # later eligible launch failure must still fail this execution test.
        assert refusal.capability["admission"]["state"] == "ineligible"
        assert refusal.capability["admission"]["reason"]
        assert not list(tmp_path.iterdir())
        return
    factory = partial(
        publish_receipt_custody,
        supervisor_binary=binary,
        execute_supervisor=execute,
        required_environment=required_environment,
    )
    assert_execution_context_rejects_substitutions(tmp_path, factory)


def test_native_verifier_binds_consumer_to_raw_receipt_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    tmp_path = proof_queue_owned_roots.native_case_path(
        tmp_path, source=Path(__file__), nodeid="native-raw-receipt-binding"
    )
    binary = _native_supervisor_binary()
    try:
        required = supervisor_custody.required_execution_environment(
            binary=binary, mode="leaf", cwd=tmp_path, env={}
        )
    except supervisor_custody.SupervisorPrelaunchRefused as refusal:
        assert refusal.capability["admission"]["state"] == "ineligible"
        assert not list(tmp_path.iterdir())
        return

    def execute(command: list[str]) -> None:
        run_custody_subject_process(command, check=True)

    _, _, custody = publish_receipt_custody(
        tmp_path,
        {"python": synthetic_python_toolchain(tmp_path)},
        supervisor_binary=binary,
        execute_supervisor=execute,
        required_environment=required,
    )
    supervisor = custody["supervisor"]
    binary_path = Path(str(supervisor["binary"]["path"]))
    policy_path = Path(str(supervisor["policy"]["path"]))
    receipt_path = Path(str(supervisor["receipt_file"]["path"]))
    original = receipt_path.read_bytes()
    alternate = b" \n" + original + b"\n "
    original_verifier = supervisor_custody.command_identity._run_captured
    observed = []

    def verify_other_generation(command, **kwargs):
        # The native verifier really accepts the alternate legal JSON bytes.
        # Restore the original pathname contents before returning its response.
        receipt_path.write_bytes(alternate)
        try:
            result = original_verifier(command, **kwargs)
            assert result.returncode == 0, result.stderr
            response = json.loads(result.stdout)
            assert response["receipt_sha256"] == hashlib.sha256(alternate).hexdigest()
            assert response["receipt_bytes"] == len(alternate)
            observed.append(response)
        finally:
            receipt_path.write_bytes(original)
        return result

    monkeypatch.setattr(
        supervisor_custody.command_identity, "_run_captured", verify_other_generation
    )
    with pytest.raises(ValueError, match="verified different receipt bytes"):
        supervisor_custody._validated_supervisor_receipt(
            binary=binary_path,
            policy_path=policy_path,
            receipt_path=receipt_path,
            cwd=tmp_path,
            env=required,
        )
    assert len(observed) == 1
    assert receipt_path.read_bytes() == original
