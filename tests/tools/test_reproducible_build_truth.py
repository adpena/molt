from __future__ import annotations

import json
from pathlib import Path
import sys

import pytest

from tools import check_reproducible_build as reproducibility


def test_repeated_build_requires_two_observations(tmp_path: Path) -> None:
    source = tmp_path / "source.py"
    source.write_text("print('ok')\n", encoding="utf-8")

    matched, details = reproducibility._build_repeated_and_compare(
        str(source), "dev", True, False, 1
    )

    assert matched is False
    assert details["error"] == "runs must be at least 2"


def test_repeated_build_compares_every_observation(tmp_path: Path, monkeypatch) -> None:
    source = tmp_path / "source.py"
    artifact = tmp_path / "artifact.o"
    source.write_text("print('ok')\n", encoding="utf-8")
    observations = [b"same", b"same", b"different"]

    def fake_build_once(source, cache_dir, profile, prefer_object, build_timeout):
        artifact.write_bytes(observations.pop(0))
        return (
            str(artifact),
            "",
            {"status": "completed", "environment": {"MOLT_CACHE": cache_dir}},
        )

    monkeypatch.setattr(reproducibility, "_build_once", fake_build_once)
    matched, details = reproducibility._build_repeated_and_compare(
        str(source), "dev", True, False, 3
    )

    assert matched is False
    assert details["runs"] == 3
    assert details["unique_hashes"] == 2


def test_batch_receipt_counts_artifact_and_audit_results(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "source.py"
    receipt = tmp_path / "receipt.json"
    source.write_text("print('ok')\n", encoding="utf-8")
    monkeypatch.setattr(
        reproducibility,
        "_build_repeated_and_compare",
        lambda *_args, **_kwargs: (
            True,
            {"source": str(source), "runs": 2, "match": True},
        ),
    )
    monkeypatch.setattr(
        reproducibility,
        "check_ir_determinism",
        lambda _programs, _runs: [{"check": "ir", "status": "pass"}],
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_reproducible_build.py",
            "--batch",
            str(source),
            "--audit-ir",
            "--json-out",
            str(receipt),
        ],
    )

    assert reproducibility.main() == 0
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert payload["schema"] == "molt.reproducibility-proof.v3"
    assert payload["selected"] == 2
    assert payload["executed"] == 2
    assert payload["status"] == "success"


def test_batch_receipt_fails_closed_for_missing_registered_source(
    tmp_path: Path, monkeypatch
) -> None:
    receipt = tmp_path / "receipt.json"
    missing = tmp_path / "missing.py"
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_reproducible_build.py",
            "--batch",
            str(missing),
            "--json-out",
            str(receipt),
        ],
    )

    assert reproducibility.main() == 2
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert payload["selected"] == 1
    assert payload["executed"] == 0
    assert payload["errors"] == 1
    assert payload["status"] == "failure"


def test_single_build_mode_emits_counted_receipt(tmp_path: Path, monkeypatch) -> None:
    source = tmp_path / "source.py"
    receipt = tmp_path / "receipt.json"
    source.write_text("print('ok')\n", encoding="utf-8")
    monkeypatch.setattr(
        reproducibility,
        "_build_repeated_and_compare",
        lambda *_args, **_kwargs: (
            True,
            {"source": str(source), "runs": 2, "match": True},
        ),
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_reproducible_build.py",
            "--build",
            str(source),
            "--json-out",
            str(receipt),
        ],
    )

    assert reproducibility.main() == 0
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert payload["mode"] == "build"
    assert payload["selected"] == payload["executed"] == payload["passed"] == 1
    assert payload["status"] == "success"


def test_compare_mode_emits_counted_receipt(tmp_path: Path, monkeypatch) -> None:
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"identical")
    build_a = tmp_path / "a.json"
    build_b = tmp_path / "b.json"
    receipt = tmp_path / "receipt.json"
    build_a.write_text(json.dumps({"output": str(artifact)}), encoding="utf-8")
    build_b.write_text(json.dumps({"output": str(artifact)}), encoding="utf-8")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_reproducible_build.py",
            str(build_a),
            str(build_b),
            "--json-out",
            str(receipt),
        ],
    )

    assert reproducibility.main() == 0
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert payload["mode"] == "compare"
    assert payload["selected"] == payload["executed"] == payload["passed"] == 1
    assert payload["status"] == "success"


