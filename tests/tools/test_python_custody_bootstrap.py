from __future__ import annotations

import json
import os
from pathlib import Path
import sys
import zipfile

import pytest

from tools.proof_queue_pkg import python_custody_bootstrap
from tests.process_guard_common import run_custody_subject_process


def _run_application(target: Path) -> dict[str, object]:
    completed = run_custody_subject_process(
        [
            sys.executable,
            str(Path(python_custody_bootstrap.__file__).resolve()),
            "script",
            "0",
            str(target),
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    assert completed.returncode == 0, completed.stderr
    return json.loads(completed.stdout)


def _main_source(target: Path) -> str:
    return (
        "import json,sys\n"
        f"target={str(target)!r}\n"
        "print(json.dumps({'path0':sys.path[0], 'count':sys.path.count(target)}))\n"
    )


def test_directory_application_owns_one_sys_path_entry(tmp_path: Path) -> None:
    application = tmp_path / "application"
    application.mkdir()
    (application / "__main__.py").write_text(
        _main_source(application), encoding="utf-8"
    )

    payload = _run_application(application)

    assert Path(str(payload["path0"])).resolve() == application.resolve()
    assert payload["count"] == 1


def test_zip_application_owns_one_sys_path_entry(tmp_path: Path) -> None:
    application = tmp_path / "application.pyz"
    with zipfile.ZipFile(application, "w") as archive:
        archive.writestr("__main__.py", _main_source(application))

    payload = _run_application(application)

    assert Path(str(payload["path0"])).resolve() == application.resolve()
    assert payload["count"] == 1


def test_safe_path_molt_module_receives_only_its_own_validated_source_root(
    tmp_path: Path,
) -> None:
    root = Path(python_custody_bootstrap.__file__).resolve().parents[2]
    environment = os.environ.copy()
    environment["MOLT_PROOF_SOURCE_ROOT"] = str(root)

    completed = run_custody_subject_process(
        [
            sys.executable,
            "-P",
            str(Path(python_custody_bootstrap.__file__).resolve()),
            "module",
            "0",
            "molt",
            "--version",
        ],
        cwd=tmp_path,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
    )

    assert completed.returncode == 0, completed.stderr
    assert completed.stdout.startswith("molt ")


def test_foreign_source_root_is_rejected_only_for_molt_module(
    tmp_path: Path,
) -> None:
    with pytest.raises(RuntimeError, match="does not match bootstrap repository"):
        with pytest.MonkeyPatch.context() as monkeypatch:
            monkeypatch.setenv("MOLT_PROOF_SOURCE_ROOT", str(tmp_path))
            python_custody_bootstrap._install_source_extension_import_root(
                "module", "molt.cli"
            )

    with pytest.MonkeyPatch.context() as monkeypatch:
        monkeypatch.setenv("MOLT_PROOF_SOURCE_ROOT", str(tmp_path))
        python_custody_bootstrap._install_source_extension_import_root(
            "module", "unrelated_module"
        )


def test_source_root_must_use_its_canonical_spelling(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    root = Path(python_custody_bootstrap.__file__).resolve().parents[2]
    monkeypatch.setenv("MOLT_PROOF_SOURCE_ROOT", f"{root}{os.sep}.")

    with pytest.raises(RuntimeError, match="must be canonical"):
        python_custody_bootstrap._install_source_extension_import_root("module", "molt")


def test_source_package_directory_cannot_escape_bootstrap_repository(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    root = Path(python_custody_bootstrap.__file__).resolve().parents[2]
    real_resolve = Path.resolve

    def resolve_with_escaped_source_package(
        self: Path, *, strict: bool = False
    ) -> Path:
        if self == root / "src":
            return tmp_path
        return real_resolve(self, strict=strict)

    monkeypatch.setenv("MOLT_PROOF_SOURCE_ROOT", str(root))
    monkeypatch.setattr(Path, "resolve", resolve_with_escaped_source_package)

    with pytest.raises(RuntimeError, match="source package directory escapes"):
        python_custody_bootstrap._install_source_extension_import_root("module", "molt")


def test_no_source_root_leaves_an_ordinary_module_import_path_unchanged(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("MOLT_PROOF_SOURCE_ROOT", raising=False)
    before = list(sys.path)

    python_custody_bootstrap._install_source_extension_import_root(
        "module", "json.tool"
    )

    assert sys.path == before
