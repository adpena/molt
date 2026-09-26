from __future__ import annotations

from pathlib import Path

import pytest

from molt.cli.link_member_selection import (
    LinkMemberSelectionError,
    selected_archive_members,
)
from molt.cli.native_symbol_inspection import (
    _NativeArchiveMemberSymbolFacts,
    _NativeGlobalSymbolFacts,
)
from molt.cli.static_archive_identity import (
    StaticArchiveMember,
    StaticArchiveMemberIdentity,
)


def _facts(*names: str) -> _NativeGlobalSymbolFacts:
    empty = _NativeGlobalSymbolFacts(frozenset(), frozenset(), frozenset())
    return _NativeGlobalSymbolFacts(
        frozenset(),
        frozenset(),
        frozenset(),
        members=tuple(
            _NativeArchiveMemberSymbolFacts(
                StaticArchiveMemberIdentity(
                    ordinal,
                    StaticArchiveMember(name, ordinal * 16, 16),
                    f"{ordinal + 1:064x}",
                ),
                empty,
            )
            for ordinal, name in enumerate(names)
        ),
    )


def test_gnu_trace_selects_only_extracted_ordinals_with_parenthesized_archive_path(
    tmp_path: Path,
) -> None:
    archive = (tmp_path / "dependency (space).a").resolve()
    entry = (tmp_path / "entry.o").resolve()
    selected = selected_archive_members(
        {archive: _facts("needed.o", "dormant.o")},
        dialect="elf-gnu",
        stdout=f"{entry}\n{archive}(needed.o)\n",
        stderr="",
        why_extract=(
            f"reference\textracted\tsymbol\n{entry}\t{archive}(needed.o)\tdependency\n"
        ),
    )
    assert selected == {archive: (0,)}


def test_wasm_lazy_trace_only_member_disagrees_with_why_extract(
    tmp_path: Path,
) -> None:
    archive = (tmp_path / "package.a").resolve()
    entry = (tmp_path / "entry.wasm").resolve()
    with pytest.raises(LinkMemberSelectionError, match="disagree"):
        selected_archive_members(
            {archive: _facts("needed.o", "constructor.o", "dormant.o")},
            dialect="wasm",
            stdout=f"{entry}\n{archive}(needed.o)\n{archive}(constructor.o)\n",
            stderr="",
            why_extract=(
                f"reference\textracted\tsymbol\n{entry}\t{archive}(needed.o)\tdependency\n"
            ),
        )


def test_gnu_why_extract_must_agree_with_trace(tmp_path: Path) -> None:
    archive = (tmp_path / "package.a").resolve()
    entry = (tmp_path / "entry.o").resolve()
    with pytest.raises(LinkMemberSelectionError, match="disagree"):
        selected_archive_members(
            {archive: _facts("needed.o")},
            dialect="elf-gnu",
            stdout=f"{entry}\n",
            stderr="",
            why_extract=(
                "reference\textracted\tsymbol\n"
                f"{entry}\t{archive}(needed.o)\tdependency\n"
            ),
        )
    with pytest.raises(LinkMemberSelectionError, match="disagree"):
        selected_archive_members(
            {archive: _facts("needed.o")},
            dialect="elf-gnu",
            stdout=f"{entry}\n{archive}(needed.o)\n",
            stderr="",
            why_extract="reference\textracted\tsymbol\n",
        )


def test_macho_trace_selects_only_exact_full_path_member(tmp_path: Path) -> None:
    archive = (tmp_path / "dependency (space).a").resolve()
    entry = (tmp_path / "entry.o").resolve()
    selected = selected_archive_members(
        {archive: _facts("needed.o", "dormant.o")},
        dialect="macho",
        stdout=(
            f"{entry}\n{archive}(needed.o)\n"
            f"_dependency forced load of {archive}(needed.o)\n"
        ),
        stderr="",
    )
    assert selected == {archive: (0,)}


def test_full_path_matching_preserves_case_distinct_archive_identity(
    tmp_path: Path,
) -> None:
    archive = (tmp_path / "Package.a").resolve()
    other_spelling = tmp_path / "package.a"
    if str(other_spelling.resolve(strict=False)) == str(archive):
        pytest.skip("host resolves these spellings to the same canonical archive")
    entry = (tmp_path / "entry.o").resolve()
    assert selected_archive_members(
        {archive: _facts("needed.o")},
        dialect="macho",
        stdout=f"{entry}\n{other_spelling}(needed.o)\n",
        stderr="",
    ) == {archive: ()}