@pytest.mark.parametrize("temporary_alias", [False, True])
def test_build_and_ir_receipt_environments_match_real_children(
    tmp_path: Path, monkeypatch, temporary_alias: bool
) -> None:
    """Substitute only payload computation; retain real guard/process/receipt joins."""
    if temporary_alias:
        import tempfile

        physical = tmp_path / "physical-temporary-root"
        physical.mkdir()
        alias = tmp_path / "temporary-alias"
        try:
            alias.symlink_to(physical, target_is_directory=True)
        except OSError as error:
            pytest.skip(f"host does not permit directory symlinks: {error}")
        monkeypatch.setattr(tempfile, "tempdir", str(alias))
    source = tmp_path / "source.py"
    source.write_text("print('stable')\n", encoding="utf-8")
    receipt = tmp_path / "receipt.json"
    witnesses = {"build": [], "compiler": []}
    real_guard = reproducibility.harness_memory_guard.guarded_completed_process
    monkeypatch.setenv("PYTHONHASHSEED", "77")
    monkeypatch.delenv("MOLT_DETERMINISTIC", raising=False)
    monkeypatch.setenv("PROVENANCE_TEST_SECRET", "must-not-be-serialized")

    def payload_guard(command, **kwargs):
        if command[:4] == [sys.executable, "-m", "molt.cli", "build"]:
            phase = "build"
            artifact = Path(kwargs["env"]["MOLT_CACHE"]) / "artifact"
            tail = (
                f"Path({str(artifact)!r}).write_bytes(b'constant fixture artifact'); "
                f"print(json.dumps({{'output': {str(artifact)!r}}}))"
            )
        elif (
            command[:2] == [sys.executable, "-c"]
            and "from molt.frontend import compile_to_tir" in command[2]
        ):
            phase = "compiler"
            assert kwargs["input"] == source.read_text(encoding="utf-8")
            tail = "print('{}')"
        else:
            return real_guard(command, **kwargs)
        witness = tmp_path / f"{phase}-{len(witnesses[phase])}.json"
        witnesses[phase].append(witness)
        observer = (
            "import json, os, sys; from pathlib import Path; "
            "e = {k: os.environ.get(k) for k in "
            "('PYTHONPATH','PYTHONHASHSEED','MOLT_DETERMINISTIC','MOLT_CACHE',"
            "'MOLT_EXT_ROOT','MOLT_TARGET_ROOT','CARGO_TARGET_DIR',"
            "'MOLT_BACKEND_DAEMON','MOLT_BACKEND_DAEMON_SOCKET_DIR','TMP','TEMP')}; "
            f"Path({str(witness)!r}).write_text(json.dumps("
            "{'environment': e, 'cwd': os.getcwd(), 'argv': sys.orig_argv}), "
            "encoding='utf-8'); " + tail
        )
        return real_guard([sys.executable, "-B", "-c", observer], **kwargs)

    monkeypatch.setattr(
        reproducibility.harness_memory_guard, "guarded_completed_process", payload_guard
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_reproducible_build.py",
            "--batch",
            str(source),
            "--audit-ir",
            "--runs",
            "2",
            "--json-out",
            str(receipt),
        ],
    )

    assert reproducibility.main() == 0
    raw = receipt.read_text(encoding="utf-8")
    payload = json.loads(raw)
    assert payload["schema"] == "molt.reproducibility-proof.v3"
    assert payload["selected"] == payload["executed"] == payload["passed"] == 2
    assert "PROVENANCE_TEST_SECRET" not in raw
    assert "must-not-be-serialized" not in raw
    for phase, result in (
        ("build", payload["results"][0]),
        ("compiler", payload["audits"][0]),
    ):
        assert result["source"] == str(source)
        assert "environment" not in result
        assert "command" not in result
        assert len(result["observations"]) == len(witnesses[phase]) == 2
        for index, (row, witness) in enumerate(
            zip(result["observations"], witnesses[phase], strict=True)
        ):
            observed = json.loads(witness.read_text(encoding="utf-8"))
            launch = row[phase]
            assert launch["environment"] == observed["environment"]
            assert launch["argv"] == observed["argv"]
            assert launch["cwd"] == observed["cwd"]
            if temporary_alias:
                allocated = (
                    launch["environment"]["MOLT_CACHE"]
                    if phase == "build"
                    else launch["cwd"]
                )
                assert Path(allocated).parent == physical
            assert launch["status"] == "completed"
            assert launch["returncode"] == launch["child_returncode"] == 0
            assert launch["environment"]["PYTHONHASHSEED"] == (
                "0" if phase == "build" else str(index)
            )
            assert launch["environment"]["MOLT_DETERMINISTIC"] == (
                "1" if phase == "build" else None
            )
            assert "sha256" in row


