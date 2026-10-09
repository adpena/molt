import os
import pytest
from tools.compat import test_policy
from tests.process_guard_common import install_module_view


@pytest.mark.parametrize("separator", [":", ";"])
def test_pythonpath_header_is_repo_relative_and_host_separator_portable(
    tmp_path, monkeypatch, separator
):
    (tmp_path / "src").mkdir()
    (tmp_path / "tests").mkdir()
    source = tmp_path / "test.py"
    source.write_text("# MOLT_ENV: PYTHONPATH=src:tests\n", encoding="utf-8")
    install_module_view(monkeypatch, "os", os, test_policy, pathsep=separator)
    assert test_policy.collect_environment_overrides(source, repo_root=tmp_path)[
        "PYTHONPATH"
    ] == separator.join(str(tmp_path / part) for part in ("src", "tests"))


def test_missing_pythonpath_entry_fails_closed(tmp_path):
    source = tmp_path / "test.py"
    source.write_text("# MOLT_ENV: PYTHONPATH=missing\n", encoding="utf-8")
    with pytest.raises(FileNotFoundError):
        test_policy.collect_environment_overrides(source, repo_root=tmp_path)
