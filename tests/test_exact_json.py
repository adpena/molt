from __future__ import annotations

import math
from pathlib import Path

import pytest

from molt import exact_json, file_publication


@pytest.mark.parametrize(
    "payload",
    (
        '{"outer":{"key":1,"key":2}}',
        '{"key":1,"key":2}',
    ),
)
def test_loads_exact_rejects_duplicate_keys_at_every_depth(payload: str) -> None:
    with pytest.raises(exact_json.ExactJsonError, match="duplicate JSON key 'key'"):
        exact_json.loads_exact(payload)


@pytest.mark.parametrize("token", ("NaN", "Infinity", "-Infinity"))
def test_loads_exact_rejects_every_non_finite_number(token: str) -> None:
    with pytest.raises(exact_json.ExactJsonError, match="non-finite JSON number"):
        exact_json.loads_exact(f'{{"value":{token}}}')


def test_exact_encoding_is_deterministic_utf8_and_finite(tmp_path: Path) -> None:
    expected = '{\n  "a": "café",\n  "z": 1\n}\n'.encode()
    assert exact_json.encode_exact({"z": 1, "a": "café"}) == expected
    assert exact_json.dumps_exact({"z": 1, "a": "café"}, indent=None) == (
        '{"a":"café","z":1}\n'
    )
    assert exact_json.canonical_json_bytes({"z": 1, "a": "café"}) == (
        '{"a":"café","z":1}'.encode()
    )
    assert exact_json.canonical_json_sha256({"z": 1, "a": "café"}) == (
        "79ab3e11fc70c4b67c474b34ff77941ed7eba4d33b123ac3e2bedd2e98dbe2bc"
    )
    with pytest.raises(ValueError, match="Out of range float values"):
        exact_json.encode_exact({"value": math.nan})
    with pytest.raises(ValueError, match="Out of range float values"):
        exact_json.canonical_json_bytes({"value": math.nan})
    with pytest.raises(ValueError, match="Out of range float values"):
        exact_json.canonical_json_sha256({"value": math.nan})

    path = tmp_path / "nested" / "identity.json"
    exact_json.write_exact(path, {"z": 1, "a": "café"}, exclusive=True)
    assert path.read_bytes() == expected
    with pytest.raises(FileExistsError):
        exact_json.write_exact(path, {}, exclusive=True)


def test_exact_write_failure_preserves_public_bytes_and_reaps_stage(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "identity.json"
    path.write_bytes(b"prior\n")

    def fail_before_commit(staged: Path, destination: Path) -> None:
        assert staged.read_bytes().startswith(b"{")
        assert destination.read_bytes() == b"prior\n"
        raise OSError("injected namespace failure")

    monkeypatch.setattr(file_publication, "durable_replace", fail_before_commit)

    with pytest.raises(OSError, match="injected namespace failure"):
        exact_json.write_exact(path, {"generation": 2})

    assert path.read_bytes() == b"prior\n"
    assert tuple(tmp_path.iterdir()) == (path,)


def test_exclusive_publication_never_replaces_an_existing_leaf(
    tmp_path: Path,
) -> None:
    staged = tmp_path / "staged"
    destination = tmp_path / "destination"
    staged.write_bytes(b"candidate")
    destination.write_bytes(b"prior")

    with pytest.raises(FileExistsError):
        file_publication.durable_publish_exclusive(staged, destination)

    assert staged.read_bytes() == b"candidate"
    assert destination.read_bytes() == b"prior"
