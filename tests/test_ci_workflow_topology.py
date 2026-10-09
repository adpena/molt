from __future__ import annotations

from pathlib import Path
from fnmatch import fnmatchcase
import glob
import json
import re
import tomllib

import pytest
import yaml

from molt import tool_releases

from tools.proof_counts import fail_closed_proof_exit_code


REPO_ROOT = Path(__file__).resolve().parents[1]
WORKFLOW_ROOT = REPO_ROOT / ".github" / "workflows"


def _read(path: str) -> str:
    return (REPO_ROOT / path).read_text(encoding="utf-8")


def _literal_job_needs(job: dict[str, object]) -> tuple[str, ...]:
    raw = job.get("needs")
    if raw is None:
        return ()
    if isinstance(raw, str):
        return (raw,)
    assert isinstance(raw, list)
    assert all(isinstance(item, str) for item in raw)
    return tuple(raw)


def _named_step_blocks(workflow_text: str) -> list[str]:
    blocks: list[list[str]] = []
    current: list[str] = []
    for line in workflow_text.splitlines():
        if line.startswith("      - name: "):
            if current:
                blocks.append(current)
            current = [line]
        elif current:
            current.append(line)
    if current:
        blocks.append(current)
    return ["\n".join(block) for block in blocks]


def _default_python_version() -> str:
    """The exact tooling CPython; generator oracles differ between patches."""
    version = _read(".python-version").strip()
    components = version.split(".")
    assert len(components) == 3
    assert all(component.isdigit() for component in components)
    return version


def test_setup_project_callers_use_declared_inputs() -> None:
    action = yaml.safe_load(_read(".github/actions/setup-project/action.yml"))
    declared = set(action["inputs"])
    calls = 0
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        payload = yaml.safe_load(workflow.read_text(encoding="utf-8"))
        for job in payload.get("jobs", {}).values():
            for step in job.get("steps", []):
                if step.get("uses") != "./.github/actions/setup-project":
                    continue
                calls += 1
                assert set(step.get("with", {})) <= declared, workflow
    assert calls >= 10


def test_every_cargo_cache_job_saves_even_when_its_proofs_fail() -> None:
    # actions/cache saves only after a successful job, so a red job never
    # warmed its own cache and cold-compiled every dependency on every run.
    cache_jobs = 0
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        payload = yaml.safe_load(workflow.read_text(encoding="utf-8"))
        for name, job in payload.get("jobs", {}).items():
            steps = job.get("steps", [])
            caches = any(
                step.get("uses") == "./.github/actions/setup-project"
                and str(step.get("with", {}).get("cache-cargo")) == "true"
                for step in steps
            )
            saves = [
                index
                for index, step in enumerate(steps)
                if step.get("uses") == "./.github/actions/save-cargo-cache"
            ]
            assert bool(saves) == caches, (workflow.name, name)
            if caches:
                cache_jobs += 1
                assert saves == [len(steps) - 1], (workflow.name, name)
                assert steps[-1]["if"] == "${{ !cancelled() }}", (workflow.name, name)
    assert cache_jobs >= 10

    save = yaml.safe_load(_read(".github/actions/save-cargo-cache/action.yml"))
    prune, persist = save["runs"]["steps"]
    restore = next(
        step
        for step in yaml.safe_load(_read(".github/actions/setup-project/action.yml"))[
            "runs"
        ]["steps"]
        if step.get("id") == "cargo-restore"
    )
    main_only = "env.MOLT_CARGO_CACHE_KEY != '' && github.ref == 'refs/heads/main'"
    assert prune["if"] == persist["if"] == main_only
    assert prune["run"] == "python3 tools/ci_cargo_cache.py prune"
    assert re.fullmatch(r"actions/cache/save@[0-9a-f]{40}", persist["uses"])
    assert persist["uses"].split("@")[1] == restore["uses"].split("@")[1]
    # actions/cache folds the path list into the cache version.
    assert persist["with"]["path"] == restore["with"]["path"]
    assert persist["with"]["key"] == "${{ env.MOLT_CARGO_CACHE_KEY }}"


def test_setup_project_cache_identity_is_complete_and_non_incremental() -> None:
    action = _read(".github/actions/setup-project/action.yml")
    normalizer = _read(".github/actions/setup-project/normalize-inputs.sh")
    for token in (
        "inputs.cache-namespace",
        "inputs.rust-toolchain",
        "inputs.rust-components",
        "inputs.rust-targets",
        "rust-toolchain.toml",
        "Cargo.lock",
        "**/Cargo.toml",
        "config/llvm_toolchain_releases.toml",
        "config/llvm_toolchain_arches.toml",
    ):
        assert token in action
    # The proof plan is not a Cargo input; hashing it re-keyed every cache on
    # each unrelated proof edit.
    assert "tools/proof_plan.toml" not in action
    # Jobs build different crate sets into different target directories, and
    # actions/cache folds paths into the cache version: the job id must be part
    # of the identity or one job's key shadows every other job's cache.
    assert '"${GITHUB_JOB:?}" | git hash-object --stdin' in normalizer
    steps = yaml.safe_load(action)["runs"]["steps"]
    configure = next(step for step in steps if step.get("id") == "cargo-cache")
    restore = next(step for step in steps if step.get("id") == "cargo-restore")
    record = next(
        step for step in steps if step.get("name") == "Record Cargo cache save key"
    )
    assert re.fullmatch(r"actions/cache/restore@[0-9a-f]{40}", restore["uses"])
    assert (
        configure["if"]
        == restore["if"]
        == record["if"]
        == "steps.inputs.outputs.cache-cargo == 'true'"
    )
    assert configure["run"] == "python3 tools/ci_cargo_cache.py configure"
    cached_paths = restore["with"]["path"].split()
    assert "${{ env.MOLT_CARGO_CACHE_TARGET }}" in restore["with"]["path"]
    for source in (
        "~/.cargo/registry/index",
        "~/.cargo/registry/cache",
        "~/.cargo/git/db",
    ):
        assert source in cached_paths
    for token in ("runner.os", "runner.arch", "steps.inputs.outputs.rust-cache-token"):
        assert token in restore["with"]["key"]
        assert token in restore["with"]["restore-keys"]
    # A per-run key is never an exact hit, so main always saves a fresh cache;
    # restore-keys prefer the newest cache for the same lockfile.
    assert "github.run_id" in restore["with"]["key"]
    assert "github.run_attempt" in restore["with"]["key"]
    assert "github.run_id" not in restore["with"]["restore-keys"]
    assert "hashFiles(" in restore["with"]["key"]
    assert "steps.cargo-restore.outputs.cache-primary-key" in str(record["env"])
    assert "MOLT_CARGO_CACHE_KEY=" in record["run"]
    install = next(step for step in steps if step.get("name") == "Install exact Rust")
    assert (
        steps.index(install)
        < steps.index(configure)
        < steps.index(restore)
        < steps.index(record)
    )
    assert sum("actions/cache/restore@" in str(step.get("uses")) for step in steps) == 1
    assert "cache-uv requires uv" in normalizer
    assert "sync requires uv" in normalizer
    assert "cache-cargo requires rust-toolchain" in normalizer
    assert "actionlint requires python" in normalizer
    assert "dtolnay/rust-toolchain@" not in action
    assert (
        install["run"]
        == "python3 -I -B .github/actions/setup-project/provision-rust.py"
    )
    assert install["env"]["RUST_TOOLCHAIN_ROLE"] == "${{ inputs.rust-toolchain }}"
    assert "RUSTUP_AUTO_INSTALL=0" in steps[0]["run"]
    assert 'if [[ -n "$SYNC_GROUPS" ]]' in action
    group_guard = action.split('if [[ -n "$SYNC_GROUPS" ]]', 1)[1].split(
        "        fi", 1
    )[0]
    assert 'for group in "${groups[@]}"' in group_guard
    assert "steps.inputs.outputs.rust-cache-token" in action
    assert "steps.inputs.outputs.cache-namespace" in action
    assert "sync-args" not in action
    assert "normalize-inputs.sh" in action
    assert (
        'run: bash .github/actions/setup-project/normalize-inputs.sh "$GITHUB_OUTPUT"'
        in action
    )
    assert "inputs.rust-components }}-${{ inputs.rust-targets" not in action


def test_workflow_shells_do_not_select_artifacts_with_ls() -> None:
    perf_demo = _read(".github/workflows/perf_demo.yml")
    release = _read(".github/workflows/release.yml")
    assert "latest=$(ls " not in perf_demo
    assert "WHEEL=$(ls " not in release
    assert "set -- dist/molt-*.whl" not in release
    assert release.count("release_authority select-one") == 4


def test_composite_action_shells_never_interpolate_inputs_directly() -> None:
    actions_root = REPO_ROOT / ".github" / "actions"
    checked = 0
    for action_path in sorted(actions_root.glob("*/action.y*ml")):
        payload = yaml.safe_load(action_path.read_text(encoding="utf-8"))
        for step in payload.get("runs", {}).get("steps", []):
            run = step.get("run")
            if not isinstance(run, str):
                continue
            checked += 1
            assert re.search(r"\$\{\{\s*inputs\.", run) is None, (
                action_path,
                step.get("name"),
            )
    assert checked >= 5


def test_release_candidate_admits_both_installed_guest_targets() -> None:
    steps = yaml.safe_load(_read(".github/workflows/release.yml"))["jobs"][
        "candidates"
    ]["steps"]
    setups = [
        step for step in steps if step.get("uses") == "./.github/actions/setup-project"
    ]
    assert len(setups) == 1
    setup = setups[0]["with"]
    rust = tomllib.loads(_read("rust-toolchain.toml"))["toolchain"]
    assert setup["node-version"] == "pinned"
    assert setup["rust-toolchain"] == "pinned"
    assert {part.strip() for part in setup["rust-targets"].split(",")} >= set(
        rust["targets"]
    )
    consumer_index = next(
        index
        for index, step in enumerate(steps)
        if "tools.release.verify_consumer" in str(step.get("run", ""))
    )
    assert steps.index(setups[0]) < consumer_index
    provision_index = next(
        index
        for index, step in enumerate(steps)
        if "tools.release.provision_execution_archives" in str(step.get("run", ""))
    )
    build_index = next(
        index
        for index, step in enumerate(steps)
        if "tools.release.build_compiler" in str(step.get("run", ""))
    )
    assert steps.index(setups[0]) < provision_index < build_index < consumer_index
    provision = steps[provision_index]
    consumer = steps[consumer_index]
    assert "if" not in provision and not provision.get("continue-on-error", False)
    assert '--target "${{ matrix.id }}"' in provision["run"]
    cache = (
        '--execution-archive-cache "${{ runner.temp }}/molt-release-execution-archives"'
    )
    assert cache in provision["run"] and cache in consumer["run"]
    assert "tools/guarded_exec.py --prefix MOLT_RELEASE --" in provision["run"]


@pytest.mark.parametrize(
    ("event", "selected", "expected"),
    [
        ("schedule", False, True),
        ("push", False, False),
        ("pull_request", False, False),
        ("workflow_dispatch", False, False),
        ("push", True, True),
        ("pull_request", True, True),
        ("workflow_dispatch", True, True),
    ],
)
def test_security_reusable_selection_truth_table(
    event: str, selected: bool, expected: bool
) -> None:
    assert (event == "schedule" or selected) is expected
    text = _read(".github/workflows/security_hardening.yml")
    assert "if: github.event_name == 'schedule' || inputs.python_security" in text
    assert "if: github.event_name == 'schedule' || inputs.rust_security" in text


