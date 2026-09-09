"""Section indexing is bounded metadata work for bytes and mmap consumers."""

import mmap

from molt.wasm_artifact import (
    _build_wasm_sections,
    _read_wasm_custom_section_name,
    _write_wasm_string,
    parse_wasm_section_spans,
)


def test_section_index_does_not_copy_payloads(tmp_path):
    class BoundedSlices(bytes):
        def __getitem__(self, key):
            if isinstance(key, slice):
                start, stop, step = key.indices(len(self))
                assert len(range(start, stop, step)) <= 32
            return super().__getitem__(key)

    data = _build_wasm_sections(
        [
            (0, _write_wasm_string(".debug_info") + b"x" * (1024 * 1024)),
            (1, b"x" * 1024),
        ]
    )
    expected = parse_wasm_section_spans(BoundedSlices(data))
    assert expected[0].custom_name == ".debug_info"
    path = tmp_path / "large.wasm"
    path.write_bytes(data)
    with (
        path.open("rb") as stream,
        mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as mapped,
    ):
        assert parse_wasm_section_spans(mapped) == expected


def test_custom_name_cannot_read_or_allocate_across_section_boundary():
    class NoStringSlice(bytes):
        def __getitem__(self, key):
            assert not isinstance(key, slice), "out-of-section string was copied"
            return super().__getitem__(key)

    data = NoStringSlice(b"\x08abcdefgh")
    assert _read_wasm_custom_section_name(data, limit=2) == "<unparseable>"


def test_custom_name_length_cannot_read_next_section():
    class SectionFence(bytes):
        def __getitem__(self, key):
            assert isinstance(key, int) and key == 0, "next section was read"
            return super().__getitem__(key)

    assert (
        _read_wasm_custom_section_name(SectionFence(b"\x80\x00"), limit=1)
        == "<unparseable>"
    )
