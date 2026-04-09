"""Tests for CLI flag -> manifest -> env var integration."""
import sys
import os
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'src'))


def test_parse_audit_log_flag():
    from molt.cli import _parse_audit_log_flag
    env = _parse_audit_log_flag("jsonl:stderr")
    assert env["MOLT_AUDIT_ENABLED"] == "1"
    assert env["MOLT_AUDIT_SINK"] == "jsonl"
    assert env["MOLT_AUDIT_OUTPUT"] == "stderr"


def test_parse_audit_log_flag_default_output():
    from molt.cli import _parse_audit_log_flag
    env = _parse_audit_log_flag("stderr")
    assert env["MOLT_AUDIT_ENABLED"] == "1"
    assert env["MOLT_AUDIT_SINK"] == "stderr"
    assert env["MOLT_AUDIT_OUTPUT"] == "stderr"


def test_parse_audit_log_flag_rejects_invalid_sink():
    from molt.cli import _parse_audit_log_flag
    try:
        _parse_audit_log_flag("../../../etc/passwd:stderr")
        assert False, "Should have raised ValueError for invalid sink"
    except ValueError as e:
        assert "Invalid audit sink" in str(e)


def test_parse_audit_log_flag_accepts_all_valid_sinks():
    from molt.cli import _parse_audit_log_flag, _VALID_AUDIT_SINKS
    for sink in _VALID_AUDIT_SINKS:
        env = _parse_audit_log_flag(f"{sink}:stderr")
        assert env["MOLT_AUDIT_SINK"] == sink


def test_build_slot_dir_defaults_to_repo_tmp(monkeypatch, tmp_path: Path):
    from molt import cli

    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    monkeypatch.setattr(cli, "_find_molt_root", lambda _cwd: tmp_path)

    assert cli._build_slot_dir() == tmp_path / "tmp" / "molt-build-slots"


def test_build_slot_dir_prefers_ext_root(monkeypatch, tmp_path: Path):
    from molt import cli

    ext_root = tmp_path / "external"
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ext_root))

    assert cli._build_slot_dir() == ext_root / "tmp" / "molt-build-slots"


def test_parse_io_mode_flag_virtual():
    from molt.cli import _parse_io_mode_flag
    env = _parse_io_mode_flag("virtual")
    assert env["MOLT_IO_MODE"] == "virtual"


def test_parse_io_mode_flag_real():
    from molt.cli import _parse_io_mode_flag
    env = _parse_io_mode_flag("real")
    assert "MOLT_IO_MODE" not in env  # real is default, no env var needed


def test_parse_io_mode_flag_invalid():
    from molt.cli import _parse_io_mode_flag
    try:
        _parse_io_mode_flag("invalid")
        assert False, "Should have raised ValueError"
    except ValueError:
        pass


def test_parse_type_gate_flag():
    from molt.cli import _parse_type_gate_flag
    assert _parse_type_gate_flag(True) == {"MOLT_TYPE_GATE": "1"}
    assert _parse_type_gate_flag(False) == {}