@pytest.mark.parametrize(
    ("workflow", "job"),
    [
        ("ci.yml", "docs-gates"),
        ("ci.yml", "platform-portability"),
        ("ci.yml", "native-integration"),
        ("ci.yml", "rust-build-unit-smoke"),
        ("ci.yml", "llvm-backend"),
        ("molt-wasm-ci.yml", "wasm-build"),
        ("security_hardening.yml", "rust-security"),
    ],
)
def test_ci_rust_consumers_share_compiled_artifact_cache(
    workflow: str, job: str
) -> None:
    steps = yaml.safe_load(_read(f".github/workflows/{workflow}"))["jobs"][job]["steps"]
    setups = [
        step for step in steps if step.get("uses") == "./.github/actions/setup-project"
    ]
    assert len(setups) == 1
    assert setups[0]["with"]["rust-toolchain"]
    assert setups[0]["with"]["cache-cargo"] == "true"
    assert not any("rust-cache@" in step.get("uses", "") for step in steps)


@pytest.mark.parametrize(
    ("workflow", "job"),
    [("ci.yml", "rust-build-unit-smoke"), ("molt-wasm-ci.yml", "wasm-build")],
)
def test_ci_luau_runner_uses_digest_bound_prebuilt_authority(
    workflow: str, job: str
) -> None:
    text = _read(f".github/workflows/{workflow}")
    steps = yaml.safe_load(text)["jobs"][job]["steps"]
    provision = next(
        step
        for step in steps
        if step.get("name") == "Install pinned executable Luau proof runner"
    )
    assert provision["run"] == (
        'python3 -m molt.tool_releases provision lune --github-path "$GITHUB_PATH"'
    )
    assert "cargo install lune" not in text
    assert steps.index(provision) < next(
        index
        for index, step in enumerate(steps)
        if "--run-family" in step.get("run", "")
    )


def test_ci_push_path_is_cheap_only() -> None:
    ci_text = _read(".github/workflows/ci.yml")

    docs_gate = ci_text.split("  docs-gates:", 1)[1].split("\n  classify-changes:", 1)[
        0
    ]
    rustfmt_setup = docs_gate.index("uses: ./.github/actions/setup-project")
    repository_executor = docs_gate.index("Execute repository policy partitions")
    assert rustfmt_setup < repository_executor
    assert "rust-components: rustfmt, clippy" in docs_gate
    assert "rust-targets: wasm32-wasip1" in docs_gate

    assert "concurrency:" in ci_text
    assert "merge_group:" in ci_text
    assert "github.event_name == 'pull_request'" in ci_text
    assert "format('pr-{0}', github.event.pull_request.number)" in ci_text
    assert "cancel-in-progress: ${{ github.event_name == 'pull_request' }}" in (ci_text)
    assert "docs-gates:" in ci_text
    assert "classify-changes:" in ci_text
    assert "name: Changed Path Classifier" in ci_text
    # The frontend-Python ty type-check is a zero-diagnostic ratchet enforced in
    # CI (pre-commit is not run in Actions), mirroring the pre-commit `ty` hook.
    assert 'argv = ["uv", "run", "ty", "check", "src"]' in _read(
        "tools/proof_plan.toml"
    )
    # Proof spelling lives only in the manifest; docs-gates is executor mechanics.
    assert "--run-family repository_policy --receipt" in ci_text
    assert 'id = "repository.differential.layout"' in _read("tools/proof_plan.toml")
    assert "uv run python3 tools/check_differential_suite_layout.py" not in ci_text
    assert "python-static:" in ci_text
    assert "python-unit:" in ci_text
    assert "native-integration:" in ci_text
    assert "needs: classify-changes" in ci_text
    assert "if: needs.classify-changes.outputs.python_static == 'true'" in ci_text
    assert "rust-build-unit-smoke:" in ci_text
    assert "if: needs.classify-changes.outputs.rust == 'true'" in ci_text
    assert "llvm-backend:" in ci_text
    assert "if: needs.classify-changes.outputs.llvm == 'true'" in ci_text
    assert "      - docs-gates" in ci_text
    assert "differential-tests:" not in ci_text
    assert "benchmark:" not in ci_text
    assert "parity:" not in ci_text
    assert "runs-on: ubuntu-latest" in ci_text
    assert "runs-on: ${{ matrix.runner }}" in ci_text
    # Each matrix job consumes only its own family's classifier output.
    for name in ("platform_portability", "python_unit"):
        assert (
            f"matrix: ${{{{ fromJSON(needs.classify-changes.outputs.{name}_matrix) }}}}"
            in ci_text
        )
    assert "classify-changes.outputs.matrix" not in ci_text
    assert "Swatinem/rust-cache@" not in ci_text
    assert "uses: ./.github/actions/setup-project" in ci_text
    # setup-project plans every job's resources once; no workflow restates it.
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        assert "ci_resource_env.py" not in workflow.read_text(encoding="utf-8")
    setup_steps = yaml.safe_load(_read(".github/actions/setup-project/action.yml"))[
        "runs"
    ]["steps"]
    names = [step.get("name") for step in setup_steps]
    plan_step = setup_steps[names.index("Plan job resources")]
    assert plan_step["run"] == (
        'python3 tools/ci_resource_env.py --github-env "$GITHUB_ENV"'
    )
    assert names.index("Plan job resources") > names.index(
        "Provision and bind repository Python"
    )
    assert 'CARGO_BUILD_JOBS: "1"' not in ci_text
    assert 'sync: "true"' in ci_text
    assert 'sync-frozen: "true"' in ci_text
    assert "sync-groups: dev" in ci_text
    proof_plan_text = _read("tools/proof_plan.toml")
    assert '"-m", "not slow"' in proof_plan_text
    assert "native.integration.bench-cli" in proof_plan_text
    assert "native.integration.capability-manifest" in proof_plan_text
    assert (
        "tests/test_manifest_pipeline_e2e.py::test_molt_build_with_manifest"
        in proof_plan_text
    )
    assert "tests/test_bench_tool.py::test_bench_no_cpython_sets_null_baseline" not in (
        ci_text
    )
    assert (
        "tests/test_bench_tool.py::test_bench_runtime_timeout_marks_molt_not_ok"
        not in (ci_text)
    )
    assert "tests/test_bench_harness.py" in proof_plan_text
    assert "tests/test_bench_tool.py" in proof_plan_text
    assert "tests/test_ci_workflow_topology.py" in proof_plan_text
    assert "tests/test_harness_conformance.py" in proof_plan_text
    assert "tests/test_harness_layers.py" in proof_plan_text
    assert "tests/test_monty_conformance_runner.py" in proof_plan_text
    assert "Setup canonical native linker SDK" in ci_text
    assert "proof-receipts/evidence/cargo-test-truth.json" in ci_text
    assert "proof-receipts/evidence/llvm-differential-truth.json" in ci_text
    assert "target/**/.molt_state/build_failures/*.json" not in ci_text
    llvm_upload = ci_text.split("- name: Upload LLVM/MLIR/linker receipt", 1)[1].split(
        "- name: Summarize guarded command hotspots", 1
    )[0]
    assert "include-hidden-files: true" in llvm_upload
    assert "uses: ./.github/actions/setup-llvm" in ci_text
    assert "sudo apt-get install -y lld" not in ci_text
    assert "timeout_seconds" in proof_plan_text
    # Four jobs summarize hotspots: docs-gates, python-tooling-smoke,
    # rust-build-unit-smoke, and the LLVM backend job.
    assert ci_text.count("Summarize guarded command hotspots") == 4
    assert ci_text.count("python3 tools/profile_hotspots.py --limit 20") == 4


@pytest.mark.parametrize(
    ("workflow", "job", "upload_name"),
    [
        (".github/workflows/ci.yml", "llvm-backend", "Upload LLVM/MLIR/linker receipt"),
        (".github/workflows/molt-wasm-ci.yml", "wasm-build", "Upload WASM receipt"),
    ],
)
def test_ci_control_evidence_uses_shared_path_command(workflow, job, upload_name):
    steps = yaml.safe_load(_read(workflow))["jobs"][job]["steps"]
    resolver = next(step for step in steps if step.get("id") == "build-control")
    assert resolver["run"].strip() == (
        'uv run --no-sync python -m tools.build_control_path >> "$GITHUB_OUTPUT"'
    )
    upload = next(step for step in steps if step.get("name") == upload_name)
    paths = upload["with"]["path"].splitlines()
    assert "${{ steps.build-control.outputs.root }}/build_failures/*.json" in paths
    assert not any(".molt_state" in path for path in paths)
    if job == "wasm-build":
        assert (
            "${{ steps.build-control.outputs.root }}/runtime_wasm_generations/*.json"
            in paths
        )
    assert steps.index(resolver) < steps.index(upload)


@pytest.mark.parametrize("family", ["rust", "llvm"])
def test_ci_command_diagnostics_survive_missing_final_receipts(family: str) -> None:
    jobs = yaml.safe_load(_read(".github/workflows/ci.yml"))["jobs"]
    job = jobs["rust-build-unit-smoke" if family == "rust" else "llvm-backend"]
    steps = job["steps"]
    diagnostics = [
        step
        for step in steps
        if step.get("with", {}).get("name") == f"proof-diagnostics-{family}"
    ]
    assert len(diagnostics) == 1
    upload = diagnostics[0]
    assert upload["uses"].startswith("actions/upload-artifact@")
    assert upload["with"]["include-hidden-files"] is True
    assert upload["with"]["if-no-files-found"] == "warn"
    # A killed phase may leave only its manifest and hidden partial log.
    # Final receipt publication must not gate collection of those bytes.
    assert upload["if"] == "always()"
    executor = next(
        step
        for step in steps
        if f"--run-family {family} --receipt" in step.get("run", "")
    )
    assert steps.index(executor) < steps.index(upload)
    assert "continue-on-error" not in executor
    strict_download = next(
        step
        for step in jobs["proof-plan-verdict"]["steps"]
        if step.get("uses", "").startswith("actions/download-artifact@")
    )
    assert not fnmatchcase(upload["with"]["name"], strict_download["with"]["pattern"])
    paths = upload["with"]["path"].splitlines()
    if family == "rust":
        assert paths == [
            "proof-receipts/evidence/cargo-test-truth-runs/",
            "proof-receipts/evidence/runtime-gate-runs/",
            "proof-receipts/evidence/runtime-extension-admission/",
            "${{ env.CARGO_TARGET_DIR != '' && format('{0}/**/molt-test-artifacts/**/*.log', env.CARGO_TARGET_DIR) || '' }}",
        ]
    else:
        assert job["env"]["MOLT_DEBUG_ARTIFACT_DIR"] == (
            "${{ github.workspace }}/proof-receipts/evidence/llvm-backend-debug"
        )
        assert paths == [
            "proof-receipts/llvm.json",
            "proof-receipts/evidence/llvm-differential-truth.json",
            "proof-receipts/evidence/llvm-backend-debug/native-batch-failures/",
            "${{ steps.build-control.outputs.root != '' && format('{0}/backend_daemon/*.log', steps.build-control.outputs.root) || '' }}",
            "${{ steps.build-control.outputs.root != '' && format('{0}/backend_daemon/*.log.old', steps.build-control.outputs.root) || '' }}",
            "${{ steps.build-control.outputs.profile_log }}",
            "${{ steps.build-control.outputs.profile_log != '' && format('{0}.1', steps.build-control.outputs.profile_log) || '' }}",
        ]


def _diagnostic_path_expression(line: str, values: dict[str, str]) -> str:
    """Model only the upload's declared GitHub path-expression subset."""
    conditional = re.fullmatch(
        r"\$\{\{ ([\w.-]+) != '' && format\('([^']+)', \1\) \|\| '' \}\}",
        line,
    )
    if conditional:
        value = values[conditional[1]]
        return conditional[2].format(value) if value else ""
    return re.sub(r"\$\{\{ ([\w.-]+) \}\}", lambda match: values[match[1]], line)