@pytest.mark.parametrize("phase", ["build", "compiler"])
def test_failed_compiler_phase_retains_evidence_and_does_not_count_as_pass(
    tmp_path: Path, monkeypatch, phase: str
) -> None:
    source = tmp_path / "source.py"
    source.write_text("print('ok')\n", encoding="utf-8")
    calls = []
    real_guard = reproducibility.harness_memory_guard.guarded_completed_process

    def failed_guard(command, **kwargs):
        owns_command = (
            command[:4] == [sys.executable, "-m", "molt.cli", "build"]
            if phase == "build"
            else command[:2] == [sys.executable, "-c"]
            and "from molt.frontend import compile_to_tir" in command[2]
        )
        if not owns_command:
            return real_guard(command, **kwargs)
        calls.append(command)
        return reproducibility.harness_memory_guard.GuardedCompletedProcess(
            command,
            23,
            "",
            "compiler refused",
            elapsed_s=0.01,
            child_returncode=23,
            child_stderr="compiler refused",
        )

    monkeypatch.setattr(
        reproducibility.harness_memory_guard, "guarded_completed_process", failed_guard
    )
    if phase == "build":
        matched, result = reproducibility._build_repeated_and_compare(
            str(source), "dev", False, False, 2
        )
        assert matched is False
    else:
        result = reproducibility.check_ir_determinism([source], 2)[0]
        assert result["status"] == "error"
        assert result["sha256"] == []
    assert "compiler refused" in result["error"]
    assert len(calls) == len(result["observations"]) == 1
    row = result["observations"][0]
    assert "sha256" not in row
    assert row[phase]["argv"] == calls[0]
    assert row[phase]["returncode"] == row[phase]["child_returncode"] == 23
    assert row[phase]["environment"]["PYTHONHASHSEED"] == "0"


@pytest.mark.parametrize(
    "build_json", [[], None, {"output": 23}, {"data": []}, {"data": {"output": 23}}]
)
@pytest.mark.parametrize("mode", ["build", "compare"])
def test_malformed_artifact_shape_is_a_counted_error(
    tmp_path: Path, monkeypatch, build_json: object, mode: str
) -> None:
    source = tmp_path / "source.py"
    source.write_text("print('stable')\n", encoding="utf-8")
    receipt = tmp_path / "receipt.json"
    calls = []
    real_guard = reproducibility.harness_memory_guard.guarded_completed_process

    def build_guard(command, **kwargs):
        if command[:4] != [sys.executable, "-m", "molt.cli", "build"]:
            return real_guard(command, **kwargs)
        launched = real_guard(
            [sys.executable, "-B", "-c", f"print({json.dumps(build_json)!r})"], **kwargs
        )
        calls.append(launched.args)
        return launched

    monkeypatch.setattr(
        reproducibility.harness_memory_guard, "guarded_completed_process", build_guard
    )
    if mode == "build":
        args = ["--build", str(source)]
    else:
        document = tmp_path / "build.json"
        document.write_text(json.dumps(build_json), encoding="utf-8")
        args = [str(document), str(document)]
    monkeypatch.setattr(
        sys, "argv", ["check_reproducible_build.py", *args, "--json-out", str(receipt)]
    )

    assert reproducibility.main() == 2
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert (
        payload["selected"],
        payload["executed"],
        payload["passed"],
        payload["failed"],
        payload["errors"],
    ) == (1, 0, 0, 0, 1)
    if mode == "build":
        result = payload["results"][0]
        assert "build artifact error" in result["error"]
        row = result["observations"][0]
        assert len(calls) == 1 and row["build"]["argv"] == calls[0]
        assert row["build"]["status"] == "completed"
        assert row["build"]["child_returncode"] == 0
    else:
        assert calls == [] and "observations" not in payload
        assert "error" in payload and "results" not in payload


