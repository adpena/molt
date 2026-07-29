from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys
import zipfile

from tools.proof_queue_pkg import python_custody_bootstrap


def _run_application(target: Path) -> dict[str, object]:
    completed = subprocess.run(
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