@pytest.mark.parametrize(
    ("family", "os_name"),
    [("platform_portability", os_name) for os_name in ("linux", "macos", "windows")]
    + [("native_integration", "linux"), ("wasm", "linux")],
)
@pytest.mark.parametrize(
    "state",
    [
        "failed-with-receipt",
        "failed-without-receipt",
        "setup-no-output",
        "setup-no-root",
    ],
)
def test_ci_diagnostics_retain_current_custody_without_final_receipt(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    family: str,
    os_name: str,
    state: str,
) -> None:
    from molt import dx
    from tools.build_control_path import build_control_output
    from tests.test_dx_run_context import _github_actions_custody_env

    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    native = family == "native_integration"
    wasm = family == "wasm"
    detailed = native or wasm
    cell = ""
    if not detailed:
        cells = [
            cell
            for cell in plan["matrix_cell"]
            if cell["backend"] == "proof-queue" and cell["os"] == os_name
        ]
        assert len(cells) == 1
        cell = cells[0]["id"]
    jobs = yaml.safe_load(_read(".github/workflows/ci.yml"))["jobs"]
    job = (
        yaml.safe_load(_read(".github/workflows/molt-wasm-ci.yml"))["jobs"][
            "wasm-build"
        ]
        if wasm
        else jobs[family.replace("_", "-")]
    )
    if detailed:
        assert job["env"]["MOLT_GUARD_PROFILE"] == "all"
    suffix = (
        "wasm"
        if wasm
        else "native-integration"
        if native
        else "platform-portability-${{ matrix.cell }}"
    )
    steps = job["steps"]
    upload = next(
        step
        for step in steps
        if step.get("with", {}).get("name") == f"proof-diagnostics-{suffix}"
    )
    assert upload["if"] == "always()", "missing final receipts cannot gate diagnostics"
    assert upload["uses"].startswith("actions/upload-artifact@")
    assert upload["with"]["include-hidden-files"] is True
    assert upload["with"]["if-no-files-found"] == "warn"
    resolver = next(step for step in steps if step.get("id") == "build-control")
    assert resolver["shell"] == "bash"
    assert (
        resolver["run"].strip()
        == 'uv run --no-sync python -m tools.build_control_path >> "$GITHUB_OUTPUT"'
    )
    executor = next(
        step for step in steps if f"--run-family {family}" in step.get("run", "")
    )
    setup = next(
        step for step in steps if step.get("uses") == "./.github/actions/setup-project"
    )
    assert (
        steps.index(setup)
        < steps.index(resolver)
        < steps.index(executor)
        < steps.index(upload)
    )
    assert "continue-on-error" not in executor
    receipt_upload = next(
        step
        for step in steps
        if step.get("with", {}).get("name") == f"proof-receipt-{suffix}"
    )
    assert receipt_upload["if"] == (
        f"always() && hashFiles('proof-receipts/{family}.json') != ''"
        if detailed
        else "always() && hashFiles(format('proof-receipts/platform-portability-{0}.json', matrix.cell)) != ''"
    )
    assert receipt_upload["with"]["if-no-files-found"] == "error"
    strict_download = next(
        step
        for step in jobs["proof-plan-verdict"]["steps"]
        if step.get("uses", "").startswith("actions/download-artifact@")
    )

    workspace = tmp_path / "runner-work" / "molt" / "molt"
    workspace.mkdir(parents=True)
    runner_temp = tmp_path / "runner-temp"
    environment = _github_actions_custody_env(workspace, runner_temp)
    custody = runner_temp / "mp-12345-2"
    environment[dx.GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV] = str(custody)
    environment["GITHUB_JOB"] = "wasm-build" if wasm else family.replace("_", "-")
    # Only this synthetic workspace has synthetic Git identity. Concurrent
    # custody observers retain real checkout observation for their own roots.
    checkout_head = dx._git_checkout_head
    monkeypatch.setattr(
        dx,
        "_git_checkout_head",
        lambda root: (
            environment["GITHUB_SHA"] if root == workspace else checkout_head(root)
        ),
    )
    outputs = dict(
        line.split("=", 1)
        for line in build_control_output(environment, repo_root=workspace).splitlines()
    )
    profile = custody / "tmp" / "harness_memory_guard" / "commands.jsonl"
    pytest_dir = custody / "tmp" / "pytest-memory-guard"
    assert outputs["profile_log"] == str(profile)
    assert outputs["guard_state_root"] == str(custody / "tmp" / "memory_guard")
    assert outputs["pytest_guard_root"] == str(pytest_dir)
    values = {
        "matrix.cell": cell,
        "env.CARGO_TARGET_DIR": str(custody / "target")
        if state.startswith("failed-")
        else "",
        "env.MOLT_CI_EPHEMERAL_CUSTODY_ROOT": ""
        if state == "setup-no-root"
        else str(custody),
        **{
            f"steps.build-control.outputs.{name}": value
            if state.startswith("failed-")
            else ""
            for name, value in outputs.items()
        },
    }
    name = _diagnostic_path_expression(upload["with"]["name"], values)
    assert not fnmatchcase(name, strict_download["with"]["pattern"])
    paths = upload["with"]["path"].splitlines()
    common_profiles = [
        "${{ steps.build-control.outputs.profile_log }}",
        "${{ steps.build-control.outputs.profile_log != '' && format('{0}.1', steps.build-control.outputs.profile_log) || '' }}",
    ]
    assert paths == (
        [
            f"proof-receipts/{family}.json",
            *common_profiles,
            "${{ steps.build-control.outputs.guard_state_root != '' && format('{0}/commands/**/*.json', steps.build-control.outputs.guard_state_root) || '' }}",
            "${{ steps.build-control.outputs.pytest_guard_root != '' && format('{0}/**/*.json', steps.build-control.outputs.pytest_guard_root) || '' }}",
        ]
        if detailed
        else [
            "proof-receipts/platform-portability-${{ matrix.cell }}.json",
            "proof-receipts/evidence/runtime-gate-runs/",
            "${{ env.CARGO_TARGET_DIR != '' && format('{0}/**/molt-test-artifacts/**/*.log', env.CARGO_TARGET_DIR) || '' }}",
            *common_profiles,
            "${{ env.MOLT_CI_EPHEMERAL_CUSTODY_ROOT != '' && format('{0}/tmp/pytest-memory-guard/**/*.json', env.MOLT_CI_EPHEMERAL_CUSTODY_ROOT) || '' }}",
        ]
    )
    rendered = [_diagnostic_path_expression(line, values) for line in paths]
    if not state.startswith("failed-"):
        assert all(
            not _diagnostic_path_expression(line, values) for line in common_profiles
        )
        if detailed or state == "setup-no-root":
            assert not any(
                _diagnostic_path_expression(line, values)
                for line in paths
                if line.startswith("${{")
            )

    expected: set[Path] = set()
    inner = {
        "prefix": "MOLT_PERF_CALIBRATION",
        "returncode": 125,
        "temporary_artifacts": {"closure": {"closed": False}},
        "termination_reports": [{"reason": "tracked_orphan_cleanup"}],
    }
    retained_files = [profile, profile.with_name(profile.name + ".1")]
    retained_files += [
        pytest_dir / name
        for name in (
            "pytest-1_outer-guard.json",
            "test-custody-2.json",
            ".workers/gw0_current-test.json",
        )
    ]
    if detailed:
        retained_files += [
            custody / "tmp/memory_guard/commands/current" / name
            for name in ("custody.json", "startup.json", "guard.json")
        ]
    else:
        retained_files += [
            workspace / "proof-receipts/evidence/runtime-gate-runs/run/aggregate.json",
            workspace
            / "proof-receipts/evidence/runtime-gate-runs/run/child-0/raw.stderr.log",
            custody / "target/release-output/deps/molt-test-artifacts/run/stderr.log",
        ]
    for retained in retained_files:
        # Same filenames under a foreign run must never be swept into upload.
        owner = custody if retained.is_relative_to(custody) else workspace
        foreign = runner_temp / "mp-99999-1" / retained.relative_to(owner)
        foreign.parent.mkdir(parents=True, exist_ok=True)
        foreign.write_text('{"foreign":true}', encoding="utf-8")
        if state.startswith("failed-"):
            retained.parent.mkdir(parents=True, exist_ok=True)
            retained.write_text(json.dumps(inner) + "\n", encoding="utf-8")
            expected.add(retained)
    if state.startswith("failed-"):
        (pytest_dir / "unrelated.txt").write_text("not guard JSON", encoding="utf-8")
    if state == "failed-with-receipt":
        receipt = workspace / rendered[0]
        receipt.parent.mkdir(parents=True, exist_ok=True)
        receipt.write_text('{"status":"failed"}', encoding="utf-8")
        expected.add(receipt)
    retained_paths: set[Path] = set()
    for path in filter(None, rendered):
        candidate = Path(path)
        if not candidate.is_absolute():
            candidate = workspace / candidate
        assert (
            candidate.is_relative_to(custody)
            or candidate == workspace / rendered[0]
            or candidate == workspace / "proof-receipts/evidence/runtime-gate-runs"
        )
        for found in glob.glob(str(candidate), recursive=True, include_hidden=True):
            path = Path(found)
            if path.is_dir():
                retained_paths.update(
                    child for child in path.rglob("*") if child.is_file()
                )
            else:
                retained_paths.add(path)
    assert retained_paths == expected
    if state.startswith("failed-"):
        assert json.loads(profile.read_text(encoding="utf-8")) == inner
        assert profile.with_name(profile.name + ".1") in retained_paths


def test_ci_heavy_jobs_are_path_classified() -> None:
    ci_text = _read(".github/workflows/ci.yml")

    assert 'python3 tools/proof_plan.py --github-output "$GITHUB_OUTPUT"' in (ci_text)
    assert "python_static: ${{ steps.paths.outputs.python_static }}" in ci_text
    assert "python_unit: ${{ steps.paths.outputs.python_unit }}" in ci_text
    assert (
        "native_integration: ${{ steps.paths.outputs.native_integration }}" in ci_text
    )
    assert "rust: ${{ steps.paths.outputs.rust }}" in ci_text
    assert "llvm: ${{ steps.paths.outputs.llvm }}" in ci_text
    assert "python_security: ${{ steps.paths.outputs.python_security }}" in ci_text
    assert "rust_security: ${{ steps.paths.outputs.rust_security }}" in ci_text
    assert (
        "platform_portability: ${{ steps.paths.outputs.platform_portability }}"
        in ci_text
    )
    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    for family in plan["ci_family"]:
        if family["executor"] == "github-matrix":
            output = f"{family['name']}_matrix"
            assert f"{output}: ${{{{ steps.paths.outputs.{output} }}}}" in ci_text
    assert "steps.paths.outputs.matrix }}" not in ci_text
    assert "topology: ${{ steps.paths.outputs.topology }}" in ci_text
    assert "selected: ${{ steps.paths.outputs.selected }}" in ci_text
    assert ci_text.count("needs: classify-changes") >= 4
    assert "proof-plan-verdict:" in ci_text
    assert "name: Proof Plan Verdict" in ci_text
    assert (
        "--verify-selected '${{ needs.classify-changes.outputs.selected }}'" in ci_text
    )
    assert "--receipt-dir proof-receipts" in ci_text
    assert "uses: actions/download-artifact@" in ci_text
    assert "== 'success' && 1 || 0" not in ci_text
    # Full history exactly where a job reads it: the path classifier diffs
    # against the event base, and docs-gates runs the commit-attribution
    # policy over every commit the event introduces.
    jobs = yaml.safe_load(ci_text)["jobs"]
    full_history = {
        name
        for name, job in jobs.items()
        for step in job.get("steps", [])
        if step.get("uses", "").startswith("actions/checkout@")
        and step.get("with", {}).get("fetch-depth") == 0
    }
    assert full_history == {"classify-changes", "docs-gates"}
    assert ci_text.count("fetch-depth: 0") == 2
    assert "--depth=1" not in ci_text