@pytest.mark.parametrize("failure", ["read", "stat", "cleanup", "identity"])
def test_later_build_artifact_failure_retains_completed_child(
    tmp_path: Path, monkeypatch, failure: str
) -> None:
    from contextlib import contextmanager

    source = tmp_path / "source.py"
    source.write_text("print('stable')\n", encoding="utf-8")
    receipt = tmp_path / "receipt.json"
    real_guard = reproducibility.harness_memory_guard.guarded_completed_process
    real_temp = reproducibility.OwnedTemporaryDirectory
    real_hash = reproducibility.sha256_file
    real_stat = Path.stat
    launches = []
    retained_allocations = []

    def payload_guard(command, **kwargs):
        if command[:4] != [sys.executable, "-m", "molt.cli", "build"]:
            return real_guard(command, **kwargs)
        artifact = Path(kwargs["env"]["MOLT_CACHE"]) / "artifact"
        script = (
            "import json; from pathlib import Path; "
            f"Path({str(artifact)!r}).write_bytes(b'fixture'); "
            f"print(json.dumps({{'output': {str(artifact)!r}}}))"
        )
        launched = real_guard([sys.executable, "-B", "-c", script], **kwargs)
        launches.append(launched.args)
        return launched

    def artifact_hash(path):
        if failure == "read" and Path(path).parent.name.startswith("repro_1_"):
            raise OSError()
        return real_hash(path)

    def artifact_stat(path, *args, **kwargs):
        # is_file/exists during admission still work. Fail only the aggregate's
        # post-hash stat, after the second completed launch has been recorded.
        if (
            failure == "stat"
            and path.name == "artifact"
            and path.parent.name.startswith("repro_1_")
            and len(launches) == 2
            and second_hashed
        ):
            raise OSError()
        return real_stat(path, *args, **kwargs)

    second_hashed = False

    def tracked_hash(path):
        nonlocal second_hashed
        digest = artifact_hash(path)
        if Path(path).parent.name.startswith("repro_1_"):
            second_hashed = True
        return digest

    @contextmanager
    def temporary_directory(*, prefix):
        with real_temp(prefix=prefix, dir=tmp_path) as directory:
            yield directory
            if failure == "identity" and prefix == "repro_1_":
                original = Path(directory)
                retained = original.with_name(original.name + "-retained")
                original.rename(retained)
                original.mkdir()
                (original / "replacement").write_bytes(b"other owner")
                retained_allocations.append((original, retained))
        if failure == "cleanup" and prefix == "repro_1_":
            raise OSError()

    monkeypatch.setattr(
        reproducibility.harness_memory_guard, "guarded_completed_process", payload_guard
    )
    monkeypatch.setattr(
        reproducibility,
        "OwnedTemporaryDirectory",
        temporary_directory,
    )
    monkeypatch.setattr(reproducibility, "sha256_file", tracked_hash)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_reproducible_build.py",
            "--build",
            str(source),
            "--json-out",
            str(receipt),
        ],
    )

    with monkeypatch.context() as filesystem_boundary:
        filesystem_boundary.setattr(Path, "stat", artifact_stat)
        assert reproducibility.main() == 2
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert payload["errors"] == 1 and payload["executed"] == payload["passed"] == 0
    result = payload["results"][0]
    assert result["completed_runs"] == 1
    first, second = result["observations"]
    assert first["sha256"]
    assert second["build"]["status"] == "completed"
    assert second["build"]["argv"] == launches[1]
    assert second["build"]["child_returncode"] == 0
    assert second["error"] == (
        "temporary directory allocation changed before cleanup"
        if failure == "identity"
        else "OSError"
    )
    assert second["error_phase"] == (
        "cleanup" if failure in {"cleanup", "identity"} else "artifact"
    )
    assert result["hashes"][0] == first["sha256"]

    if failure == "identity":
        assert len(retained_allocations) == 1
        original, retained = retained_allocations[0]
        assert (original / "replacement").read_bytes() == b"other owner"
        assert (retained / "artifact").read_bytes() == b"fixture"