def test_coff_loaded_member_requires_unique_full_archive_read(tmp_path: Path) -> None:
    archive = (tmp_path / "dependency (space).lib").resolve()
    entry = (tmp_path / "entry.obj").resolve()
    basename_member = f"{archive.name}(needed.obj)"
    trace = (
        f"lld-link: Reading {entry}\n"
        f"lld-link: Reading {archive}\n"
        f"lld-link: Reading {basename_member}\n"
        f"lld-link: Loaded {basename_member} for dependency\n"
    )
    assert selected_archive_members(
        {archive: _facts("needed.obj", "dormant.obj")},
        dialect="coff-msvc",
        stdout="",
        stderr=trace,
    ) == {archive: (0,)}
    foreign = (tmp_path / "foreign" / archive.name).resolve()
    with pytest.raises(LinkMemberSelectionError, match="ambiguous selected COFF"):
        selected_archive_members(
            {archive: _facts("needed.obj", "dormant.obj")},
            dialect="coff-msvc",
            stdout="",
            stderr=trace + f"lld-link: Reading {foreign}\n",
        )


def test_dormant_duplicate_names_are_allowed_but_selected_duplicates_fail(
    tmp_path: Path,
) -> None:
    archive = (tmp_path / "package.a").resolve()
    entry = (tmp_path / "entry.o").resolve()
    assert selected_archive_members(
        {archive: _facts("same.o", "same.o")},
        dialect="macho",
        stdout=f"{entry}\n",
        stderr="",
    ) == {archive: ()}
    with pytest.raises(LinkMemberSelectionError, match="ambiguous selected member"):
        selected_archive_members(
            {archive: _facts("same.o", "same.o")},
            dialect="macho",
            stdout=f"{entry}\n{archive}(same.o)\n",
            stderr="",
        )


def test_coff_path_named_members_and_dormant_basename_collision(tmp_path: Path) -> None:
    archive = (tmp_path / "package.lib").resolve()
    dormant = (tmp_path / "other" / "package.lib").resolve()
    label = f"{archive.name}(needed.obj)"
    trace = (
        f"lld-link: Reading {archive}\n"
        f"lld-link: Reading {label}\n"
        f"lld-link: Loaded {label} for dependency\n"
    )
    assert selected_archive_members(
        {
            archive: _facts("build/needed.obj", "dormant.obj"),
            dormant: _facts("unused.obj"),
        },
        dialect="coff-msvc",
        stdout="",
        stderr=trace,
    ) == {archive: (0,), dormant: ()}
    with pytest.raises(LinkMemberSelectionError, match="ambiguous selected COFF"):
        selected_archive_members(
            {
                archive: _facts("build/needed.obj"),
                dormant: _facts("other/needed.obj"),
            },
            dialect="coff-msvc",
            stdout="",
            stderr=trace + f"lld-link: Reading {dormant}\n",
        )
    with pytest.raises(LinkMemberSelectionError, match="ambiguous selected member"):
        selected_archive_members(
            {archive: _facts("left/needed.obj", "right/needed.obj")},
            dialect="coff-msvc",
            stdout="",
            stderr=trace,
        )
    foreign_only = (
        f"lld-link: Reading {dormant}\n"
        f"lld-link: Reading {label}\n"
        f"lld-link: Loaded {label} for dependency\n"
    )
    assert selected_archive_members(
        {archive: _facts("needed.obj")},
        dialect="coff-msvc",
        stdout="",
        stderr=foreign_only,
    ) == {archive: ()}


def test_unknown_members_malformed_tracked_labels_and_empty_evidence_fail_closed(
    tmp_path: Path,
) -> None:
    archive = (tmp_path / "package.a").resolve()
    entry = (tmp_path / "entry.o").resolve()
    with pytest.raises(LinkMemberSelectionError, match="unknown selected member"):
        selected_archive_members(
            {archive: _facts("needed.o")},
            dialect="macho",
            stdout=f"{entry}\n{archive}(unknown.o)\n",
            stderr="",
        )
    with pytest.raises(LinkMemberSelectionError, match="unrecognized selected member"):
        selected_archive_members(
            {archive: _facts("needed.o")},
            dialect="macho",
            stdout=f"{entry}\n{archive}[needed.o]\n",
            stderr="",
        )
    with pytest.raises(LinkMemberSelectionError, match="non-absolute selected"):
        selected_archive_members(
            {archive: _facts("needed.o")},
            dialect="elf-gnu",
            stdout=f"{entry}\n",
            stderr="",
            why_extract=(
                "reference\textracted\tsymbol\n"
                f"{entry}\t{archive.name}(needed.o)\tdependency\n"
            ),
        )
    with pytest.raises(LinkMemberSelectionError, match="header"):
        selected_archive_members(
            {archive: _facts("needed.o")},
            dialect="elf-gnu",
            stdout=f"{entry}\n",
            stderr="",
            why_extract="",
        )
    with pytest.raises(LinkMemberSelectionError, match="no absolute input-read"):
        selected_archive_members(
            {archive: _facts("needed.o")},
            dialect="macho",
            stdout="",
            stderr="",
        )
    with pytest.raises(LinkMemberSelectionError, match="unsupported selection dialect"):
        selected_archive_members(
            {archive: _facts("needed.o")},
            dialect="coff-gnu",
            stdout=f"{entry}\n",
            stderr="",
        )