def test_ci_proof_families_are_admitted_independently() -> None:
    """A selected sibling failure must never mask another proof family."""

    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    ci_jobs = yaml.safe_load(_read(".github/workflows/ci.yml"))["jobs"]
    families = plan["ci_family"]

    assert all(family["dependencies"] == [] for family in families)
    admission_jobs = {family["admission_job"] for family in families}
    assert admission_jobs == {
        "docs-gates",
        "formal-verification",
        "llvm-backend",
        "native-integration",
        "platform-portability",
        "python-static",
        "python-unit",
        "rust-build-unit-smoke",
        "security-hardening",
        "wasm-validation",
    }

    for family in families:
        assert family["admission_workflow"] == ".github/workflows/ci.yml"
        job_name = family["admission_job"]
        assert _literal_job_needs(ci_jobs[job_name]) == tuple(family["admission_needs"])
        assert "continue-on-error" not in ci_jobs[job_name]

    assert _literal_job_needs(ci_jobs["docs-gates"]) == ()
    for job_name in admission_jobs - {"docs-gates"}:
        assert _literal_job_needs(ci_jobs[job_name]) == ("classify-changes",)
        condition = str(ci_jobs[job_name].get("if", ""))
        assert ".result" not in condition

    # Simulate every family admission failing in turn. Every other selected
    # admission still has all of its own prerequisites satisfied because no
    # proof admission consumes a sibling result.
    for failed_job in admission_jobs:
        statuses = {"classify-changes": "success", failed_job: "failure"}
        for candidate_job in admission_jobs - {failed_job}:
            assert all(
                statuses.get(dependency) == "success"
                for dependency in _literal_job_needs(ci_jobs[candidate_job])
            )

    verdict_needs = _literal_job_needs(ci_jobs["proof-plan-verdict"])
    assert set(verdict_needs) == {"classify-changes", *admission_jobs}
    assert len(verdict_needs) == len(set(verdict_needs))
    assert ci_jobs["proof-plan-verdict"]["if"] == "always()"