@pytest.mark.parametrize(
    "failure", ["prepare", "launch", "cleanup", "launch-cleanup", "identity"]
)
def test_later_ir_empty_oserror_cannot_pass_or_erase_completed_observation(
    tmp_path: Path, monkeypatch, failure: str
) -> None:
    from contextlib import contextmanager
    import hashlib

    source = tmp_path / "source.py"
    source.write_text("print('stable')\n", encoding="utf-8")
    receipt = tmp_path / "receipt.json"
    real_guard = reproducibility.harness_memory_guard.guarded_completed_process
    real_temp = reproducibility.OwnedTemporaryDirectory
    launches = []
    retained_allocations = []

    def ir_guard(command, **kwargs):
        if (
            command[:2] != [sys.executable, "-c"]
            or "from molt.frontend import compile_to_tir" not in command[2]
        ):
            return real_guard(command, **kwargs)
        if (
            failure in {"launch", "launch-cleanup"}
            and kwargs["env"]["PYTHONHASHSEED"] == "1"
        ):
            launches.append(command)
            raise OSError()
        launched = real_guard([sys.executable, "-B", "-c", "print('{}')"], **kwargs)
        launches.append(launched.args)
        return launched

    @contextmanager
    def temporary_directory(*, prefix):
        if failure == "prepare" and prefix == "repro_ir_1_":
            raise OSError()
        with real_temp(prefix=prefix, dir=tmp_path) as directory:
            yield directory
            if failure == "identity" and prefix == "repro_ir_1_":
                original = Path(directory)
                retained = original.with_name(original.name + "-retained")
                original.rename(retained)
                original.mkdir()
                (original / "replacement").write_bytes(b"other owner")
                retained_allocations.append((original, retained))
        if failure in {"cleanup", "launch-cleanup"} and prefix == "repro_ir_1_":
            raise OSError()

    monkeypatch.setattr(
        reproducibility.harness_memory_guard, "guarded_completed_process", ir_guard
    )
    monkeypatch.setattr(
        reproducibility,
        "OwnedTemporaryDirectory",
        temporary_directory,
    )
    # Isolate the artifact cell; IR helper/aggregation and its real child remain actual.
    monkeypatch.setattr(
        reproducibility,
        "_build_repeated_and_compare",
        lambda *_a, **_k: (True, {"match": True}),
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_reproducible_build.py",
            "--batch",
            str(source),
            "--audit-ir",
            "--runs",
            "2",
            "--json-out",
            str(receipt),
        ],
    )

    assert reproducibility.main() == 2
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert (
        payload["selected"],
        payload["executed"],
        payload["passed"],
        payload["failed"],
        payload["errors"],
    ) == (2, 1, 1, 0, 1)
    result = payload["audits"][0]
    assert result["status"] == "error" and result["completed_runs"] == 1
    assert result["sha256"] == [hashlib.sha256(b"{}\n").hexdigest()]
    first, second = result["observations"]
    assert first["sha256"] == result["sha256"][0]
    if failure == "prepare":
        assert len(launches) == 1
        assert second["compiler"] is None
    else:
        assert second["compiler"]["argv"] == launches[1]
        assert second["compiler"]["environment"]["PYTHONHASHSEED"] == "1"
        failed_launch = failure in {"launch", "launch-cleanup"}
        assert second["compiler"]["status"] == (
            "error" if failed_launch else "completed"
        )
        assert second["compiler"]["child_returncode"] == (None if failed_launch else 0)
    assert "sha256" not in second
    assert ("allocation changed" if failure == "identity" else "OSError") in result[
        "error"
    ]

    if failure == "identity":
        assert len(retained_allocations) == 1
        original, retained = retained_allocations[0]
        assert (original / "replacement").read_bytes() == b"other owner"
        assert retained.is_dir()


@pytest.mark.parametrize(
    ("document", "prefer_object", "expected"),
    [
        ({"output": "app"}, False, "app"),
        ({"data": {"output": "app"}, "status": "ok"}, False, "app"),
        ({"build": {"binary": "app"}}, False, "app"),
        ({"output": "app", "artifacts": {"object": "app.o"}}, True, "app.o"),
        ({"data": {"artifacts": {"object": "app.o"}}}, True, "app.o"),
    ],
)
def test_artifact_parser_preserves_supported_output_shapes(
    document: object, prefer_object: bool, expected: str
) -> None:
    assert reproducibility.extract_artifact_path(document, prefer_object) == expected


