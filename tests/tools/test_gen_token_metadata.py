"""Pinned-source oracles and failure controls for target token metadata."""

from __future__ import annotations

import ast
from dataclasses import replace
import re
import pytest

from molt.target_python import SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS
from tools import gen_stdlib_module_union as sources
from tools import gen_token_metadata as gen


@pytest.fixture(scope="module")
def snapshots() -> dict[str, sources.SourceSnapshot]:
    return sources.pinned_snapshots(SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS)


def _module_oracle(
    snapshot: sources.SourceSnapshot,
) -> tuple[tuple[tuple[str, int], ...], tuple[tuple[str, str], ...], tuple[str, ...]]:
    """Read Lib/token.py independently of the grammar-based producer.

    Only literal declarations enter this oracle; no pinned source is executed
    and no host interpreter's token numbering supplies an expectation.
    """
    module = next(item.text for item in snapshot.inputs if item.role == "token_module")
    declarations = {
        node.targets[0].id: node.value
        for node in ast.parse(module).body
        if isinstance(node, ast.Assign)
        and len(node.targets) == 1
        and isinstance(node.targets[0], ast.Name)
    }
    constants = tuple(
        (name, ast.literal_eval(value))
        for name, value in declarations.items()
        if name.isupper()
        and isinstance(value, ast.Constant)
        and type(value.value) is int
    )
    operators = declarations["EXACT_TOKEN_TYPES"]
    assert isinstance(operators, ast.Dict)
    exact = tuple(
        (ast.literal_eval(symbol), token.id)
        for symbol, token in zip(operators.keys, operators.values)
        if isinstance(token, ast.Name)
    )
    assert len(exact) == len(operators.keys)
    return constants, exact, tuple(ast.literal_eval(declarations["__all__"]))


def _edit_source(
    snapshot: sources.SourceSnapshot, role: str, before: str, after: str
) -> sources.SourceSnapshot:
    item = next(item for item in snapshot.inputs if item.role == role)
    assert before in item.text, (role, before)
    changed = replace(item, text=item.text.replace(before, after, 1))
    return replace(
        snapshot,
        inputs=tuple(
            changed if original is item else original for original in snapshot.inputs
        ),
    )


@pytest.mark.parametrize("version", ["3.12", "3.13", "3.14"])
def test_all_token_projections_match_the_pinned_module_oracle(
    snapshots, version
) -> None:
    snapshot = snapshots[version]
    metadata = gen.derive_metadata(snapshot)
    constants, exact, exports = _module_oracle(snapshot)
    assert metadata.python == version
    assert metadata.constants == constants
    assert metadata.exact_types == exact
    assert metadata.base_exports == exports
    assert dict(constants)["COMMENT"] == {"3.12": 64, "3.13": 62, "3.14": 65}[version]
    assert dict(exact)["("] == "LPAR"
    assert dict(exact)[":="] == "COLONEQUAL"


def test_source_is_parsed_without_executing_the_pinned_module(snapshots) -> None:
    snapshot = snapshots["3.12"]
    changed = _edit_source(
        snapshot,
        "token_module",
        "# Auto-generated",
        'raise AssertionError("pinned source must never execute")\n# Auto-generated',
    )
    assert gen.derive_metadata(changed) == gen.derive_metadata(snapshot)


def test_projection_tracks_semantics_without_repeating_source_provenance(
    snapshots, tmp_path, monkeypatch
) -> None:
    snapshot = snapshots["3.12"]
    monkeypatch.setattr(sources, "PINS_PATH", tmp_path / "pins.toml")
    monkeypatch.setattr(sources, "SNAPSHOT_PATH", tmp_path / "snapshot.json")
    monkeypatch.setattr(gen, "SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS", ("3.12",))
    monkeypatch.setattr(gen, "OUTPUT", tmp_path / "tokens.rs")

    def no_network(*_args, **_kwargs):
        pytest.fail("semantic projection must use the admitted source receipt")

    monkeypatch.setattr(sources, "_git", no_network)

    def project(current):
        # Re-seal edited source bytes, then use the real receipt/pin admission.
        current = replace(
            current,
            inputs=tuple(
                replace(
                    item,
                    git_object=sources._git_blob_id(item.text.encode("utf-8")),
                )
                if item.role != "public_headers"
                else item
                for item in current.inputs
            ),
        )
        sources.PINS_PATH.write_text(
            'schema = "molt.cpython-regrtest-sources.v1"\n[[source]]\n'
            f'python = "{current.python}"\ntag = "{current.tag}"\n'
            f'revision = "{current.revision}"\n'
            'git_url = "https://github.com/python/cpython.git"\n',
            encoding="utf-8",
        )
        sources.SNAPSHOT_PATH.write_text(
            sources.render_snapshot({current.python: current}), encoding="utf-8"
        )
        assert sources.pinned_snapshots((current.python,))[current.python] == current
        return gen.generated_outputs()[gen.OUTPUT]

    before = project(snapshot)
    micro = int(snapshot.tag.rsplit(".", 1)[1]) + 1
    tag = f"v{snapshot.python}.{micro}"
    release = next(item for item in snapshot.inputs if item.role == "release")
    release_text, replacements = re.subn(
        r"(?m)^(#define\s+PY_MICRO_VERSION\s+)\d+\b",
        lambda match: match[1] + str(micro),
        release.text,
    )
    assert replacements == 1
    release_text = release_text.replace(
        snapshot.tag.removeprefix("v"), tag.removeprefix("v")
    )
    advanced = replace(
        snapshot,
        tag=tag,
        revision="f" * 40,
        inputs=tuple(
            replace(item, text=release_text) if item is release else item
            for item in snapshot.inputs
        ),
    )
    assert advanced.revision != snapshot.revision
    assert _module_oracle(advanced) == _module_oracle(snapshot)
    assert project(advanced) == before

    changed = _edit_source(
        advanced, "token_generator", "NT_OFFSET = 256", "NT_OFFSET = 257"
    )
    changed = _edit_source(
        changed, "token_module", "NT_OFFSET = 256", "NT_OFFSET = 257"
    )
    assert dict(_module_oracle(changed)[0])["NT_OFFSET"] == 257
    assert project(changed) != before