def test_proof_plan_validation_rejects_cross_family_admission_masking(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from tools import proof_plan

    plan = proof_plan.ProofPlan.load()
    original = proof_plan._workflow_job_needs

    def masked_needs(block: str) -> tuple[str, ...]:
        if block.startswith("  wasm-validation:"):
            return ("classify-changes", "rust-build-unit-smoke")
        return original(block)

    monkeypatch.setattr(proof_plan, "_workflow_job_needs", masked_needs)
    validation_errors = plan.validate()
    assert any(
        "wasm: admission job 'wasm-validation' needs " in error
        and "rust-build-unit-smoke" in error
        for error in validation_errors
    )


def test_llvm_ci_resolves_toolchain_from_manifest_authority() -> None:
    ci_text = _read(".github/workflows/ci.yml")
    perf_text = _read(".github/workflows/perf-gate.yml")
    wasm_text = _read(".github/workflows/molt-wasm-ci.yml")
    action_text = _read(".github/actions/setup-llvm/action.yml")
    action = yaml.safe_load(action_text)
    action_steps = action["runs"]["steps"]
    steps = {step["name"]: step for step in action_steps}

    assert "uses: ./.github/actions/setup-llvm" in ci_text
    assert "uses: ./.github/actions/setup-llvm" in perf_text
    assert "PYTHONPATH=src python -m molt.llvm_toolchain" in action_text
    assert "python3" not in action_text
    assert '--github-output "$GITHUB_OUTPUT"' in action_text
    assert '--github-env "$GITHUB_ENV"' in action_text
    assert "steps.contract.outputs.apt_packages" in action_text
    assert "steps.contract.outputs.apt_installer_url" in action_text
    assert "steps.contract.outputs.apt_installer_sha256" in action_text
    assert 'installer="$RUNNER_TEMP/molt-llvm-apt.sh"' in action_text
    assert "sha256sum --check --strict" in action_text
    assert "wget -qO /tmp" not in action_text
    assert "the wasm SDK profile requires wasi=true" in action_text
    # Host packages serve only the Linux full SDK. Every WebAssembly tool comes
    # from the one manifest-owned wasi-sdk provisioner on every host.
    apt_step = steps["Provision full LLVM SDK packages"]
    assert apt_step["if"] == "inputs.profile == 'full'"
    assert 'if [ "$RUNNER_OS" != "Linux" ]' in apt_step["run"]
    assert sum("apt-get" in str(step.get("run", "")) for step in action_steps) == 1
    assert "lld-$LLVM_MAJOR" not in action_text
    cache = steps["Restore verified wasi-sdk archive"]
    assert cache["if"] == "inputs.wasi == 'true'"
    assert re.fullmatch(r"actions/cache@[0-9a-f]{40}", cache["uses"])
    assert cache["with"] == {
        "path": "${{ runner.temp }}/molt-wasi-sdk-downloads",
        "key": "${{ steps.contract.outputs.wasi_sdk_cache_key }}",
    }
    provision = steps["Provision pinned host wasi-sdk"]
    assert provision["if"] == "inputs.wasi == 'true'"
    assert provision["id"] == "wasi-sdk"
    assert "PYTHONPATH=src python -m tools.provision_wasi_sdk" in provision["run"]
    assert '--downloads "$RUNNER_TEMP/molt-wasi-sdk-downloads"' in provision["run"]
    assert '--github-output "$GITHUB_OUTPUT"' in provision["run"]
    for shell_install in ("curl", "tar ", "sha256sum", "stat "):
        assert shell_install not in provision["run"]
    verify_wasm = steps["Verify and project WebAssembly SDK identity"]
    assert verify_wasm["env"] == {
        "WASI_SDK_INSTALL": "${{ steps.wasi-sdk.outputs.install }}"
    }
    assert "--verify-wasm" in verify_wasm["run"]
    assert '--wasi-sdk "$WASI_SDK_INSTALL"' in verify_wasm["run"]
    assert "--verify" in steps["Verify and project full SDK identity"]["run"]
    step_names = list(steps)
    full_verify = step_names.index("Verify and project full SDK identity")
    assert full_verify < step_names.index("Verify and project WebAssembly SDK identity")
    assert action["outputs"]["wasi_sdk"]["value"] == (
        "${{ steps.wasi-sdk.outputs.install }}"
    )
    assert action["outputs"]["wasi_sysroot"]["value"] == (
        "${{ steps.wasi-sdk.outputs.sysroot }}"
    )
    assert "--wasi-sysroot" not in action_text
    assert "wasi_sysroot_url" not in action_text
    # These command families declare both native ld.lld and SDK wasm-ld. The
    # WebAssembly-only SDK must never stand in for the full native LLVM SDK.
    jobs = yaml.safe_load(ci_text)["jobs"]
    for job in ("rust-build-unit-smoke", "native-integration"):
        sdk_steps = [
            step
            for step in jobs[job]["steps"]
            if step.get("uses") == "./.github/actions/setup-llvm"
        ]
        assert len(sdk_steps) == 1
        assert sdk_steps[0]["with"] == {"profile": "full", "wasi": "true"}
    wasm_steps = yaml.safe_load(wasm_text)["jobs"]["wasm-build"]["steps"]
    llvm_steps = [
        step
        for step in wasm_steps
        if step.get("uses") == "./.github/actions/setup-llvm"
    ]
    assert len(llvm_steps) == 1
    assert llvm_steps[0]["with"] == {"profile": "full", "wasi": "true"}
    assert all("wasi-libc" not in str(step.get("run", "")) for step in wasm_steps)
    release_steps = yaml.safe_load(_read(".github/workflows/release.yml"))["jobs"][
        "candidates"
    ]["steps"]
    release_llvm = [
        index
        for index, step in enumerate(release_steps)
        if step.get("uses") == "./.github/actions/setup-llvm"
    ]
    assert [release_steps[index]["with"] for index in release_llvm] == [
        {"profile": "wasm", "wasi": "true"}
    ]
    identity_index = next(
        index
        for index, step in enumerate(release_steps)
        if step.get("name") == "Verify candidate toolchain identity"
    )
    assert release_llvm[0] < identity_index
    assert '"$MOLT_WASM_LD" --version' in release_steps[identity_index]["run"]
    assert "grep -oE" not in ci_text
    assert "grep -oE" not in perf_text
    assert "LLVM_SYS_${MAJOR}1_PREFIX" not in ci_text
    assert "LLVM_SYS_${MAJOR}1_PREFIX" not in perf_text


def test_wasm_ci_provisions_pinned_optimizer_before_linked_partitions() -> None:
    workflow = yaml.safe_load(_read(".github/workflows/molt-wasm-ci.yml"))
    steps = workflow["jobs"]["wasm-build"]["steps"]
    optimizer_steps = [
        (index, step)
        for index, step in enumerate(steps)
        if step.get("uses") == "./.github/actions/setup-binaryen"
    ]
    assert len(optimizer_steps) == 1
    optimizer_index, optimizer = optimizer_steps[0]
    assert optimizer["id"] == "binaryen"
    partition_index, partition = next(
        (index, step)
        for index, step in enumerate(steps)
        if "--run-family wasm" in step.get("run", "")
    )
    assert optimizer_index < partition_index
    assert partition["env"]["MOLT_WASM_OPT"] == (
        "${{ steps.binaryen.outputs.wasm_opt }}"
    )
    action = yaml.safe_load(_read(".github/actions/setup-binaryen/action.yml"))
    assert action["outputs"]["wasm_opt"]["value"] == (
        "${{ steps.binaryen.outputs.wasm_opt }}"
    )
    provision = next(
        step for step in action["runs"]["steps"] if step.get("id") == "binaryen"
    )
    assert "python -m tools.provision_binaryen" in provision["run"]
    assert '--github-output "$GITHUB_OUTPUT"' in provision["run"]


def test_pr_trust_labeler_is_advisory_not_authoritative() -> None:
    labeler_text = _read(".github/workflows/pr_trust_labeler.yml")
    gate_text = _read(".github/workflows/pr_trust_gate.yml")

    assert "Trust gate remains authoritative" in labeler_text
    assert "error?.status === 403" in labeler_text
    assert "core.warning" in labeler_text
    assert "github.rest.issues.addLabels" in labeler_text
    assert "core.setFailed" not in labeler_text
    assert "core.setFailed" in gate_text


def test_kani_incompatibility_is_an_explicit_advisory_receipt_not_a_proof() -> None:
    kani_text = _read(".github/workflows/kani.yml")

    assert "name: Kani Advisory Compatibility Probe" in kani_text
    assert "name: Kani advisory compatibility probe" in kani_text
    assert '"schema": "molt.kani-compatibility.v1"' in kani_text
    assert '"authoritative": False' in kani_text
    assert '"required": False' in kani_text
    assert '"proofs_executed": 0' in kani_text
    assert '"status": "compatible" if compatible else "unavailable"' in kani_text
    assert "this advisory probe does not claim bounded verification" in kani_text
    assert "cargo kani --tests" not in kani_text
    assert not any(
        "bounded verification" in line.lower()
        for line in kani_text.splitlines()
        if line.lstrip().startswith("name:")
    )


def test_kani_workflow_is_scheduled_manual_and_standalone() -> None:
    kani_text = _read(".github/workflows/kani.yml")

    assert "workflow_dispatch:" in kani_text
    assert "schedule:" in kani_text
    assert "push:" not in kani_text
    assert "pull_request:" not in kani_text
    assert "classify-changes:" not in kani_text
    assert "proof_plan.py" not in kani_text


def test_kani_workflow_gates_verifier_rust_version_honestly() -> None:
    kani_text = _read(".github/workflows/kani.yml")

    assert "Check Kani toolchain compatibility" in kani_text
    assert "id: kani-toolchain" not in kani_text
    assert 'workspace_manifest["workspace"]["package"]["rust-version"]' in kani_text
    assert 'runtime" / "molt-obj-model" / "Cargo.toml"' in kani_text
    assert 'runtime" / "molt-runtime" / "Cargo.toml"' in kani_text
    assert "compatible = version_key(kani_rustc) >= version_key(required)" in kani_text
    assert "this advisory probe does not claim bounded verification" in kani_text
    assert "outputs.compatible" not in kani_text
    assert "GITHUB_OUTPUT" not in kani_text
    assert "Report skipped Kani proofs" not in kani_text
    assert "molt.kani-compatibility.v1" in kani_text
    assert "proofs_executed" in kani_text
    assert "if-no-files-found: error" in kani_text
    assert "--ignore-rust-version" not in kani_text


def test_github_workflows_opt_into_node24_action_runtime() -> None:
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        text = workflow.read_text(encoding="utf-8")
        if "uses:" not in text:
            continue

        assert 'FORCE_JAVASCRIPT_ACTIONS_TO_NODE24: "true"' in text, workflow
        assert "ACTIONS_ALLOW_USE_UNSECURE_NODE_VERSION" not in text, workflow


def test_github_workflows_do_not_reintroduce_node20_action_pins() -> None:
    node20_action_pins = {
        "actions/checkout@v4",
        "actions/checkout@v5",
        "actions/setup-python@v5",
        "actions/setup-node@v4",
        "actions/cache@v4",
        "actions/upload-artifact@v4",
        "actions/download-artifact@v4",
        "actions/github-script@v7",
        "actions/attest-build-provenance@v2",
        "astral-sh/setup-uv@v3",
        "astral-sh/setup-uv@v4",
        "astral-sh/setup-uv@v7",
        "astral-sh/setup-uv@v8.1.0",
        "softprops/action-gh-release@v2",
    }

    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        text = workflow.read_text(encoding="utf-8")
        for action_pin in sorted(node20_action_pins):
            assert action_pin not in text, (workflow, action_pin)


def test_github_workflows_pin_every_external_action_to_full_sha() -> None:
    action_files = [
        *WORKFLOW_ROOT.glob("*.yml"),
        *REPO_ROOT.glob(".github/actions/*/action.yml"),
    ]
    uses_pattern = re.compile(r"^\s*(?:-\s*)?uses:\s*([^\s#]+)", re.MULTILINE)
    # owner/repo[/path]@sha: subpath actions such as actions/cache/restore.
    sha_pattern = re.compile(r"^[^/@]+/[^/@]+(?:/[^/@]+)*@[0-9a-f]{40}$", re.IGNORECASE)
    found = 0
    for workflow in sorted(action_files):
        text = workflow.read_text(encoding="utf-8")
        for target in uses_pattern.findall(text):
            if target.startswith("./") or target.startswith("docker://"):
                continue
            found += 1
            assert sha_pattern.fullmatch(target), (workflow, target)
    assert found > 0


def test_github_actions_pin_one_exact_release_per_action() -> None:
    """Each external action resolves to one commit and one exact release tag.

    Dependabot rewrites every ``uses:`` line of an action together; a second
    digest or a floating ``# v7`` comment means a partial or hand-made bump.
    """
    action_files = [
        *WORKFLOW_ROOT.glob("*.yml"),
        *REPO_ROOT.glob(".github/actions/*/action.yml"),
    ]
    uses_pattern = re.compile(
        r"^\s*(?:-\s*)?uses:\s*([^\s#@]+)@([0-9a-f]{40})(?:\s*#\s*(\S+))?\s*$",
        re.MULTILINE,
    )
    release = re.compile(r"^v\d+\.\d+\.\d+$")
    pins: dict[str, set[tuple[str, str | None]]] = {}
    for path in sorted(action_files):
        for action, sha, comment in uses_pattern.findall(path.read_text("utf-8")):
            repository = "/".join(action.split("/")[:2])
            pins.setdefault(repository, set()).add((sha, comment or None))
            assert comment and release.fullmatch(comment), (path, action, comment)
    assert pins
    divergent = {repo: sorted(found) for repo, found in pins.items() if len(found) != 1}
    assert divergent == {}


def test_setup_uv_installs_the_plan_pinned_uv() -> None:
    policy = {
        entry["name"]: entry
        for entry in tomllib.loads(_read("tools/proof_plan.toml"))["toolchain_policy"]
    }
    setup_project = _read(".github/actions/setup-project/action.yml")
    assert setup_project.count("astral-sh/setup-uv@") == 1
    assert f'version: "{policy["uv"]["setup_value"]}"' in setup_project
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        assert "astral-sh/setup-uv@" not in workflow.read_text("utf-8"), workflow


def test_executable_receipt_root_is_git_ignored() -> None:
    ignored = {
        line.strip()
        for line in _read(".gitignore").splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    }
    assert "proof-receipts/" in ignored
    for relative in (
        ".github/workflows/ci.yml",
        ".github/workflows/formal.yml",
        ".github/workflows/molt-wasm-ci.yml",
        ".github/workflows/security_hardening.yml",
    ):
        for line in _read(relative).splitlines():
            if "--receipt " in line or "--receipt-dir " in line:
                assert "proof-receipts" in line, (relative, line)


def test_executable_proof_workflows_contain_only_executor_mechanics() -> None:
    allowed_tools = {
        "tools/bootstrap_browser_asset_graph.py",
        "tools/ci_resource_env.py",
        "tools/guarded_exec.py",
        "tools/profile_hotspots.py",
        "tools/proof_plan.py",
    }
    for relative in (
        ".github/workflows/ci.yml",
        ".github/workflows/formal.yml",
        ".github/workflows/molt-wasm-ci.yml",
        ".github/workflows/security_hardening.yml",
    ):
        for line_number, line in enumerate(_read(relative).splitlines(), start=1):
            # Setup/cache inputs may legitimately live under tools/ without
            # executing repository policy. Only Python tool invocations are
            # proof-authority candidates that must route through the plan.
            if "tools/" not in line or ".py" not in line:
                continue
            assert any(tool in line for tool in allowed_tools), (
                relative,
                line_number,
                line,
            )


def test_github_workflows_keep_cargo_target_dirs_cache_stable() -> None:
    unstable_tokens = ("${{ github.run_id }}", "${{ github.run_attempt }}")
    offenders: list[str] = []
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        for line_no, line in enumerate(
            workflow.read_text(encoding="utf-8").splitlines(), start=1
        ):
            if "CARGO_TARGET_DIR" not in line:
                continue
            if any(token in line for token in unstable_tokens):
                rel = workflow.relative_to(REPO_ROOT).as_posix()
                offenders.append(f"{rel}:{line_no}: {line.strip()}")

    assert offenders == []


def test_rust_security_reuses_cached_tool_builds() -> None:
    workflow_text = _read(".github/workflows/security_hardening.yml")
    rust_security = workflow_text.split("  rust-security:", 1)[1]

    assert (
        "MOLT_SESSION_ID: rust-security-${{ github.run_id }}-${{ github.run_attempt }}"
        in rust_security
    )
    assert "uses: ./.github/actions/setup-project" in rust_security
    assert "rust-toolchain: pinned" in rust_security
    assert 'cache-cargo: "true"' in rust_security
    setup_project = _read(".github/actions/setup-project/action.yml")
    assert "Restore Cargo dependency cache" in setup_project
    assert "${{ env.MOLT_CARGO_CACHE_TARGET }}" in setup_project
    assert "cargo install cargo-deny --version 0.20.2 --locked" in rust_security
    assert "cargo install cargo-audit --version 0.22.2 --locked" in rust_security
    assert "rm -rf" not in rust_security
    assert "tmp/security/cargo-audit-advisory-db" in _read("tools/proof_plan.toml")


def test_platform_portability_is_one_generated_cross_os_authority() -> None:
    ci_text = _read(".github/workflows/ci.yml")
    plan = tomllib.loads(_read("tools/proof_plan.toml"))

    assert not (WORKFLOW_ROOT / "proof-queue-portability.yml").exists()
    family = next(
        family
        for family in plan["ci_family"]
        if family["name"] == "platform_portability"
    )
    assert family["executor"] == "github-matrix"
    assert family["workflow"] == ".github/workflows/ci.yml"
    assert family["job"] == "platform-portability"
    assert "fail-fast: false" in ci_text
    assert "runs-on: ${{ matrix.runner }}" in ci_text
    setup_action = yaml.safe_load(_read(".github/actions/setup-project/action.yml"))
    # Every job that sets up the project gets custody outside the checkout:
    # runtime fixtures and guard scratch never land in the source tree.
    first = setup_action["runs"]["steps"][0]
    assert first["name"] == "Configure verified ephemeral custody"
    custody = first["run"]
    assert "MOLT_CI_EPHEMERAL_CUSTODY_ROOT=$custodyRoot" in custody
    assert "UV_PROJECT_ENVIRONMENT=$(Join-Path $custodyRoot 'venv')" in custody
    assert "$env:RUNNER_TEMP" in custody
    assert "$env:RUNNER_TEMP" in ci_text
    assert not (REPO_ROOT / ".github/actions/ephemeral-custody").exists()
    assert "${{ runner.temp }}" not in ci_text
    assert "--run-family platform_portability --receipt" in ci_text
    assert '--matrix-cell "${{ matrix.cell }}"' in ci_text
    assert "uv run --active --project . --no-sync python -m pytest" not in ci_text

    cells = {
        cell["id"]: cell
        for cell in plan["matrix_cell"]
        if cell["backend"] == "proof-queue"
    }
    assert {(cell["os"], cell["runner"]) for cell in cells.values()} == {
        ("linux", "ubuntu-latest"),
        ("macos", "macos-14"),
        ("windows", "windows-2022"),
    }
    commands = [
        command
        for command in plan["command"]
        if command["family"] == "platform_portability"
    ]
    # Besides the three queue cells, the family carries two Rust cells: the
    # macOS cell (the primary host's workspace clippy and runtime gate) and the
    # aarch64 Linux cell (workspace clippy where C char is unsigned).
    rust_cells = {
        cell["id"]: cell
        for cell in plan["matrix_cell"]
        if cell["id"] in {command["cell"] for command in commands}
        and cell["backend"] == "rust"
    }
    assert {
        (cell["os"], cell["arch"], cell["runner"]) for cell in rust_cells.values()
    } == {("linux", "aarch64", "ubuntu-24.04-arm"), ("macos", "aarch64", "macos-14")}
    assert {command["cell"] for command in commands} == set(cells) | set(rust_cells)
    commands = [command for command in commands if command["cell"] in cells]
    queue_commands = [command for command in commands if ".queue." in command["id"]]
    ir_commands = [command for command in commands if ".ir." in command["id"]]
    assert len({tuple(command["argv"]) for command in queue_commands}) == 1
    assert {command["cell"] for command in ir_commands} == {
        "macos-arm64-py312-queue-portability",
        "windows-x86_64-py312-queue-portability",
    }


def test_shell_completion_qualification_requires_actual_shells_in_existing_linux_job():
    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    command = next(
        row for row in plan["command"] if row["id"] == "portability.completion.linux"
    )
    assert command["family"] == "platform_portability"
    assert command["cell"] == "linux-x86_64-py312-queue-portability"
    assert set(command["tiers"]) == {"pr", "main"}
    assert {"bash", "fish", "zsh"} <= set(command["toolchains"])
    assert command["argv"][-1] == (
        "tests/cli/test_cli_smoke.py::test_cli_completion_uses_parser_commands_flags_and_positional_semantics"
    )
    policies = {row["name"]: row for row in plan["toolchain_policy"]}
    for shell in ("bash", "fish", "zsh"):
        assert policies[shell]["executable"] == shell
        assert policies[shell]["version_args"] == ["--version"]
    steps = yaml.safe_load(_read(".github/workflows/ci.yml"))["jobs"][
        "platform-portability"
    ]["steps"]
    setup = next(
        row for row in steps if row.get("name") == "Install completion test shells"
    )
    execute = next(
        row
        for row in steps
        if row.get("name") == "Execute selected platform portability partition"
    )
    assert setup["if"] == "matrix.cell == 'linux-x86_64-py312-queue-portability'"
    assert "apt-get install --no-install-recommends --yes fish zsh" in setup["run"]
    assert steps.index(setup) < steps.index(execute)


def test_python_unit_runs_one_receipted_job_per_generated_cell() -> None:
    jobs = yaml.safe_load(_read(".github/workflows/ci.yml"))["jobs"]
    job = jobs["python-unit"]
    assert job["runs-on"] == "${{ matrix.runner }}"
    assert job["strategy"] == {
        "fail-fast": False,
        "matrix": "${{ fromJSON(needs.classify-changes.outputs.python_unit_matrix) }}",
    }
    steps = job["steps"]
    executor = next(
        step for step in steps if "--run-family python_unit" in step.get("run", "")
    )
    assert executor["run"].split() == [
        "python3",
        "tools/proof_plan.py",
        "--run-family",
        "python_unit",
        "--receipt",
        "proof-receipts/python-unit-${{",
        "matrix.cell",
        "}}.json",
        "--matrix-cell",
        '"${{',
        "matrix.cell",
        '}}"',
    ]
    upload = next(
        step for step in steps if step.get("name") == "Upload Python unit receipt"
    )
    # One artifact per cell; the verdict merges every proof-receipt-* artifact.
    assert upload["with"] == {
        "name": "proof-receipt-python-unit-${{ matrix.cell }}",
        "path": "proof-receipts/python-unit-${{ matrix.cell }}.json",
        "if-no-files-found": "error",
    }
    assert steps.index(executor) < steps.index(upload)
    download = next(
        step
        for step in jobs["proof-plan-verdict"]["steps"]
        if step.get("uses", "").startswith("actions/download-artifact@")
    )
    assert download["with"]["pattern"] == "proof-receipt-*"
    assert download["with"]["merge-multiple"] is True


def test_checkouts_drop_persisted_credentials_and_permissions_are_bounded() -> None:
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        text = workflow.read_text(encoding="utf-8")
        lines = text.splitlines()
        for index, line in enumerate(lines):
            if "uses: actions/checkout@" not in line:
                continue
            block = "\n".join(lines[index : index + 6])
            assert "persist-credentials: false" in block, workflow

        if workflow.name in {"pr_trust_gate.yml", "pr_trust_labeler.yml"}:
            continue
        assert "\npermissions:\n  contents: read\n" in text, workflow

    release = _read(".github/workflows/release.yml")
    assert (
        release.count("contents: write") == 2
    )  # Draft read admission and protected promotion.
    assert release.count("id-token: write") == 1
    assert release.count("attestations: write") == 1
    assert release.count("artifact-metadata: write") == 1


def test_pre_commit_hooks_are_read_only_by_default() -> None:
    default_python = _default_python_version()
    pre_commit_text = _read(".pre-commit-config.yaml")

    assert "- id: ruff" in pre_commit_text
    assert "repo: https://github.com/astral-sh/ruff-pre-commit" not in pre_commit_text
    # The hooks pass staged paths explicitly; only --force-exclude makes ruff
    # honour pyproject's extend-exclude (generated modules) for explicit paths.
    assert "uv run ruff check --force-exclude" in pre_commit_text
    assert f"--python {default_python}" not in pre_commit_text
    assert "--fix" not in pre_commit_text
    assert "- id: ruff-format" in pre_commit_text
    assert "uv run ruff format --check --force-exclude" in pre_commit_text
    assert "uv run ty check src" in pre_commit_text
    assert "tools/secret_guard.py --staged" in pre_commit_text
    assert "- id: end-of-file-fixer" not in pre_commit_text
    assert "- id: trailing-whitespace" not in pre_commit_text
    assert "git diff --cached --check" in pre_commit_text
    ruff = tomllib.loads(_read("pyproject.toml"))["tool"]["ruff"]
    assert ruff["force-exclude"] is True
    assert "src/molt/_wasm_abi_generated.py" in ruff["extend-exclude"]


def test_default_ci_python_version_comes_from_single_file() -> None:
    default_python = _default_python_version()

    checked_files = [".pre-commit-config.yaml"] + [
        f".github/workflows/{path.name}" for path in sorted(WORKFLOW_ROOT.glob("*.yml"))
    ]
    minor = default_python.rsplit(".", 1)[0]
    for path in checked_files:
        text = _read(path)
        for version in (default_python, minor):
            assert f"--python {version}" not in text
            assert f"uv python install {version}" not in text
            assert f'python-version: "{version}"' not in text
            assert f"python-version: '{version}'" not in text

    setup_project = _read(".github/actions/setup-project/action.yml")
    assert "actions/setup-python@" not in setup_project
    assert "bash .github/actions/setup-project/provision-python.sh" in setup_project
    bootstrap = _read(".github/actions/setup-project/provision-python.sh")
    assert "cat .python-version" in bootstrap
    for workflow in ("ci.yml", "formal.yml", "release.yml"):
        assert "uses: ./.github/actions/setup-project" in _read(
            f".github/workflows/{workflow}"
        )


def test_repo_githook_delegates_to_pre_commit_authority() -> None:
    hook_text = _read(".githooks/pre-commit")

    assert "pre-commit run --hook-stage pre-commit" in hook_text
    assert "tools/secret_guard.py" not in hook_text


def test_ci_clippy_failures_are_not_swallowed() -> None:
    ci_text = _read(".github/workflows/ci.yml")
    proof_plan_text = _read("tools/proof_plan.toml")
    assert "rust.clippy.workspace-default" in proof_plan_text
    assert "rust.clippy.feature-surfaces" in proof_plan_text
    assert "--run-family rust --receipt" in ci_text
    assert "continue-on-error" not in ci_text


def test_ci_rust_compile_truth_has_no_redundant_subset_commands() -> None:
    ci_text = _read(".github/workflows/ci.yml")
    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    commands = {command["id"]: command for command in plan["command"]}
    backend_manifest = tomllib.loads(_read("runtime/molt-backend/Cargo.toml"))

    # Workspace tests compile every package's normal library/bin target before
    # executing test targets, while all-target Clippy covers libs, bins,
    # examples, tests, and benches.  A preceding workspace build/runtime check
    # therefore adds no target or feature coverage and can starve both truths.
    for redundant_id in (
        "rust.check.runtime-default",
        "rust.build.workspace",
        "rust.test.backend-native-feature",
        "rust.clippy.backend-native",
    ):
        assert redundant_id not in commands
    assert "native-backend" in backend_manifest["features"]["default"]
    assert commands["rust.test.default-truth"]["dependencies"] == []
    # The compiler-build resource runs one command at a time, so ordering edges
    # add no starvation protection; they would only let one red truth hide the
    # independent lint truths behind "required dependency failed".
    assert commands["rust.clippy.workspace-default"]["dependencies"] == []
    assert commands["rust.clippy.feature-surfaces"]["dependencies"] == []
    compiler_policy = next(
        policy
        for policy in plan["resource_policy"]
        if policy["name"] == "compiler-build-resource"
    )
    assert compiler_policy["max_parallel"] == 1
    assert commands["rust.test.default-truth"]["argv"] == [
        "uv",
        "run",
        "--frozen",
        "python3",
        "tools/run_cargo_test_truth.py",
    ]
    rust_job = ci_text.split("\n  rust-build-unit-smoke:", 1)[1].split(
        "\n  llvm-backend:", 1
    )[0]
    assert 'uv: "false"' not in rust_job
    assert 'cache-uv: "false"' not in rust_job
    assert commands["rust.test.default-truth"]["timeout_budget"] == "suite"
    assert commands["rust.clippy.workspace-default"]["argv"] == [
        "cargo",
        "clippy",
        "--locked",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]
    assert "logs/ci-cargo-build.log" not in ci_text


def test_ci_memory_intensive_steps_use_memory_guard() -> None:
    ci_text = _read(".github/workflows/ci.yml")

    assert "--run-family repository_policy --receipt" in ci_text
    assert "uv run python3 -m pytest -q" not in ci_text
    assert "--run-family rust --receipt" in ci_text
    assert "--run-family llvm --receipt" in ci_text
    proof_executor = _read("tools/proof_plan.py")
    assert "peak_rss_bytes" in proof_executor
    assert "guard_metrics_schema" in proof_executor
    assert "os.killpg" not in proof_executor
    assert "psutil" not in proof_executor
    assert "python3 tools/profile_hotspots.py --limit 20" in ci_text


def test_kani_intrinsic_contracts_avoid_symbolic_std_sort() -> None:
    kani_text = _read("runtime/molt-obj-model/tests/kani_intrinsic_contracts.rs")

    assert "struct BoundedI64List" in kani_text
    assert "struct BoundedBoolList" in kani_text
    assert "Vec<" not in kani_text
    assert "Vec::" not in kani_text
    assert ".collect()" not in kani_text
    assert "DefaultHasher" not in kani_text
    assert "std::hash" not in kani_text
    assert "wrapping_mul" not in kani_text
    assert ".dedup()" not in kani_text
    assert ".sort()" not in kani_text


def test_kani_advisory_probe_has_single_cargo_cache_authority() -> None:
    kani_workflow = _read(".github/workflows/kani.yml")

    assert "uses: ./.github/actions/setup-project" in kani_workflow
    assert "cache-namespace: kani" in kani_workflow
    assert "actions/cache@v4" not in kani_workflow
    assert "Cache cargo registry and target" not in kani_workflow
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "cargo install --locked kani-verifier"
    ) in kani_workflow
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- cargo kani setup"
    ) in kani_workflow
    assert "cd runtime/molt-obj-model && cargo kani --tests" not in kani_workflow
    assert "cd runtime/molt-runtime && cargo kani --tests" not in kani_workflow
    assert "cargo kani --tests" not in kani_workflow