@pytest.mark.parametrize("audit_ir", [False, True])
def test_later_batch_source_refusal_retains_prior_real_child_cell(
    tmp_path: Path, monkeypatch, audit_ir: bool
) -> None:
    good, refused = tmp_path / "good.py", tmp_path / "refused.py"
    for source in (good, refused):
        source.write_text("print('stable')\n", encoding="utf-8")
    receipt = tmp_path / "receipt.json"
    real_guard = reproducibility.harness_memory_guard.guarded_completed_process
    real_stat, real_open = Path.stat, Path.open
    launches = []

    def payload_guard(command, **kwargs):
        if command[:4] == [sys.executable, "-m", "molt.cli", "build"]:
            assert command[-1] == str(good), "refused source must never be launched"
            artifact = Path(kwargs["env"]["MOLT_CACHE"]) / "artifact"
            script = (
                "import json; from pathlib import Path; "
                f"Path({str(artifact)!r}).write_bytes(b'fixture'); "
                f"print(json.dumps({{'output': {str(artifact)!r}}}))"
            )
        elif (
            command[:2] == [sys.executable, "-c"]
            and "from molt.frontend import compile_to_tir" in command[2]
        ):
            script = "print('{}')"
        else:
            return real_guard(command, **kwargs)
        launched = real_guard([sys.executable, "-B", "-c", script], **kwargs)
        launches.append(launched.args)
        return launched

    def source_stat(path, *args, **kwargs):
        if path == refused:
            raise PermissionError("source admission refused")
        return real_stat(path, *args, **kwargs)

    def source_open(path, *args, **kwargs):
        if path == refused:
            raise PermissionError("source read refused")
        return real_open(path, *args, **kwargs)

    monkeypatch.setattr(
        reproducibility.harness_memory_guard, "guarded_completed_process", payload_guard
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_reproducible_build.py",
            "--batch",
            str(good),
            str(refused),
            *(["--audit-ir"] if audit_ir else []),
            "--json-out",
            str(receipt),
        ],
    )
    with monkeypatch.context() as admission:
        admission.setattr(Path, "stat", source_stat)
        admission.setattr(Path, "open", source_open)
        assert reproducibility.main() == 2
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    cells = 2 if audit_ir else 1
    assert (
        payload["selected"],
        payload["executed"],
        payload["passed"],
        payload["failed"],
        payload["errors"],
    ) == (2 * cells, cells, cells, 0, cells)
    completed, error = payload["results"]
    assert completed["match"] is True and completed["completed_runs"] == 2
    assert [row["build"]["argv"] for row in completed["observations"]] == launches[:2]
    assert all(
        row["build"]["status"] == "completed" for row in completed["observations"]
    )
    assert error["error_phase"] == "source"
    assert error["observations"] == [] and error["completed_runs"] == 0
    if audit_ir:
        assert [row["status"] for row in payload["audits"]] == ["pass", "error"]
        assert payload["audits"][1]["observations"] == []
        assert "source read refused" in payload["audits"][1]["error"]
    else:
        assert payload["audits"] == []


@pytest.mark.parametrize("audit_ir", [False, True])
def test_post_build_source_admission_belongs_only_to_requested_ir_cell(
    tmp_path: Path, monkeypatch, audit_ir: bool
) -> None:
    source = tmp_path / "source.py"
    source.write_text("print('stable')\n", encoding="utf-8")
    receipt = tmp_path / "receipt.json"
    real_guard = reproducibility.harness_memory_guard.guarded_completed_process
    real_is_file, real_read_text = Path.is_file, Path.read_text
    launches, post_build_preflights = [], []

    def build_guard(command, **kwargs):
        if (
            command[:2] == [sys.executable, "-c"]
            and "from molt.frontend import compile_to_tir" in command[2]
        ):
            pytest.fail("refused IR source must not launch a compiler")
        if command[:4] != [sys.executable, "-m", "molt.cli", "build"]:
            return real_guard(command, **kwargs)
        artifact = Path(kwargs["env"]["MOLT_CACHE"]) / "artifact"
        script = (
            "import json; from pathlib import Path; "
            f"Path({str(artifact)!r}).write_bytes(b'fixture'); "
            f"print(json.dumps({{'output': {str(artifact)!r}}}))"
        )
        launched = real_guard([sys.executable, "-B", "-c", script], **kwargs)
        launches.append(launched.args)
        return launched

    def is_file(path):
        if path == source and len(launches) == 2:
            post_build_preflights.append(path)
            raise PermissionError("redundant audit preflight")
        return real_is_file(path)

    def read_text(path, *args, **kwargs):
        if path == source and len(launches) == 2:
            raise PermissionError("IR source read refused")
        return real_read_text(path, *args, **kwargs)

    monkeypatch.setattr(
        reproducibility.harness_memory_guard, "guarded_completed_process", build_guard
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_reproducible_build.py",
            "--batch",
            str(source),
            *(["--audit-ir"] if audit_ir else []),
            "--json-out",
            str(receipt),
        ],
    )
    with monkeypatch.context() as admission:
        admission.setattr(Path, "is_file", is_file)
        admission.setattr(Path, "read_text", read_text)
        assert reproducibility.main() == (2 if audit_ir else 0)
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert post_build_preflights == []
    assert payload["selected"] == (2 if audit_ir else 1)
    assert payload["executed"] == payload["passed"] == 1
    assert payload["errors"] == int(audit_ir)
    assert payload["results"][0]["completed_runs"] == 2
    assert [
        row["build"]["argv"] for row in payload["results"][0]["observations"]
    ] == launches
    if audit_ir:
        assert payload["audits"][0]["status"] == "error"
        assert payload["audits"][0]["observations"] == []
        assert payload["audits"][0]["completed_runs"] == 0
    else:
        assert payload["audits"] == []