@pytest.mark.parametrize(
    ("role", "before", "after", "message"),
    [
        ("token_grammar", "ENDMARKER", "ENDMARKER\nENDMARKER", "duplicate.*name"),
        ("token_grammar", "ENDMARKER", "lowercase", "invalid.*definition"),
        (
            "token_grammar",
            "LPAR                    '('",
            "LPAR                    1",
            "operator",
        ),
        (
            "token_grammar",
            "RPAR                    ')'",
            "RPAR                    '('",
            "operator",
        ),
        ("token_module", "ENDMARKER = 0", "ENDMARKER = 1", "Grammar/Tokens.*disagree"),
        (
            "token_module",
            "ENDMARKER = 0",
            "ENDMARKER = True",
            "Grammar/Tokens.*disagree",
        ),
        ("token_module", "'(': LPAR", "'(': RPAR", "operator projection disagrees"),
        ("token_module", "'(': LPAR", "'(': 7", "must name a declared constant"),
        (
            "token_module",
            "EXACT_TOKEN_TYPES = {",
            "EXACT_TOKEN_TYPES = ()\ndiscarded = {",
            "must be a dictionary",
        ),
        (
            "token_generator",
            "NT_OFFSET = 256",
            "NT_OFFSET = True",
            "offset.*representable",
        ),
        (
            "token_generator",
            "NT_OFFSET = 256",
            "NT_OFFSET = 1",
            "offset.*representable",
        ),
        (
            "token_generator",
            "NT_OFFSET = 256",
            "NT_OFFSET = 32768",
            "offset.*representable",
        ),
    ],
)
def test_inconsistent_or_unrepresentable_source_fails_closed(
    snapshots, role: str, before: str, after: str, message: str
) -> None:
    changed = _edit_source(snapshots["3.12"], role, before, after)
    with pytest.raises(ValueError, match=message):
        gen.derive_metadata(changed)


def test_duplicate_module_assignments_cannot_hide_disagreement(snapshots) -> None:
    changed = _edit_source(
        snapshots["3.12"],
        "token_module",
        "ENDMARKER = 0",
        "ENDMARKER = 1\nENDMARKER = 0",
    )
    with pytest.raises(ValueError, match="duplicate CPython assignment: ENDMARKER"):
        gen.derive_metadata(changed)


def test_empty_grammar_and_invalid_base_exports_are_rejected(snapshots) -> None:
    snapshot = snapshots["3.12"]
    grammar = next(item for item in snapshot.inputs if item.role == "token_grammar")
    empty = replace(
        snapshot,
        inputs=tuple(
            replace(item, text="# empty\n") if item is grammar else item
            for item in snapshot.inputs
        ),
    )
    with pytest.raises(ValueError, match="grammar is empty"):
        gen.derive_metadata(empty)
    changed = _edit_source(snapshot, "token_module", "__all__ = [", "__all__ = [7,")
    with pytest.raises(ValueError, match="base exports"):
        gen.derive_metadata(changed)


@pytest.mark.parametrize("collision", [False, True])
def test_render_refuses_an_unrepresentable_closed_token_domain(
    snapshots, collision
) -> None:
    metadata = gen.derive_metadata(snapshots["3.12"])
    constants = (
        (("A_B", 0), ("A__B", 1))
        if collision
        else tuple((f"TOKEN_{number}", number) for number in range(257))
    )
    with pytest.raises(ValueError, match="closed u8 domain"):
        gen.render((replace(metadata, constants=constants),))


def test_generated_output_is_current_idempotent_and_excludes_pending_sources(
    monkeypatch,
) -> None:
    captured = sources.load_snapshot(sources.SNAPSHOT_PATH)
    assert "3.15" in captured
    requested = []
    pinned = sources.pinned_snapshots

    def admitted(versions):
        requested.append(tuple(versions))
        return pinned(versions)

    def no_network(*_args, **_kwargs):
        raise AssertionError("token generation must use committed source receipts")

    monkeypatch.setattr(sources, "pinned_snapshots", admitted)
    monkeypatch.setattr(sources, "_git", no_network)
    first = gen.generated_outputs()
    assert first == gen.generated_outputs()
    assert requested == [SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS] * 2
    assert set(first) == {gen.OUTPUT}
    assert gen.OUTPUT.read_bytes() == first[gen.OUTPUT].encode("utf-8")
    assert "(3, 15) =>" not in first[gen.OUTPUT]
    for version in SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS:
        assert f"// CPython {version}." in first[gen.OUTPUT]
        assert captured[version].revision not in first[gen.OUTPUT]