def test_formal_workflow_uses_bounded_blocking_quint_gate() -> None:
    formal_workflow = _read(".github/workflows/formal.yml")

    assert "--run-command formal.quint.models --receipt" in formal_workflow
    assert "for model in *.qnt" not in formal_workflow
    assert 'quint verify "$model"' not in formal_workflow
    assert "failed verification (non-blocking)" not in formal_workflow


def test_lean_workflows_share_exact_provisioning_authority() -> None:
    setup_action = _read(".github/actions/setup-lean/action.yml")
    formal_workflow = _read(".github/workflows/formal.yml")
    nightly_workflow = _read(".github/workflows/nightly.yml")

    assert "toolchain=\"$(tr -d '\\r\\n' < formal/lean/lean-toolchain)\"" in (
        setup_action
    )
    probe = 'if ! "$elan" run "$toolchain" lean --version >/dev/null 2>&1; then'
    assert '"$elan" toolchain install "$toolchain"' in setup_action
    assert probe in setup_action
    assert setup_action.index(probe) < setup_action.index(
        '"$elan" toolchain install "$toolchain"'
    )
    assert 'expected_version="${toolchain##*:v}"' in setup_action
    assert '"$exact_version" != "Lean (version $expected_version,"*' in setup_action
    assert '"$selected_version" != "$exact_version"' in setup_action
    setup_project = _read(".github/actions/setup-project/action.yml")
    assert "formal/lean/.lake" in setup_project
    assert "~/.elan/toolchains" in setup_project
    assert 'cache-lean: "true"' in formal_workflow
    assert formal_workflow.count("uses: ./.github/actions/setup-lean") == 1
    assert "uses: ./.github/actions/setup-lean" not in nightly_workflow
    assert "elan/master" not in setup_action
    assert "tools.release.fetch_pinned_tool" in setup_action
    assert "elan-init.sh" not in formal_workflow
    assert "elan-init.sh" not in nightly_workflow