@pytest.mark.parametrize("mode", ["--build", "--batch"])
def test_source_admission_error_is_a_counted_cell_without_launch(
    tmp_path: Path, monkeypatch, mode: str
) -> None:
    source = tmp_path / "source.py"
    source.write_text("print('stable')\n", encoding="utf-8")
    receipt = tmp_path / "receipt.json"
    real_stat = Path.stat

    def source_stat(path, *args, **kwargs):
        if path == source:
            raise PermissionError()
        return real_stat(path, *args, **kwargs)

    monkeypatch.setattr(
        sys,
        "argv",
        ["check_reproducible_build.py", mode, str(source), "--json-out", str(receipt)],
    )
    with monkeypatch.context() as admission:
        admission.setattr(Path, "stat", source_stat)
        assert reproducibility.main() == 2
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert payload["selected"] == payload["errors"] == 1
    assert payload["executed"] == payload["passed"] == payload["failed"] == 0
    result = payload["results"][0]
    assert result["observations"] == [] and result["completed_runs"] == 0
    assert result["error_phase"] == "source" and "PermissionError" in result["error"]


@pytest.mark.parametrize(
    "failure", ["json-stat", "json-read", "json-utf8", "artifact-read", "artifact-stat"]
)
def test_compare_input_failures_have_one_counted_receiver_without_launch(
    tmp_path: Path, monkeypatch, failure: str
) -> None:
    import builtins

    artifact = tmp_path / "artifact"
    artifact.write_bytes(b"fixture")
    document = tmp_path / "build.json"
    document.write_text(json.dumps({"output": str(artifact)}), encoding="utf-8")
    if failure == "json-utf8":
        document.write_bytes(b"\xff")
    receipt = tmp_path / "receipt.json"
    real_open, real_stat = builtins.open, Path.stat
    metadata_calls = []

    def selected_open(file, *args, **kwargs):
        if not isinstance(file, (str, Path)):
            return real_open(file, *args, **kwargs)
        path = Path(file)
        if (failure == "json-read" and path == document) or (
            failure == "artifact-read" and path == artifact
        ):
            raise PermissionError()
        return real_open(file, *args, **kwargs)

    def selected_stat(path, *args, **kwargs):
        if failure == "json-stat" and path == document:
            metadata_calls.append(path)
            raise PermissionError("unnecessary metadata check")
        if failure == "artifact-stat" and path == artifact:
            raise PermissionError()
        return real_stat(path, *args, **kwargs)

    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_reproducible_build.py",
            str(document),
            str(document),
            "--json-out",
            str(receipt),
        ],
    )
    with monkeypatch.context() as admission:
        # Exact selected-file interception, restored before pytest/guard reporting.
        admission.setattr(builtins, "open", selected_open)
        admission.setattr(Path, "stat", selected_stat)
        assert reproducibility.main() == (0 if failure == "json-stat" else 2)
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert metadata_calls == []
    assert payload["selected"] == 1
    assert payload["passed"] == payload["executed"] == int(failure == "json-stat")
    assert payload["errors"] == int(failure != "json-stat")
    assert payload["failed"] == 0
    if failure != "json-stat":
        assert "results" not in payload
        assert payload["error_phase"] == (
            "build-json" if failure.startswith("json") else "artifact-read"
        )
    assert "observations" not in payload and "audits" not in payload