def test_quint_workflows_pin_patched_node24_toolchain() -> None:
    formal_workflow = _read(".github/workflows/formal.yml")
    nightly_workflow = _read(".github/workflows/nightly.yml")

    setup_project = _read(".github/actions/setup-project/action.yml")
    assert "uses: actions/setup-node@" in setup_project
    assert "node-version: pinned" in formal_workflow
    assert "check-latest: true" not in setup_project
    quint = next(
        policy
        for policy in tomllib.loads(_read("tools/proof_plan.toml"))["toolchain_policy"]
        if policy["name"] == "quint"
    )
    assert (
        f'MOLT_QUINT_NPM_PACKAGE: "@informalsystems/quint@{quint["setup_value"]}"'
        in formal_workflow
    )
    # Quint pins its own evaluator release; the workflow pins that exact release.
    assert re.search(
        r'MOLT_QUINT_RUST_EVALUATOR_VERSION: "v\d+\.\d+\.\d+"', formal_workflow
    )
    assert re.search(
        r'MOLT_QUINT_RUST_EVALUATOR_SHA256: "[0-9a-f]{64}"', formal_workflow
    )
    assert "Install Quint Rust evaluator" in formal_workflow
    assert "sha256sum --check" in formal_workflow

    assert 'npm install -g "$MOLT_QUINT_NPM_PACKAGE"' not in nightly_workflow
    assert "actions/setup-node@" not in nightly_workflow
    assert "node-version:" not in nightly_workflow
    assert "Install Quint Rust evaluator" not in nightly_workflow


def test_nightly_contains_correctness_jobs() -> None:
    nightly_text = _read(".github/workflows/nightly.yml")

    assert "schedule:" in nightly_text
    assert "workflow_dispatch:" in nightly_text
    for job in (
        "nightly-prepare:",
        "conformance-shard:",
        "differential-shard:",
        "regrtest-shard:",
        "conformance-aggregate:",
        "differential-aggregate:",
        "regrtest-aggregate:",
    ):
        assert job in nightly_text
    for family in (
        "nightly_shard_prepare",
        "nightly_conformance",
        "nightly_differential",
        "nightly_regrtest",
        "nightly_determinism",
        "nightly_verification_t3",
    ):
        assert f"--run-family {family} --receipt" in nightly_text
    assert "nightly-verdict:" in nightly_text
    assert "uses: actions/download-artifact@" in nightly_text
    assert "--verify-scheduled --receipt-dir nightly-artifacts" in nightly_text
    assert "tools/guarded_exec.py" not in nightly_text
    assert "tests/harness/run_molt_conformance.py" not in nightly_text
    assert "tests/molt_diff.py" not in nightly_text
    assert "tools/cpython_regrtest.py" not in nightly_text
    assert nightly_text.count("tools/nightly_sharding.py run-shard") == 3
    assert nightly_text.count("tools/nightly_runtime_bundle.py verify-extract") == 3
    assert nightly_text.count('MOLT_STDLIB_PROFILE: "full"') == 3
    assert nightly_text.count('MOLT_DIFF_STDLIB_PROFILE: "full"') == 1
    assert "max-parallel: 8" in nightly_text
    assert "max-parallel: 16" in nightly_text
    assert "max-parallel: 4" in nightly_text
    assert "tools/check_reproducible_build.py" not in nightly_text
    proof_plan_text = _read("tools/proof_plan.toml")
    assert 'id = "nightly.verification-t3.reproducibility"' in proof_plan_text
    assert '"--corpus", "full", "--runs", "5", "--audit-ir"' in proof_plan_text
    assert '"proof-results/reproducibility-tier3.json"' in proof_plan_text
    assert "tools/check_deterministic_runtime.py" not in nightly_text
    assert "proof-results/nightly/deterministic-runtime.json" in nightly_text
    assert "proof-results/nightly/ir-verification.json" in nightly_text
    assert "name: proof-receipt-nightly-determinism" in nightly_text
    assert "mkdir -p /tmp/repro_sweep" not in nightly_text
    assert "MOLT_CACHE=/tmp/repro_sweep" not in nightly_text
    assert "~/.molt/build/" not in nightly_text
    assert "cargo build -p molt-runtime" not in nightly_text
    assert "cargo build -p molt-runtime --release" not in nightly_text
    assert "SKIP: build failed" not in nightly_text
    assert "|| true" not in nightly_text
    assert "continue-on-error: true" not in nightly_text


def test_hosted_workflow_heavy_commands_enter_memory_guard() -> None:
    nightly_text = _read(".github/workflows/nightly.yml")
    formal_text = _read(".github/workflows/formal.yml")
    security_text = _read(".github/workflows/security_hardening.yml")
    release_text = _read(".github/workflows/release.yml")

    assert nightly_text.count("python3 tools/proof_plan.py --run-family") == 7
    assert "run: cargo build -p molt-runtime --profile dev-fast" not in nightly_text
    assert "tools/ci_gate.py --tier" not in nightly_text
    assert "uses: ./.github/workflows/formal.yml" in nightly_text
    assert '"formal-methods-full"' not in _read("tools/ci_gate.py")
    assert '"formal-methods-quint-only"' not in _read("tools/ci_gate.py")
    assert '"correspondence-check"' not in _read("tools/ci_gate.py")
    assert "quint verify formal/quint/" not in nightly_text
    assert "run: cargo install cargo-deny --locked" not in nightly_text
    assert "run: cargo deny check" not in nightly_text
    assert "          quint verify formal/quint/" not in nightly_text
    assert "uses: ./.github/workflows/security_hardening.yml" not in nightly_text

    assert "--run-command formal.lean.build --receipt" not in formal_text
    assert "--run-command formal.lean.sorry-baseline --receipt" in formal_text
    assert "--run-command formal.quint.models --receipt" in formal_text
    assert "--run-command formal.correspondence --receipt" in formal_text
    assert "run: lake build" not in formal_text
    assert "run: python3 tools/check_formal_methods.py --quint-only" not in formal_text
    assert (
        "run: python3 tools/check_formal_methods.py --check-correspondence"
        not in formal_text
    )

    assert "--run-family python_security --receipt" in security_text
    assert "--run-family rust_security --receipt" in security_text
    policies = {
        policy["name"]: policy["setup_value"]
        for policy in tomllib.loads(_read("tools/proof_plan.toml"))["toolchain_policy"]
        if "setup_value" in policy
    }
    for tool in ("cargo-deny", "cargo-audit"):
        assert (
            "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
            f"cargo install {tool} --version {policies[tool]} --locked"
        ) in security_text
    assert "run: uv run pip-audit --ignore-vuln CVE-2025-69872" not in security_text
    assert "run: cargo deny check" not in security_text
    assert "          cargo install cargo-deny --locked" not in security_text
    assert "          cargo install cargo-audit --locked" not in security_text

    release_workflow = yaml.safe_load(release_text)
    build_steps = [
        step["run"]
        for job in release_workflow["jobs"].values()
        for step in job.get("steps", [])
        if step.get("name") == "Build independent native release generations"
    ]
    assert len(build_steps) == 1
    assert "for lane in primary secondary; do" in build_steps[0]
    assert "tools/guarded_exec.py --prefix MOLT_RELEASE --" in build_steps[0]
    assert "python -m tools.release.build_compiler" in build_steps[0]
    assert '--output "dist/native-$lane"' in build_steps[0]
    assert "run: cargo build -p molt-worker --release" not in release_text


def test_named_proof_lanes_fail_closed_and_share_counted_verdict_authority() -> None:
    nightly_text = _read(".github/workflows/nightly.yml")
    formal_text = _read(".github/workflows/formal.yml")
    ci_gate_text = _read("tools/ci_gate.py")

    for workflow_text in (nightly_text, formal_text):
        assert "continue-on-error: true" not in workflow_text
    assert "SKIP: build failed" not in nightly_text
    assert "if-no-files-found: ignore" not in nightly_text
    assert '"--no-fail"' not in ci_gate_text
    assert '"success": passed + failed + errored > 0' in ci_gate_text
    assert '"zero_work": passed + failed + errored == 0' in ci_gate_text
    assert '"required": r.name in required_names' in ci_gate_text
    for tool in (
        "tools/check_deterministic_runtime.py",
        "tools/check_reproducible_build.py",
        "tools/verify_ir_suite.py",
        "tools/mutation_test.py",
        "tools/translation_validate.py",
    ):
        text = _read(tool)
        assert "fail_closed_proof_exit_code(" in text
    for tool in (
        "tools/check_deterministic_runtime.py",
        "tools/verify_ir_suite.py",
    ):
        text = _read(tool)
        assert '"executed": passed + failed' in text
    reproducibility_text = _read("tools/check_reproducible_build.py")
    assert "def _write_proof_receipt(" in reproducibility_text
    for count in ("selected", "executed", "passed", "failed", "errors"):
        assert f'"{count}": {count}' in reproducibility_text

    assert fail_closed_proof_exit_code(executed=1, failed=0, errors=0) == 0
    assert fail_closed_proof_exit_code(executed=1, failed=1, errors=0) == 1
    assert fail_closed_proof_exit_code(executed=0, failed=0, errors=0) == 2
    assert fail_closed_proof_exit_code(executed=0, failed=0, errors=1) == 2
    with pytest.raises(ValueError, match="cannot exceed"):
        fail_closed_proof_exit_code(executed=0, failed=1, errors=0)


def test_security_hardening_is_reusable_and_ci_uses_one_planner() -> None:
    ci_text = _read(".github/workflows/ci.yml")
    security_text = _read(".github/workflows/security_hardening.yml")

    assert "workflow_call:" in security_text
    assert "classify-changes:" not in security_text
    assert "--github-output" not in security_text
    assert "inputs.python_security" in security_text
    assert "inputs.rust_security" in security_text
    assert "uses: ./.github/workflows/security_hardening.yml" in ci_text
    assert ci_text.count('tools/proof_plan.py --github-output "$GITHUB_OUTPUT"') == 1
    assert ci_text.count("tools/proof_plan.py\n          --verify-selected") == 1


def test_release_and_perf_workflows_exist_for_hosted_validation() -> None:
    release_text = _read(".github/workflows/release.yml")
    perf_text = _read(".github/workflows/perf-gate.yml")

    assert "push:" not in release_text
    assert 'test "$GITHUB_REF" = "refs/tags/$REQUESTED_VERSION"' in release_text
    assert "workflow_dispatch:" in release_text
    release_config = _read("config/release_targets.toml")
    assert "macos-15" in release_config
    assert "ubuntu-24.04" in release_config
    assert "windows-2022" in release_config
    assert "windows-11-arm" in release_config
    assert "fromJSON(needs.plan.outputs.matrix)" in release_text
    assert "schedule:" in perf_text
    assert "MOLT_SESSION_ID: perfscore-${{ matrix.backend }}" in perf_text
    assert "tools/guarded_exec.py --prefix MOLT_BENCH" in perf_text
    assert "tools/perf_scoreboard.py" in perf_text
    assert "backend: [native, llvm]" in perf_text
    assert '--backend "${{ matrix.backend }}"' in perf_text
    assert "--profile release-fast" in perf_text
    assert "--samples 5" in perf_text
    assert "--warmup 2" in perf_text
    assert "--repeat 5" in perf_text
    assert "--classify" in perf_text
    assert "--require-quiescent" in perf_text
    assert "bench/scoreboard/logs_*/" in perf_text
    assert "--no-gate" not in perf_text
    assert "--allow-nonauthoritative" not in perf_text
    assert "tools/bench.py" not in perf_text
    assert "bench/results/" not in perf_text
    assert not (WORKFLOW_ROOT / "perf-validation.yml").exists()


def test_perf_demo_workflow_uses_canonical_env_and_single_uv_sync() -> None:
    perf_demo_text = _read(".github/workflows/perf_demo.yml")
    run_stack_text = _read("bench/scripts/run_stack.sh")

    assert "MOLT_SESSION_ID: perf-demo-${{ github.run_id }}" in perf_demo_text
    assert 'MOLT_UV_SYNC: "0"' in perf_demo_text
    assert 'if [[ "${MOLT_UV_SYNC:-1}" != "0" ]]' in run_stack_text
    assert 'cargo build --profile "$CARGO_PROFILE" -p molt-worker' in run_stack_text
    assert (
        'CARGO_ROOT="${CARGO_TARGET_DIR:-$ROOT/target/sessions/${MOLT_SESSION_ID:-demo-stack}}"'
        in run_stack_text
    )
    assert 'WORKER_BIN="$CARGO_ROOT/$CARGO_PROFILE/molt-worker"' in run_stack_text


def test_wasm_ci_uses_molt_wasm_host_for_imported_modules() -> None:
    wasm_text = _read(".github/workflows/molt-wasm-ci.yml")
    proof_text = _read("tools/proof_plan.toml")

    assert "--run-family wasm --receipt" in wasm_text
    assert 'id = "wasm.build.host"' in proof_text
    assert 'id = "wasm.run.hello"' in proof_text
    assert 'id = "wasm.run.comprehension"' in proof_text
    assert 'id = "wasm.run.sieve"' in proof_text
    assert "cargo build --profile dev-fast -p molt-wasm-host" not in wasm_text
    assert "wasmtime run /tmp/test_hello.wasm" not in wasm_text
    assert "wasmtime run /tmp/test_comprehension.wasm" not in wasm_text
    assert "wasmtime run /tmp/test_sieve.wasm" not in wasm_text


def test_node_toolchain_consumers_provision_the_canonical_version() -> None:
    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    node_policy = next(
        policy for policy in plan["toolchain_policy"] if policy["name"] == "node"
    )
    node_families = {
        command["family"]
        for command in plan["command"]
        if "node" in command["toolchains"]
    }
    consumers = {
        "repository_policy": (".github/workflows/ci.yml", "docs-gates"),
        "rust": (".github/workflows/ci.yml", "rust-build-unit-smoke"),
        "wasm": (".github/workflows/molt-wasm-ci.yml", "wasm-build"),
        "formal": (".github/workflows/formal.yml", "formal-quint"),
    }
    # setup-project resolves `pinned` from config/tool_releases.toml; the
    # plan's preflight pattern must accept exactly that version.
    assert node_policy["setup_value"] == tool_releases.tool_release("node").version

    assert node_families == set(consumers)
    for family, (workflow_path, job_name) in consumers.items():
        jobs = yaml.safe_load(_read(workflow_path))["jobs"]
        setup_steps = [
            step
            for step in jobs[job_name]["steps"]
            if step.get("uses") == "./.github/actions/setup-project"
        ]
        assert len(setup_steps) == 1, family
        assert setup_steps[0].get("with", {}).get("node-version") == "pinned", family


def test_wasm_ci_uses_canonical_artifact_roots_and_dev_profile() -> None:
    wasm_text = _read(".github/workflows/molt-wasm-ci.yml")
    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    wasm_commands = [
        command for command in plan["command"] if command["family"] == "wasm"
    ]

    assert "MOLT_EXT_ROOT: /tmp/molt-ext" in wasm_text
    assert "workflow_call:" in wasm_text
    assert "push:" not in wasm_text
    assert "pull_request:" not in wasm_text
    assert "MOLT_CACHE: /tmp/molt-ext/molt_cache" in wasm_text
    assert "MOLT_DIFF_ROOT: /tmp/molt-ext/diff" in wasm_text
    assert "MOLT_DIFF_TMPDIR: /tmp/molt-ext/tmp" in wasm_text
    assert "MOLT_WASM_RUNTIME_DIR: /tmp/molt-ext/wasm" in wasm_text
    assert "concurrency:" in wasm_text
    assert "cancel-in-progress: ${{ github.event_name == 'pull_request' }}" in (
        wasm_text
    )
    assert "MOLT_CI_PYTHON" not in wasm_text
    assert "uses: ./.github/actions/setup-project" in wasm_text
    assert 'cache-cargo: "true"' in wasm_text
    assert "cache-namespace: wasm-ci" in wasm_text
    assert "python3 -m molt.tool_releases provision wasm-tools" in wasm_text
    assert '--github-path "$GITHUB_PATH"' in wasm_text
    assert "taiki-e/install-action" not in wasm_text
    assert (
        "MOLT_SESSION_ID: wasm-ci-${{ github.run_id }}-${{ github.run_attempt }}"
        in wasm_text
    )
    assert "MOLT_WASM_TEST_CHILD_RLIMIT_GB" not in wasm_text
    assert 'MOLT_WASM_TEST_KEEPALIVE_SEC: "20"' in wasm_text
    assert 'MOLT_PREFER_EXTERNAL_ARTIFACTS: "1"' in wasm_text
    assert 'MOLT_MEMORY_GUARD_TERMINATION_WAIT_SEC: "2"' in wasm_text
    assert "CARGO_INCREMENTAL:" not in wasm_text
    assert 'CARGO_BUILD_JOBS: "1"' not in wasm_text
    assert "MOLT_WASM_TEST_TIMEOUT_SEC:" not in wasm_text
    assert "MOLT_CARGO_TIMEOUT:" not in wasm_text
    assert "MOLT_BACKEND_DAEMON_SOCKET_DIR" not in wasm_text
    assert "MOLT_BACKEND_DAEMON_CACHE_MB" not in wasm_text
    assert "tools/guarded_exec.py" not in wasm_text
    assert "tools/venv_exec.py" not in wasm_text
    assert "--run-family wasm --receipt" in wasm_text
    assert len(wasm_commands) >= 12
    assert all(command.get("timeout_env") for command in wasm_commands)
    assert all(
        command.get("timeout_budget") or command.get("timeout_seconds")
        for command in wasm_commands
    )
    assert (
        next(
            command for command in wasm_commands if command["id"] == "wasm.build.host"
        )["timeout_budget"]
        == "cold"
    )
    assert any(command["id"] == "wasm.compile.hello" for command in wasm_commands)
    assert any(command["id"] == "wasm.test.control-flow" for command in wasm_commands)
    assert "python3 tools/profile_hotspots.py --limit 20" in wasm_text
    assert "/home/runner/.cache/molt" not in wasm_text


def test_wasm_ci_guarded_steps_have_github_timeout_backstops() -> None:
    wasm_text = _read(".github/workflows/molt-wasm-ci.yml")
    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    wasm_family = next(
        family for family in plan["ci_family"] if family["name"] == "wasm"
    )

    assert f"timeout-minutes: {wasm_family['timeout_minutes']}" in wasm_text
    assert "--timeout" not in wasm_text
    assert "MOLT_CARGO_TIMEOUT:" not in wasm_text
    assert "MOLT_WASM_TEST_TIMEOUT_SEC:" not in wasm_text


# Repository Actions policy (Settings > Actions > General), read from
# `gh api repos/adpena/molt/actions/permissions/selected-actions`: GitHub-owned
# actions, actions owned by the repository owner, and these patterns. A
# disallowed `uses:` fails every job at "Prepare all required actions" before
# any step runs, so the policy is checked statically here.
_ACTIONS_POLICY_OWNERS = frozenset(("actions", "github", "adpena"))
_ACTIONS_POLICY_PATTERNS = (
    "astral-sh/setup-uv@*",
    "cloudflare/wrangler-action@*",
    "softprops/action-gh-release@*",
    "taiki-e/install-action@*",
)


def _action_references() -> list[tuple[str, str]]:
    references: list[tuple[str, str]] = []
    root = Path(__file__).resolve().parents[1]
    paths = sorted((root / ".github" / "workflows").glob("*.yml")) + sorted(
        (root / ".github" / "actions").glob("*/action.yml")
    )
    for path in paths:
        document = yaml.safe_load(path.read_text(encoding="utf-8"))
        jobs = (document or {}).get("jobs", {}) or {}
        steps = [
            step for job in jobs.values() for step in (job or {}).get("steps", []) or []
        ]
        steps += ((document or {}).get("runs", {}) or {}).get("steps", []) or []
        uses = [
            job["uses"]
            for job in jobs.values()
            if isinstance(job, dict) and "uses" in job
        ]
        uses += [
            step["uses"] for step in steps if isinstance(step, dict) and "uses" in step
        ]
        references.extend((path.relative_to(root).as_posix(), use) for use in uses)
    return references


def test_every_action_reference_is_admitted_by_repository_policy() -> None:
    references = _action_references()
    assert references
    for source, use in references:
        if use.startswith("./"):
            continue
        assert re.fullmatch(r"[\w.-]+/[\w./-]+@[0-9a-f]{40}", use), (source, use)
        owner = use.split("/", 1)[0]
        admitted = owner in _ACTIONS_POLICY_OWNERS or any(
            fnmatchcase(use, pattern) for pattern in _ACTIONS_POLICY_PATTERNS
        )
        assert admitted, (
            f"{source}: {use} is not allowed by the repository Actions policy"
        )


def test_bootstrap_binds_managed_python_before_repository_or_rust_consumers() -> None:
    action = yaml.safe_load(_read(".github/actions/setup-project/action.yml"))
    steps = action["runs"]["steps"]
    names = [step.get("name") for step in steps]
    python = names.index("Provision and bind repository Python")
    assert names.index("Install exact uv") < python < names.index("Install exact Rust")
    assert (
        action["outputs"]["python-path"]["value"]
        == "${{ steps.python.outputs.python-path }}"
    )
    for step in steps[:python]:
        assert not re.search(r"\bpython3?\s", str(step.get("run", "")))
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        payload = yaml.safe_load(workflow.read_text(encoding="utf-8"))
        for job in payload.get("jobs", {}).values():
            for step in job.get("steps", []):
                if step.get("uses") != "./.github/actions/setup-project":
                    continue
                inputs = step.get("with", {})
                if str(inputs.get("python", "true")) == "true":
                    assert str(inputs.get("uv", "true")) == "true", workflow
    job = yaml.safe_load(_read(".github/workflows/ci.yml"))["jobs"]["python-unit"]
    setup = next(
        step
        for step in job["steps"]
        if step.get("uses") == "./.github/actions/setup-project"
    )
    assert setup["with"]["rust-toolchain"] == "pinned"
    execute = next(
        step
        for step in job["steps"]
        if "--run-family python_unit" in step.get("run", "")
    )
    assert job["steps"].index(setup) < job["steps"].index(execute)


def test_workflows_root_no_artifact_state_inside_the_checkout() -> None:
    """setup-project gives every job custody under RUNNER_TEMP; dx rejects a
    canonical root (target, caches, scratch) that points into the checkout."""
    import sys

    sys.path.insert(0, str(REPO_ROOT / "src"))
    from molt.dx import CANONICAL_ROOT_ENV_KEYS

    keys = set(CANONICAL_ROOT_ENV_KEYS) | {"MOLT_WASM_TEST_CARGO_TARGET_DIR"}
    offenders = []
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        for number, line in enumerate(
            workflow.read_text(encoding="utf-8").splitlines(), start=1
        ):
            match = re.match(r"^\s+([A-Z_]+):\s*(.*)$", line)
            if match and match[1] in keys and "github.workspace" in match[2]:
                offenders.append(f"{workflow.name}:{number}: {line.strip()}")
    assert offenders == []
    wasm_text = _read(".github/workflows/molt-wasm-ci.yml")
    assert (
        "printf 'MOLT_WASM_TEST_CARGO_TARGET_DIR=%s\\n' \"$CARGO_TARGET_DIR\""
        in wasm_text
    )
