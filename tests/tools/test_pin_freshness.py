from __future__ import annotations

from datetime import UTC, datetime, timedelta
import hashlib
import io
import json
import re
import shutil
import tarfile
import tomllib
import zipfile
from pathlib import Path

import pytest

from molt import tool_releases
from molt.binaryen_toolchain import BinaryenConfigError, load_binaryen_manifest
from molt.llvm_toolchain import load_llvm_releases
from molt.verified_subset import load_verified_subset_policy
from tests.process_guard_common import run_guarded_test_process
from tools import finding_status, pin_freshness
from tools.pin_freshness import Pin, PinFreshnessError, Release

ROOT = Path(__file__).resolve().parents[2]
PIN_AUTHORITIES = (
    "config/tool_releases.toml",
    "config/llvm_toolchain_releases.toml",
    "config/binaryen_releases.toml",
    "config/release_supply_chain.toml",
    "formal/lean/lean-toolchain",
    "rust-toolchain.toml",
    "tools/proof_plan.toml",
    "config/verified_subset.toml",
)
NOW = datetime(2026, 10, 22, 12, 0, tzinfo=UTC)
WEEK = pin_freshness.MIN_RELEASE_AGE


def _ago(days: float) -> datetime:
    return NOW - timedelta(days=days)


def _advisories(table: dict[str, tuple[str, ...]]):
    """A per-version advisory feed answering from `table` (advisory IDs)."""
    return lambda versions: {
        v: tuple(pin_freshness.Advisory(i, (), "") for i in table.get(v, ()))
        for v in versions
    }


def _pin(
    name: str,
    current: str,
    *releases: tuple[str, float],
    affected: dict[str, tuple[str, ...]] | None = None,
) -> Pin:
    """A pin whose upstream published each (version, days ago) release."""
    listed = [Release(version, _ago(days)) for version, days in releases]
    feed = _advisories(affected) if affected is not None else None
    return Pin(name, "authority", current, lambda: listed, feed)


def _check(pins, open_rows=frozenset(), holds=None) -> tuple[int, str]:
    out = io.StringIO()
    failures = pin_freshness.check_pins(
        pins, set(open_rows), now=NOW, holds={} if holds is None else holds, out=out
    )
    return failures, out.getvalue()


def test_versions_compare_numerically() -> None:
    key = pin_freshness.version_key
    assert key("1.261.0") > key("1.259.0")
    assert key("26.10.0") > key("24.16.0")
    assert key("34") > key("33.0")
    assert key("133") > key("130")
    assert key("3.14") > key("3.12")
    with pytest.raises(PinFreshnessError):
        key("latest")


def test_check_passes_when_every_pin_is_current() -> None:
    failures, out = _check([_pin("uv", "0.12.23", ("0.12.22", 40), ("0.12.23", 30))])
    assert failures == 0
    assert "current uv: 0.12.23" in out


def test_check_fails_on_a_pin_below_its_newest_aged_release() -> None:
    failures, out = _check([_pin("uv", "0.11.24", ("0.11.24", 60), ("0.12.23", 30))])
    assert failures == 1
    assert "STALE   uv: 0.11.24 -> 0.12.23 (authority)" in out


def test_a_release_younger_than_seven_days_waits_with_its_date() -> None:
    failures, out = _check([_pin("uv", "0.12.23", ("0.12.23", 30), ("0.12.24", 2))])
    assert failures == 0
    assert (
        "WAITING uv: 0.12.23; 0.12.24 (public 2026-10-20) "
        "waits until 2026-10-27T12:00:00Z"
    ) in out


def test_a_stale_pin_moves_only_to_the_newest_aged_release() -> None:
    pin = _pin("lune", "0.10.5", ("0.10.5", 100), ("0.11.0", 10), ("0.11.1", 1))
    failures, out = _check([pin])
    assert failures == 1
    assert (
        "STALE   lune: 0.10.5 -> 0.11.0 (authority); 0.11.1 (public 2026-10-21) "
        "waits until 2026-10-28T12:00:00Z"
    ) in out


def test_the_age_rule_admits_a_release_at_exactly_seven_days() -> None:
    def target(published: datetime) -> str | None:
        releases = [Release("1.0.0", _ago(30)), Release("1.1.0", published)]
        return pin_freshness.select_target("1.0.0", releases, NOW).version

    assert target(NOW - WEEK) == "1.1.0"
    assert target(NOW - WEEK + timedelta(seconds=1)) is None


def test_no_aged_release_leaves_the_pin_where_it_is() -> None:
    failures, out = _check([_pin("uv", "0.13.4", ("0.13.4", 10), ("0.13.5", 1))])
    assert failures == 0
    assert "WAITING uv: 0.13.4; 0.13.5" in out


def test_an_advisory_moves_at_once_to_the_oldest_clean_release() -> None:
    pin = _pin(
        "uv",
        "1.0.0",
        ("1.0.0", 60),
        ("1.0.1", 1),
        ("1.0.2", 0.5),
        affected={"1.0.0": ("GHSA-aaaa",)},
    )
    failures, out = _check([pin])
    assert failures == 1
    assert "ADVISORY uv: 1.0.0 -> 1.0.1 fixes GHSA-aaaa (authority); 1.0.2" in out


def test_an_advisory_move_skips_releases_that_still_carry_it() -> None:
    # 1.0.1 fixes one advisory but not the other; 1.0.3 is the oldest clean.
    target = pin_freshness.select_target(
        "1.0.0",
        [
            Release("1.0.0", _ago(60)),
            Release("1.0.1", _ago(3)),
            Release("1.0.3", _ago(2)),
            Release("1.0.5", _ago(1)),
        ],
        NOW,
        _advisories(
            {
                "1.0.0": ("GHSA-aaaa", "RUSTSEC-2026-0001"),
                "1.0.1": ("RUSTSEC-2026-0001",),
            }
        ),
    )
    assert target.version == "1.0.3"
    assert target.fixes == ("GHSA-aaaa", "RUSTSEC-2026-0001")


def test_the_target_is_never_a_release_that_carries_an_advisory() -> None:
    # One advisory affects [0, 1.5.0) and [2.0.0, 2.3.0): the newest aged
    # release, 2.1.0, carries it again, so the move stops at 1.5.0.
    target = pin_freshness.select_target(
        "1.2.0",
        [
            Release("1.2.0", _ago(90)),
            Release("1.5.0", _ago(60)),
            Release("2.1.0", _ago(20)),
            Release("2.3.0", _ago(1)),
        ],
        NOW,
        _advisories({"1.2.0": ("GHSA-xxxx",), "2.1.0": ("GHSA-xxxx",)}),
    )
    assert (target.version, target.fixes) == ("1.5.0", ("GHSA-xxxx",))
    assert target.waiting is not None and target.waiting.version == "2.3.0"
    clean = pin_freshness.select_target(
        "1.5.0",
        [Release("1.5.0", _ago(60)), Release("2.1.0", _ago(20))],
        NOW,
        _advisories({"2.1.0": ("GHSA-xxxx",)}),
    )
    assert clean.version is None


def test_a_newer_aged_clean_release_beats_the_oldest_fix() -> None:
    target = pin_freshness.select_target(
        "1.0.0",
        [
            Release("1.0.0", _ago(60)),
            Release("1.0.1", _ago(40)),
            Release("1.2.0", _ago(20)),
        ],
        NOW,
        _advisories({"1.0.0": ("GHSA-aaaa",)}),
    )
    assert (target.version, target.fixes) == ("1.2.0", ("GHSA-aaaa",))


def test_a_security_release_moves_its_line_at_once() -> None:
    releases = [
        Release("24.12.0", _ago(1), security=True),
        Release("26.12.0", _ago(30)),
        Release("26.12.1", _ago(1), security=True),
    ]
    target = pin_freshness.select_target("26.12.0", releases, NOW)
    assert (target.version, target.fixes) == (
        "26.12.1",
        ("security release 26.12.1",),
    )
    # A security release on another line leaves this line alone: 27.0.1 is
    # a newer line's fix, so 26.12.1 waits for it to age like any release.
    newer_line = [
        Release("26.12.1", _ago(30)),
        Release("27.0.1", _ago(1), security=True),
    ]
    other = pin_freshness.select_target("26.12.1", newer_line, NOW)
    assert (other.version, other.fixes) == (None, ())
    assert other.waiting == newer_line[1]


def test_no_hold_excuses_an_advisory() -> None:
    pin = _pin(
        "uv",
        "1.0.0",
        ("1.0.0", 60),
        ("1.0.1", 1),
        affected={"1.0.0": ("GHSA-aaaa",)},
    )
    failures, out = _check([pin], {"HF-62"}, {"uv": "HF-62"})
    assert failures == 1
    assert "ADVISORY uv: 1.0.0 -> 1.0.1 fixes GHSA-aaaa" in out
    assert "HELD" not in out


def test_an_advisory_no_release_fixes_fails() -> None:
    pin = _pin(
        "uv",
        "1.0.0",
        ("1.0.0", 60),
        ("1.0.1", 2),
        affected={"1.0.0": ("PYSEC-2026-1",), "1.0.1": ("PYSEC-2026-1",)},
    )
    failures, out = _check([pin])
    assert failures == 1
    assert "ADVISORY uv: 1.0.0 carries PYSEC-2026-1, which no release fixes" in out


def test_a_hold_on_an_open_row_reports_without_failing() -> None:
    failures, out = _check(
        [_pin("llvm", "22.1.8", ("22.1.8", 90), ("23.1.3", 30))],
        {"HF-64"},
        {"llvm": "HF-64"},
    )
    assert failures == 0
    assert "HELD    llvm: 22.1.8 -> 23.1.3 (HF-64)" in out


def test_a_hold_stays_valid_while_the_newest_release_waits_on_age() -> None:
    failures, out = _check(
        [_pin("uv", "0.13.4", ("0.13.4", 10), ("0.13.5", 1))],
        {"HF-62"},
        {"uv": "HF-62"},
    )
    assert failures == 0
    assert "HELD    uv: 0.13.4 -> 0.13.5 (HF-62); 0.13.5" in out


def test_a_hold_must_cite_an_open_row() -> None:
    failures, out = _check(
        [_pin("llvm", "22.1.8", ("22.1.8", 90), ("23.1.3", 30))],
        set(),
        {"llvm": "HF-64"},
    )
    assert failures == 1
    assert "hold cites HF-64, not an open ledger row" in out


def test_a_hold_on_a_pin_at_its_newest_release_must_be_dropped() -> None:
    failures, out = _check(
        [_pin("llvm", "23.1.3", ("23.1.3", 30))], {"HF-64"}, {"llvm": "HF-64"}
    )
    assert failures == 1
    assert "current at 23.1.3; drop its hold" in out


def test_a_hold_must_name_a_known_pin() -> None:
    failures, out = _check([], {"HF-64"}, {"llvm": "HF-64"})
    assert failures == 1
    assert "hold names unknown pin 'llvm'" in out


def test_repository_holds_cite_open_ledger_rows() -> None:
    open_rows = finding_status.read_ledger(ROOT).open_keys
    for name, row in pin_freshness.HOLDS.items():
        assert row in open_rows, (name, row)


def test_github_release_lists_keep_only_stable_published_releases() -> None:
    def record(tag: str, **flags: object) -> dict[str, object]:
        return {"tag_name": tag, "published_at": "2026-10-12T18:13:12Z", **flags}

    def fetch(url: str) -> object:
        assert url == "https://api.github.com/repos/o/r/releases?per_page=100"
        return [
            record("v4.36.0-rc1", prerelease=True),
            record("v4.36.0", prerelease=True),  # a stable spelling, flagged
            record("v4.36.0-rc2"),  # not flagged, but not a stable spelling
            record("v4.37.0", draft=True),
            record("nightly-2026-10-20"),
            record("v4.35.0"),
            {"tag_name": "v4.35.1", "published_at": None},
        ]

    assert pin_freshness.github_releases("o/r", "v", fetch) == [
        Release("4.35.0", datetime(2026, 10, 12, 18, 13, 12, tzinfo=UTC))
    ]


def test_node_releases_carry_the_index_security_flag() -> None:
    def fetch(url: str) -> object:
        assert url == "https://nodejs.org/dist/index.json"
        return [
            {"version": "v26.12.1", "date": "2026-10-21", "security": True},
            {"version": "v26.12.0", "date": "2026-10-01", "security": False},
        ]

    assert pin_freshness.node_releases(fetch) == [
        Release("26.12.1", datetime(2026, 10, 22, tzinfo=UTC), security=True),
        Release("26.12.0", datetime(2026, 10, 2, tzinfo=UTC), security=False),
    ]


def test_a_date_only_record_counts_from_the_end_of_its_day() -> None:
    assert pin_freshness.parse_time("2026-10-14") == datetime(2026, 10, 15, tzinfo=UTC)
    assert pin_freshness.parse_time("2026-10-14T05:00:00+02:00") == datetime(
        2026, 10, 14, 3, tzinfo=UTC
    )
    with pytest.raises(PinFreshnessError, match="no time zone"):
        pin_freshness.parse_time("2026-10-14T05:00:00")


def _vuln(vuln_id: str, *events: dict[str, str], **extra: object) -> dict:
    return {
        "id": vuln_id,
        "affected": [
            {
                "package": {"ecosystem": "PyPI", "name": "Some_Pkg"},
                "ranges": [{"type": "ECOSYSTEM", "events": list(events)}],
            },
            {
                "package": {"ecosystem": "npm", "name": "some-pkg"},
                "ranges": [{"type": "SEMVER", "events": [{"fixed": "1.0.1"}]}],
            },
            {
                "package": {"ecosystem": "PyPI", "name": "some-pkg"},
                "ranges": [{"type": "GIT", "events": [{"fixed": "abc123"}]}],
            },
        ],
        **extra,
    }


def test_osv_remedy_reports_the_fix_or_the_last_affected_release() -> None:
    vuln = _vuln(
        "GHSA-aaaa",
        {"introduced": "0"},
        {"fixed": "1.2.0"},
        {"introduced": "2.0.0"},
        {"fixed": "2.0.3.post1"},
    )
    remedy = pin_freshness.osv_remedy
    assert remedy(vuln, "PyPI", "some-pkg", "1.1.0") == "fixed in 1.2.0"
    assert remedy(vuln, "PyPI", "some.pkg", "2.0.1") == "fixed in 2.0.3.post1"
    assert remedy(vuln, "npm", "Some_Pkg", "1.0.0") == "no fixed release"
    last = _vuln("GHSA-bbbb", {"introduced": "0"}, {"last_affected": "2.0.1"})
    assert remedy(last, "PyPI", "some-pkg", "2.0.1") == "affects up to 2.0.1"


class _Osv:
    """OSV with a fixed table: {(name, version): [(id, record events)]}."""

    def __init__(self, ecosystem: str, table: dict | None = None):
        self.ecosystem = ecosystem
        self.table = table or {}
        self.batches: list[list[tuple[str, str]]] = []

    def post(self, url: str, payload: dict) -> object:
        assert url == "https://api.osv.dev/v1/querybatch", url
        queries = [
            (query["package"]["name"], query["version"]) for query in payload["queries"]
        ]
        assert {query["package"]["ecosystem"] for query in payload["queries"]} <= {
            self.ecosystem
        }
        self.batches.append(queries)
        return {
            "results": [
                {"vulns": [{"id": row[0]} for row in self.table.get(q, [])]}
                for q in queries
            ]
        }

    def fetch(self, url: str) -> object:
        vuln_id = url.removeprefix("https://api.osv.dev/v1/vulns/")
        for (name, _), rows in self.table.items():
            for row_id, events, *extra in rows:
                if row_id == vuln_id:
                    return {
                        "id": vuln_id,
                        "affected": [
                            {
                                "package": {"ecosystem": self.ecosystem, "name": name},
                                "ranges": [{"type": "ECOSYSTEM", "events": events}],
                            }
                        ],
                        **(extra[0] if extra else {}),
                    }
        raise AssertionError(url)

    def fetchers(self) -> pin_freshness.Fetchers:
        return pin_freshness.Fetchers(self.fetch, post=self.post)


def test_osv_batches_skip_withdrawn_records_and_keep_aliases() -> None:
    osv = _Osv(
        "PyPI",
        {
            ("alpha", "1.0"): [
                (
                    "GHSA-one",
                    [{"introduced": "0"}, {"fixed": "1.5"}],
                    {"aliases": ["CVE-2026-1"]},
                ),
                ("GHSA-gone", [{"fixed": "9"}], {"withdrawn": "2026-10-01T00:00:00Z"}),
            ]
        },
    )
    found = pin_freshness.osv_affected(
        "PyPI", [("alpha", "1.0"), ("beta", "2.0")], osv.fetchers()
    )
    assert found == {
        ("alpha", "1.0"): [
            pin_freshness.Advisory("GHSA-one", ("CVE-2026-1",), "fixed in 1.5")
        ]
    }
    assert osv.batches == [[("alpha", "1.0"), ("beta", "2.0")]]


def _no_network(*args: object) -> object:
    raise AssertionError(f"unexpected fetch {args}")


def test_each_pin_reads_its_release_list_once() -> None:
    urls: list[str] = []

    def fetch(url: str) -> object:
        urls.append(url)
        return []

    (rust,) = (
        pin
        for pin in pin_freshness.collect_pins(ROOT, fetch=fetch, post=_no_network)
        if pin.name == "rust"
    )
    assert rust.releases() == rust.releases() == []
    assert urls == ["https://api.github.com/repos/rust-lang/rust/releases?per_page=100"]


def test_every_pin_reads_its_authority_loader() -> None:
    collected = pin_freshness.collect_pins(ROOT, fetch=_no_network, post=_no_network)
    assert len({pin.name for pin in collected}) == len(collected)
    pins = {pin.name: pin for pin in collected}
    tools = tool_releases.load_tool_releases(ROOT)
    for name, release in tools.items():
        assert pins[name].current == release.version
        assert pins[name].authority == tool_releases.TOOL_RELEASES_PATH
    assert set(pin_freshness.HOLDS) <= set(pins)
    assert {
        "rust",
        "python",
        "python-patch",
        "uv",
        "llvm",
        "wasi-sdk",
        "binaryen",
        "lean",
    } <= set(pins)
    references = load_verified_subset_policy()
    expected = {
        f"reference-python-{line}": reference
        for line, reference in zip(
            references.python_versions, references.reference_cpython, strict=True
        )
    }
    actual = {
        name: pin.current
        for name, pin in pins.items()
        if name.startswith("reference-python-")
    }
    assert actual == expected
    assert all(
        pins[name].authority == "config/verified_subset.toml" for name in expected
    )
    assert pins["python"].current == pins["python-patch"].current
    assert (
        pins["python"].current
        == (ROOT / ".python-version").read_text(encoding="utf-8").strip()
    )
    # Pins whose upstream publishes advisories through OSV carry a feed.
    for name in ("uv", "quint", "cargo-deny", "cargo-audit", "wasm-tools", "lune"):
        assert pins[name].advisories is not None, name


def test_python_pins_track_the_patch_line_and_the_newest_line() -> None:
    seen = []
    line = ".".join(
        (ROOT / ".python-version").read_text(encoding="utf-8").strip().split(".")[:2]
    )

    def fetch(url: str) -> object:
        seen.append(url)
        assert url == pin_freshness.PYTHON_RELEASES
        return [
            {"name": f"Python {line}.98", "release_date": "2026-09-01T00:00:00Z"},
            {"name": f"Python {line}.99", "release_date": "2026-10-21T00:00:00Z"},
            {"name": "Python 9.1.0", "release_date": "2026-10-01T00:00:00Z"},
            {
                "name": "Python 9.2.0rc1",
                "release_date": "2026-09-01T00:00:00Z",
                "pre_release": True,
            },
            {"name": "Python 9.2.0", "release_date": None},
        ]

    pins = {
        pin.name: pin
        for pin in pin_freshness.collect_pins(ROOT, fetch=fetch, post=_no_network)
    }
    patch = pin_freshness.select_target(
        pins["python-patch"].current, pins["python-patch"].releases(), NOW
    )
    assert (patch.version, patch.newest.version) == (f"{line}.98", f"{line}.99")
    assert pins["python"].releases()[-1] == Release(
        "9.1.0", datetime(2026, 10, 1, tzinfo=UTC)
    )
    assert len(seen) == 1  # one python.org read serves both pins


def _leb128(value: int) -> bytes:
    out = bytearray()
    while True:
        byte, value = value & 0x7F, value >> 7
        out.append(byte | (0x80 if value else 0))
        if not value:
            return bytes(out)


def _wasm(*sections: tuple[int, bytes]) -> bytes:
    return b"\0asm\x01\0\0\0" + b"".join(
        bytes([section]) + _leb128(len(payload)) + payload
        for section, payload in sections
    )


def _custom(name: str, data: bytes) -> tuple[int, bytes]:
    return 0, _leb128(len(name)) + name.encode() + data


def _ar(members: dict[str, bytes]) -> bytes:
    out = b"!<arch>\n"
    for name, data in members.items():
        out += f"{name + '/':<16}{0:<12}{0:<6}{0:<6}{644:<8}{len(data):<10}`\n".encode()
        out += data + (b"\n" if len(data) % 2 else b"")
    return out


def _wasm_archive(producer: bytes = b"clang") -> bytes:
    """One wasm32 object archive; `producer` varies like host build paths."""
    return _ar({"a.o": _wasm((1, b"\x00"), _custom("producers", producer))})


def _tarball(
    *members: str, payload: bytes = b"binary", builtin: bytes | None = None
) -> bytes:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        for member in members:
            if member.endswith("/"):
                info = tarfile.TarInfo(member.rstrip("/"))
                info.type = tarfile.DIRTYPE
                info.mode = 0o755
                archive.addfile(info)
                continue
            if member.endswith("VERSION"):
                body = _WASI_VERSION
            elif member.endswith(".a") and builtin is not None:
                body = builtin
            else:
                body = payload
            info = tarfile.TarInfo(member)
            info.size = len(body)
            info.mode = 0o755
            archive.addfile(info, io.BytesIO(body))
    return buffer.getvalue()


_WASI_VERSION = b"99.0+m\nwasi-libc: 0000000\nllvm: 0000000\nllvm-version: 99.1.0\nconfig: 0000000\n"


def _zip(member: str, payload: bytes = b"binary") -> bytes:
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr(member, payload)
    return buffer.getvalue()


def _release_record(blobs: dict[str, bytes], release_id: int = 424242) -> dict:
    return {
        "id": release_id,
        "assets": [
            {
                "name": url.rsplit("/", 1)[-1],
                "size": len(data),
                "digest": "sha256:" + hashlib.sha256(data).hexdigest(),
            }
            for url, data in blobs.items()
        ],
    }


class _Upstream:
    """One faked upstream: a release list, one release record and its blobs,
    and OSV.

    The record and blobs belong to `tag`; `young` tags are listed as published
    a day ago and have no record, so a move to one fails loudly. `affected`
    maps a version to the OSV advisory IDs on it.
    """

    def __init__(
        self,
        tag: str,
        record_url: str,
        blobs: dict[str, bytes],
        *,
        age_days: float = 30,
        young: tuple[str, ...] = (),
        affected: dict[str, tuple[str, ...]] | None = None,
    ):
        self.tag = tag
        self.record_url = record_url
        self.blobs = blobs
        self.record = _release_record(blobs)
        self.releases = [
            {"tag_name": tag, "published_at": _ago(age_days).isoformat()},
            *({"tag_name": t, "published_at": _ago(1).isoformat()} for t in young),
        ]
        self.affected = affected or {}

    def fetch(self, url: str) -> object:
        if url.endswith("/releases?per_page=100"):
            return self.releases
        if url == self.record_url:
            return self.record
        if url.startswith("https://api.osv.dev/v1/vulns/"):
            return {"id": url.rsplit("/", 1)[-1]}
        raise AssertionError(f"unexpected fetch {url}")

    def fetch_text(self, url: str) -> bytes:
        return self.blobs[url]

    def fetch_file(self, url: str, destination: Path) -> None:
        destination.write_bytes(self.blobs[url])

    def post(self, url: str, payload: object) -> object:
        assert url == "https://api.osv.dev/v1/querybatch", url
        return {
            "results": [
                {"vulns": [{"id": i} for i in self.affected.get(q["version"], ())]}
                for q in payload["queries"]
            ]
        }

    def fetchers(self) -> pin_freshness.Fetchers:
        return pin_freshness.Fetchers(
            self.fetch, self.fetch_text, self.fetch_file, self.post
        )


def _tool_upstream(
    release: tool_releases.ToolRelease, new: str, **upstream: object
) -> _Upstream:
    blobs = {}
    for asset in release.assets.values():
        url = asset.url.replace(release.version, new)
        member = asset.archive_member.replace(release.version, new)
        blobs[url] = _zip(member) if url.endswith(".zip") else _tarball(member)
    tag = release.provenance.url.rsplit("/", 1)[-1]
    return _Upstream(
        tag.replace(release.version, new),
        release.provenance.url.replace(release.version, new),
        blobs,
        **upstream,
    )


def _pin_root(tmp_path: Path) -> Path:
    """A Git root holding the pin authorities (no Actions, no npm projects)."""
    for relative in PIN_AUTHORITIES:
        target = tmp_path / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / relative, target)
    policy = tomllib.loads(
        (tmp_path / "config/verified_subset.toml").read_text(encoding="utf-8")
    )
    for suite in policy["differential_suites"]:
        (tmp_path / suite["path"]).mkdir(parents=True, exist_ok=True)
    assert (
        run_guarded_test_process(["git", "init", "-q", str(tmp_path)]).returncode == 0
    )
    return tmp_path


def _changed_lines(before: str, after: str) -> list[tuple[str, str]]:
    return [
        (old, new)
        for old, new in zip(before.splitlines(), after.splitlines(), strict=True)
        if old != new
    ]


@pytest.mark.parametrize("tool", ["wasm-tools", "uv"])
def test_tool_update_verifies_every_asset_and_moves_its_policy(tmp_path, tool) -> None:
    root = _pin_root(tmp_path)
    release = tool_releases.load_tool_releases(root)[tool]
    # 9.9.10 is a day old: the move stops at the aged 9.9.9 and never reads it.
    young = release.provenance.url.rsplit("/", 1)[-1].replace(release.version, "9.9.10")
    upstream = _tool_upstream(release, "9.9.9", young=(young,))
    plan_before = (root / "tools/proof_plan.toml").read_text(encoding="utf-8")

    message = pin_freshness.update(root, tool, upstream.fetchers(), now=NOW)

    assert message.startswith(f"{tool}: {release.version} -> 9.9.9")
    moved = tool_releases.load_tool_releases(root)
    assert moved[tool].version == "9.9.9"
    assert moved[tool].provenance.release_id == 424242
    for key, asset in moved[tool].assets.items():
        assert asset.sha256 == hashlib.sha256(upstream.blobs[asset.url]).hexdigest()
        assert asset.size == len(upstream.blobs[asset.url]), key
    for name, other in tool_releases.load_tool_releases(ROOT).items():
        if name != tool:
            assert moved[name] == other
    changed = _changed_lines(
        plan_before, (root / "tools/proof_plan.toml").read_text(encoding="utf-8")
    )
    assert changed and all(
        old.replace(release.version, "9.9.9").replace(
            re.escape(release.version), re.escape("9.9.9")
        )
        == new
        for old, new in changed
    )
    assert any(re.escape("9.9.9") in line for _, line in changed)


def test_tool_update_waits_for_a_release_younger_than_seven_days(tmp_path) -> None:
    root = _pin_root(tmp_path)
    release = tool_releases.load_tool_releases(root)["wasm-tools"]
    upstream = _tool_upstream(release, "9.9.9", age_days=1)
    before = (root / "config/tool_releases.toml").read_bytes()

    message = pin_freshness.update(root, "wasm-tools", upstream.fetchers(), now=NOW)

    assert message.startswith(f"wasm-tools: already at {release.version}; 9.9.9")
    assert (root / "config/tool_releases.toml").read_bytes() == before


def test_tool_update_takes_a_young_advisory_fix_at_once(tmp_path) -> None:
    root = _pin_root(tmp_path)
    release = tool_releases.load_tool_releases(root)["wasm-tools"]
    upstream = _tool_upstream(
        release,
        "9.9.9",
        age_days=1,
        affected={release.version: ("RUSTSEC-2026-0999",)},
    )

    message = pin_freshness.update(root, "wasm-tools", upstream.fetchers(), now=NOW)

    assert "fixing RUSTSEC-2026-0999" in message
    assert tool_releases.load_tool_releases(root)["wasm-tools"].version == "9.9.9"


@pytest.mark.parametrize("defect", ["digest", "size", "missing", "member"])
def test_tool_update_fails_closed_and_leaves_files_unchanged(tmp_path, defect) -> None:
    root = _pin_root(tmp_path)
    release = tool_releases.load_tool_releases(root)["wasm-tools"]
    upstream = _tool_upstream(release, "9.9.9")
    first = upstream.record["assets"][0]
    url = next(url for url in upstream.blobs if url.endswith(first["name"]))
    if defect == "digest":
        first["digest"] = "sha256:" + "0" * 64
    elif defect == "size":
        first["size"] += 1
    elif defect == "missing":
        del first["digest"]
    else:
        archive = _zip if first["name"].endswith(".zip") else _tarball
        upstream.blobs[url] = archive("unexpected/member")
        first["size"] = len(upstream.blobs[url])
        first["digest"] = "sha256:" + hashlib.sha256(upstream.blobs[url]).hexdigest()
    before = {relative: (root / relative).read_bytes() for relative in PIN_AUTHORITIES}
    with pytest.raises(PinFreshnessError):
        pin_freshness.update(root, "wasm-tools", upstream.fetchers(), now=NOW)
    assert before == {
        relative: (root / relative).read_bytes() for relative in PIN_AUTHORITIES
    }


def test_checksum_manifest_update_verifies_against_shasums(tmp_path) -> None:
    root = _pin_root(tmp_path)
    release = tool_releases.load_tool_releases(root)["node"]
    upstream = _tool_upstream(release, "99.0.0")
    manifest_url = release.provenance.url.replace(release.version, "99.0.0")
    upstream.blobs[manifest_url] = "\n".join(
        f"{hashlib.sha256(data).hexdigest()}  {url.rsplit('/', 1)[-1]}"
        for url, data in upstream.blobs.items()
    ).encode()

    def fetch(url: str) -> object:
        assert url == "https://nodejs.org/dist/index.json", url
        return [{"version": "v99.0.0", "date": "2026-10-01"}]

    pin_freshness.update(
        root,
        "node",
        pin_freshness.Fetchers(fetch, upstream.fetch_text, upstream.fetch_file),
        now=NOW,
    )
    moved = tool_releases.load_tool_releases(root)["node"]
    assert moved.version == "99.0.0"
    assert moved.provenance.release_id is None


def test_binaryen_update_derives_tree_identity_with_the_provisioner(tmp_path) -> None:
    root = _pin_root(tmp_path)
    release = load_binaryen_manifest(root).release
    new = "999"
    archive_root = f"binaryen-version_{new}"
    blobs = {
        target.url.replace(f"version_{release.version}", f"version_{new}"): _tarball(
            f"{archive_root}/",
            f"{archive_root}/bin/",
            f"{archive_root}/{target.executable}",
        )
        for target in release.targets
    }
    upstream = _Upstream(
        f"version_{new}",
        release.provenance_url.replace(f"version_{release.version}", f"version_{new}"),
        blobs,
    )

    pin_freshness.update(root, "binaryen", upstream.fetchers(), now=NOW)

    moved = load_binaryen_manifest(root).release
    assert moved.version == new
    for target in moved.targets:
        assert target.archive_root == archive_root
        assert target.tree_entries == 3
        assert target.executable_sha256 == hashlib.sha256(b"binary").hexdigest()
        assert target.sha256 == hashlib.sha256(blobs[target.url]).hexdigest()


def test_wasi_sdk_update_reads_versions_and_moves_wasm_ld(tmp_path) -> None:
    root = _pin_root(tmp_path)
    wasi = load_llvm_releases(root).wasi_sdk
    old_tag = wasi.archive_version.split(".")[0]

    def move(text: str) -> str:
        return text.replace(
            f"wasi-sdk-{wasi.archive_version}", "wasi-sdk-99.0"
        ).replace(f"wasi-sdk-{old_tag}/", "wasi-sdk-99/")

    builtin_paths = [
        "share/wasi-sysroot/lib/wasm32-wasip1/libc-printscan-long-double.a",
        "lib/clang/99/lib/wasm32-unknown-wasip1/libclang_rt.builtins.a",
    ]
    builtins = {
        target.id: _wasm_archive(producer=target.id.encode()) for target in wasi.targets
    }
    blobs = {
        move(target.url): _tarball(
            f"{move(target.archive_root)}/",
            f"{move(target.archive_root)}/VERSION",
            *(f"{move(target.archive_root)}/{path}" for path in builtin_paths),
            builtin=builtins[target.id],
        )
        for target in wasi.targets
    }
    upstream = _Upstream(
        "wasi-sdk-99",
        wasi.provenance_url.replace(f"wasi-sdk-{old_tag}", "wasi-sdk-99"),
        blobs,
    )
    plan_before = (root / "tools/proof_plan.toml").read_text(encoding="utf-8")

    pin_freshness.update(root, "wasi-sdk", upstream.fetchers(), now=NOW)

    moved = load_llvm_releases(root).wasi_sdk
    assert (moved.archive_version, moved.sdk_version, moved.llvm_version) == (
        "99.0",
        "99.0+m",
        "99.1.0",
    )
    changed = _changed_lines(
        plan_before, (root / "tools/proof_plan.toml").read_text(encoding="utf-8")
    )
    assert changed and all(
        wasi.llvm_version in old or re.escape(wasi.llvm_version) in old
        for old, _ in changed
    )
    assert not (root / "vendor/wasm-builtins").exists()
    for asset in moved.targets:
        assert asset.sha256 == hashlib.sha256(blobs[asset.url]).hexdigest()
        assert asset.size == len(blobs[asset.url])


def test_wasi_sdk_update_preserves_distinct_complete_host_assets(
    tmp_path,
) -> None:
    root = _pin_root(tmp_path)
    wasi = load_llvm_releases(root).wasi_sdk
    old_tag = wasi.archive_version.split(".")[0]

    def move(text: str) -> str:
        return text.replace(
            f"wasi-sdk-{wasi.archive_version}", "wasi-sdk-99.0"
        ).replace(f"wasi-sdk-{old_tag}/", "wasi-sdk-99/")

    blobs = {}
    for index, target in enumerate(wasi.targets):
        archive_root = move(target.archive_root)
        members = (
            f"{archive_root}/",
            f"{archive_root}/VERSION",
            f"{archive_root}/share/wasi-sysroot/lib/wasm32-wasip1/"
            "libc-printscan-long-double.a",
            f"{archive_root}/lib/clang/99/lib/wasm32-unknown-wasip1/libclang_rt.builtins.a",
        )
        blobs[move(target.url)] = _tarball(
            *members,
            builtin=_ar({f"m{index}.o": _wasm((1, b"\x00"))}),
        )
    upstream = _Upstream(
        "wasi-sdk-99",
        wasi.provenance_url.replace(f"wasi-sdk-{old_tag}", "wasi-sdk-99"),
        blobs,
    )
    pin_freshness.update(root, "wasi-sdk", upstream.fetchers(), now=NOW)
    moved = load_llvm_releases(root).wasi_sdk
    assert len({asset.sha256 for asset in moved.targets}) == len(moved.targets)
    for asset in moved.targets:
        assert asset.sha256 == hashlib.sha256(blobs[asset.url]).hexdigest()
    assert not (root / "vendor/wasm-builtins").exists()


def test_update_rejects_pins_it_does_not_own(tmp_path) -> None:
    root = _pin_root(tmp_path)

    def fetch(url: str) -> object:
        return [{"tag_name": "v99.0.0", "published_at": "2026-09-01T00:00:00Z"}]

    with pytest.raises(PinFreshnessError, match="lean moves at its own authority"):
        pin_freshness.update(
            root, "lean", pin_freshness.Fetchers(fetch=fetch, post=_no_network), now=NOW
        )


def test_rewrite_toml_fields_requires_every_field() -> None:
    text = '[a]\nx = "1"\n\n[b]\ny = 2\n'
    assert (
        pin_freshness.rewrite_toml_fields(text, {"a": {"x": "9"}, "b": {"y": 3}})
        == '[a]\nx = "9"\n\n[b]\ny = 3\n'
    )
    with pytest.raises(PinFreshnessError, match="fields not found"):
        pin_freshness.rewrite_toml_fields(text, {"a": {"z": "9"}})


def test_rewrite_policy_requires_exactly_one_policy() -> None:
    with pytest.raises(PinFreshnessError, match="0 'nope' policies"):
        pin_freshness.rewrite_policy(
            '[[toolchain_policy]]\nname = "x"\n', "nope", "1", "2"
        )


def test_manifest_loaders_reread_a_rewritten_file(tmp_path) -> None:
    root = _pin_root(tmp_path)
    assert load_binaryen_manifest(root).release.version != "0"
    manifest = root / "config/binaryen_releases.toml"
    manifest.write_text("schema_version = 1\n", encoding="utf-8")
    with pytest.raises(BinaryenConfigError):
        load_binaryen_manifest(root)


# Early pins: a pin younger than 7 days needs a reason.


def _early_pin(name: str, current: str, *releases, affected=None, security=()):
    listed = [
        Release(version, _ago(days), security=version in security)
        for version, days in releases
    ]
    feed = _advisories(affected) if affected is not None else None
    return Pin(name, "authority", current, lambda: listed, feed)


def _pin_exemption(
    name: str, version: str, until: datetime, cites: str = "HF-1"
) -> pin_freshness.Exemption:
    return pin_freshness.Exemption(
        str(pin_freshness.PIN_EXEMPTIONS),
        "pin",
        name,
        version,
        until,
        "adopted early",
        cites,
        None,
    )


def test_a_young_pin_without_a_reason_fails_with_its_eligible_date() -> None:
    pin = _early_pin("lune", "0.11.1", ("0.11.0", 10), ("0.11.1", 1))
    failures, out = _check([pin])
    assert failures == 1
    assert (
        "EARLY   lune: 0.11.1 is younger than 7 days (eligible 2026-10-28T12:00:00Z)"
    ) in out


def test_a_young_pin_with_an_exemption_passes() -> None:
    pin = _early_pin("lune", "0.11.1", ("0.11.0", 10), ("0.11.1", 1))
    out = io.StringIO()
    failures = pin_freshness.check_pins(
        [pin],
        set(),
        now=NOW,
        holds={},
        early={"lune": _pin_exemption("lune", "0.11.1", _ago(1) + WEEK)},
        out=out,
    )
    assert failures == 0
    assert (
        "EARLY   lune: 0.11.1 until 2026-10-28T12:00:00Z (adopted early)"
        in out.getvalue()
    )


@pytest.mark.parametrize(
    ("version", "until", "problem"),
    [
        ("0.11.0", _ago(1) + WEEK, "exempts 0.11.0, but the pin is 0.11.1"),
        ("0.11.1", _ago(2) + WEEK, "ages out 2026-10-28T12:00:00Z"),
        (
            "0.11.1",
            _ago(1) + WEEK + timedelta(hours=1),
            "ages out 2026-10-28T12:00:00Z",
        ),
    ],
)
def test_a_pin_exemption_must_name_the_pin_and_its_age_out(
    version, until, problem
) -> None:
    pin = _early_pin("lune", "0.11.1", ("0.11.0", 10), ("0.11.1", 1))
    out = io.StringIO()
    failures = pin_freshness.check_pins(
        [pin],
        set(),
        now=NOW,
        holds={},
        early={"lune": _pin_exemption("lune", version, until)},
        out=out,
    )
    assert failures == 1
    assert problem in out.getvalue()


def test_a_young_pin_that_fixes_an_advisory_needs_no_exemption() -> None:
    pin = _early_pin(
        "uv",
        "0.13.4",
        ("0.13.1", 8),
        ("0.13.4", 2),
        affected={"0.13.1": ("GHSA-hq85",)},
    )
    failures, out = _check([pin])
    assert failures == 0
    assert "EARLY   uv: 0.13.4 fixes GHSA-hq85 on 0.13.1; eligible" in out


def test_an_advisory_admits_only_the_oldest_clean_fixing_release() -> None:
    # 0.13.4 is the fix; 0.13.5 is a younger release the rule does not admit.
    pin = _early_pin(
        "uv",
        "0.13.5",
        ("0.13.1", 8),
        ("0.13.3", 3),
        ("0.13.4", 2),
        ("0.13.5", 1),
        affected={"0.13.1": ("GHSA-hq85",), "0.13.3": ("GHSA-hq85",)},
    )
    failures, out = _check([pin])
    assert failures == 1
    assert "EARLY   uv: 0.13.5 is younger than 7 days" in out
    fix = _early_pin(
        "uv",
        "0.13.4",
        ("0.13.1", 8),
        ("0.13.3", 3),
        ("0.13.4", 2),
        ("0.13.5", 1),
        affected={"0.13.1": ("GHSA-hq85",), "0.13.3": ("GHSA-hq85",)},
    )
    failures, out = _check([fix])
    assert failures == 0
    assert "EARLY   uv: 0.13.4 fixes GHSA-hq85 on 0.13.1" in out


def test_an_aged_out_pin_record_warns_without_failing() -> None:
    # A pin's record does nothing once the pin is 7 days old; only a lock
    # exemption's `==` pin keeps freezing its package (see the uv test).
    policy = pin_freshness.Policy((_pin_exemption("lune", "0.11.1", _ago(1)),), ())
    out = io.StringIO()
    assert (
        pin_freshness.check_exemptions([policy], now=NOW, expired_fail=True, out=out)
        == 0
    )
    assert out.getvalue().startswith(
        "WARNING pin: lune 0.11.1 aged out 2026-10-21T12:00:00Z"
    )


UV_HQ85 = (
    ("0.13.1", 8),
    ("0.13.3", 3),
    ("0.13.4", 2),
    ("0.13.5", 1),
)


@pytest.mark.parametrize(
    ("version", "cites", "problem"),
    [
        # 0.13.4 is the oldest clean release above 0.13.1.
        ("0.13.4", "GHSA-hq85", None),
        (
            "0.13.5",
            "GHSA-hq85",
            "0.13.5 cites GHSA-hq85, but 0.13.5 is not the oldest release that "
            "fixes it on 0.13.1",
        ),
        # 0.13.1, the aged release below, does not carry it.
        ("0.13.4", "GHSA-zzzz", "0.13.4 is not the oldest release that fixes it"),
        # A ledger row needs no proof.
        ("0.13.5", "HF-1", None),
    ],
)
def test_a_pin_record_citing_an_advisory_must_name_its_oldest_fix(
    version, cites, problem
) -> None:
    pin = _early_pin(
        "uv",
        version,
        *UV_HQ85,
        affected={"0.13.1": ("GHSA-hq85",), "0.13.3": ("GHSA-hq85",)},
    )
    record = _pin_exemption("uv", version, NOW + WEEK, cites)
    prove = pin_freshness.citation_proof(
        lambda: [pin], pin_freshness.Fetchers(_no_network, post=_no_network), now=NOW
    )
    out = io.StringIO()
    failures = pin_freshness.check_exemptions(
        [pin_freshness.Policy((record,), ())],
        now=NOW,
        expired_fail=True,
        prove=prove,
        out=out,
    )
    if problem is None:
        assert failures == 0
        assert out.getvalue().startswith("EXEMPT  pin: uv")
    else:
        assert failures == 1
        assert out.getvalue().startswith(
            f"ERROR   {pin_freshness.PIN_EXEMPTIONS}: uv {version} cites"
        )
        assert problem in out.getvalue()


def test_a_pin_without_an_advisory_feed_cannot_cite_an_advisory() -> None:
    pin = _early_pin("lune", "0.11.1", ("0.11.0", 10), ("0.11.1", 1))
    prove = pin_freshness.citation_proof(
        lambda: [pin], pin_freshness.Fetchers(_no_network, post=_no_network), now=NOW
    )
    problem = prove(_pin_exemption("lune", "0.11.1", NOW, "GHSA-aaaa-bbbb-cccc"))
    assert problem is not None
    assert "no advisory feed covers it; cite the ledger row" in problem


def test_a_citation_counts_its_advisory_not_a_security_release() -> None:
    # 1.0.1 supersedes 1.0.0 as a security release, but 1.0.0 does not carry
    # the cited advisory, so the citation cannot stand.
    pin = _early_pin(
        "tool", "1.0.1", ("1.0.0", 10), ("1.0.1", 1), affected={}, security=("1.0.1",)
    )
    prove = pin_freshness.citation_proof(
        lambda: [pin], pin_freshness.Fetchers(_no_network, post=_no_network), now=NOW
    )
    problem = prove(_pin_exemption("tool", "1.0.1", NOW, "GHSA-aaaa-bbbb-cccc"))
    assert problem is not None
    assert "1.0.1 is not the oldest release that fixes it on 1.0.0" in problem


def test_printed_times_round_up_to_the_records_whole_second() -> None:
    published = datetime(2026, 10, 15, 17, 40, 8, 77000, tzinfo=UTC)
    assert pin_freshness.utc_text(published + WEEK) == "2026-10-22T17:40:09Z"
    assert pin_freshness.utc_text(datetime(2026, 10, 22, tzinfo=UTC)) == (
        "2026-10-22T00:00:00Z"
    )


def test_a_young_security_release_needs_no_exemption() -> None:
    # A newer line's security release (27.0.1) does not supersede 26.12.1.
    pin = _early_pin(
        "node",
        "26.12.1",
        ("26.12.0", 30),
        ("26.12.1", 1),
        ("27.0.1", 0.5),
        security=("26.12.1", "27.0.1"),
    )
    failures, out = _check([pin])
    assert failures == 0
    assert (
        "EARLY   node: 26.12.1 supersedes 26.12.0 as a security release; eligible"
    ) in out


def test_pin_exemptions_load_from_their_record(tmp_path) -> None:
    (tmp_path / "config").mkdir()
    record = tmp_path / pin_freshness.PIN_EXEMPTIONS
    record.write_text(
        "schema_version = 1\n\n[[pin]]\n"
        'name = "lune"\nversion = "0.11.1"\nuntil = 2026-10-28T10:05:45Z\n'
        'cites = "HF-1"\nreason = "adopted early"\n\n'
        '[[pin]]\nname = "lune"\nversion = "0.11.1"\n'
        'until = 2026-10-28T10:05:45Z\ncites = "HF-2"\nreason = "twice"\n\n'
        '[[pin]]\nname = "uv"\nversion = "0.13.5"\n'
        'until = 2026-10-29T01:35:54Z\ncites = "because"\nreason = "free text"\n\n'
        '[[pin]]\nname = "x"\n',
        encoding="utf-8",
    )
    policy = pin_freshness.pin_policy(tmp_path, {"HF-1"})
    assert [e.until for e in policy.exemptions] == [
        datetime(2026, 10, 28, 10, 5, 45, tzinfo=UTC)
    ] * 2 + [datetime(2026, 10, 29, 1, 35, 54, tzinfo=UTC)]
    assert any("needs exactly name, version" in p for p in policy.problems)
    assert any("lune has more than one exemption" in p for p in policy.problems)
    # The citation is typed: an open ledger row or a published advisory ID.
    assert any("lune cites HF-2, not an open ledger row" in p for p in policy.problems)
    assert any(
        "uv must cite an advisory ID or a ledger row" in p for p in policy.problems
    )


def test_actions_are_pins_read_from_their_version_comments(tmp_path) -> None:
    workflows = tmp_path / ".github" / "workflows"
    workflows.mkdir(parents=True)
    sha = "a" * 40
    (workflows / "ci.yml").write_text(
        f"steps:\n  - uses: actions/cache/restore@{sha} # v6.1.0\n"
        f"  - uses: actions/cache@{sha} # v6.1.0\n"
        "  - uses: ./.github/actions/setup-project\n",
        encoding="utf-8",
    )
    assert pin_freshness.action_pins(tmp_path) == {"actions/cache": "6.1.0"}
    (workflows / "other.yml").write_text(
        f"  - uses: actions/cache@{sha} # v6.0.0\n", encoding="utf-8"
    )
    with pytest.raises(PinFreshnessError, match="several versions"):
        pin_freshness.action_pins(tmp_path)


def test_every_action_and_package_manager_is_a_pin() -> None:
    pins = {
        pin.name: pin
        for pin in pin_freshness.collect_pins(ROOT, fetch=_no_network, post=_no_network)
    }
    for repo, version in pin_freshness.action_pins(ROOT).items():
        assert pins[repo].current == version
        assert pins[repo].advisories is not None
    assert "astral-sh/setup-uv" in pins
    for directory in ("deploy/cloudflare", "tools/browser_asset_graph"):
        manager = json.loads(
            (ROOT / directory / "package.json").read_text(encoding="utf-8")
        )
        assert manager["packageManager"] == f"npm@{pins[f'npm ({directory})'].current}"


# The lock resolvers: uv's and npm's own release-age settings and exemptions.

UV_RULE = (
    '[tool.uv]\nexclude-newer = "7 days"\n'
    'constraint-dependencies = [\n    "certifi==2026.10.19",\n]\n'
)
CERTIFI = (
    'certifi = "2026-10-20T02:24:17Z"  # 2026.10.19 (GHSA-6q48-6c7w-9gw2): '
    "drops a root; ages out 2026-10-27\n"
)


def _uv_lock(packages: dict[str, tuple[str, str]], *, exempt: str = "") -> str:
    """A uv.lock with each {name: (version, newest upload time)} package."""
    lines = ['version = 1\nrevision = 3\nrequires-python = ">=3.12"\n', exempt]
    for name, (version, uploaded) in sorted(packages.items()):
        lines.append(
            f'\n[[package]]\nname = "{name}"\nversion = "{version}"\n'
            'source = { registry = "https://pypi.org/simple" }\n'
            f'sdist = {{ url = "https://files/{name}-{version}.tar.gz", '
            f'hash = "sha256:00", size = 1, upload-time = "{uploaded}" }}\n'
        )
    lines.append(
        '\n[[package]]\nname = "molt"\nversion = "0.1.0"\nsource = { editable = "." }\n'
    )
    return "".join(lines)


def _uv_root(tmp_path: Path, pyproject: str, packages: dict) -> Path:
    (tmp_path / "pyproject.toml").write_text(pyproject, encoding="utf-8")
    (tmp_path / "uv.lock").write_text(_uv_lock(packages), encoding="utf-8")
    return tmp_path


CERTIFI_LOCK = {"certifi": ("2026.10.19", "2026-10-20T02:24:16.604Z")}


def test_a_uv_exemption_admits_one_release_until_it_ages_out(tmp_path) -> None:
    root = _uv_root(
        tmp_path,
        UV_RULE + "\n[tool.uv.exclude-newer-package]\n" + CERTIFI,
        CERTIFI_LOCK,
    )
    policy = pin_freshness.uv_policy(root)
    assert policy.problems == ()
    (exemption,) = policy.exemptions
    assert (exemption.package, exemption.version, exemption.reason) == (
        "certifi",
        "2026.10.19",
        "drops a root",
    )
    assert exemption.until == datetime(2026, 10, 27, 2, 24, 16, 604000, tzinfo=UTC)
    for now, expired_fail, failures, expected in (
        (exemption.until - timedelta(seconds=1), True, 0, "EXEMPT  uv.lock: certifi"),
        (exemption.until, True, 1, "EXPIRED uv.lock: certifi 2026.10.19 aged out"),
        (exemption.until, False, 0, "EXPIRED uv.lock: certifi 2026.10.19 aged out"),
    ):
        out = io.StringIO()
        assert (
            pin_freshness.check_exemptions(
                [policy], now=now, expired_fail=expired_fail, out=out
            )
            == failures
        )
        assert out.getvalue().startswith(expected)


@pytest.mark.parametrize(
    ("pyproject", "problem"),
    [
        (
            UV_RULE.replace("certifi==2026.10.19", "certifi>=2026.10.19"),
            'needs "certifi==2026.10.19", on its own line',
        ),
        (
            UV_RULE
            + CERTIFI.replace(
                "  # 2026.10.19 (GHSA-6q48-6c7w-9gw2): drops a root", "  #"
            ),
            "needs a comment",
        ),
        # A reason is required, not only a date and a citation.
        (UV_RULE + CERTIFI.replace(" drops a root;", ";"), "needs a comment"),
        # The citation is required and typed.
        (UV_RULE + CERTIFI.replace(" (GHSA-6q48-6c7w-9gw2)", ""), "needs a comment"),
        (
            UV_RULE + CERTIFI.replace("GHSA-6q48-6c7w-9gw2", "security"),
            "must cite an advisory ID or a ledger row",
        ),
        (UV_RULE + CERTIFI.replace("10-27", "10-28"), "says it ages out 2026-10-28"),
        (UV_RULE + CERTIFI.replace("# 2026.10.19", "# 2026.10.18"), "names 2026.10.18"),
        # A later cutoff would admit releases newer than the exempt one.
        (
            UV_RULE + CERTIFI.replace("2026-10-20T02:24:17Z", "2026-10-21T00:00:00Z"),
            "matches no locked",
        ),
        (UV_RULE + CERTIFI.replace('"2026-10-20T02:24:17Z"', "false"), "RFC 3339"),
    ],
)
def test_a_uv_exemption_must_be_exact_and_explained(
    tmp_path, pyproject, problem
) -> None:
    if "[tool.uv.exclude-newer-package]" not in pyproject:
        head, _, entry = pyproject.partition("certifi = ")
        pyproject = (
            head + "\n[tool.uv.exclude-newer-package]\ncertifi = " + entry
            if entry
            else head + "\n[tool.uv.exclude-newer-package]\n" + CERTIFI
        )
    root = _uv_root(tmp_path, pyproject, CERTIFI_LOCK)
    (found,) = pin_freshness.uv_policy(root).problems
    assert problem in found


# An exemption that cites an advisory must name the oldest release that
# fixes it; the registry lists the releases and OSV the advisories.

GHSA_6Q48 = (
    "GHSA-6q48-6c7w-9gw2",
    [{"introduced": "0"}, {"fixed": "2026.10.19"}],
    {"aliases": ["PYSEC-2026-4320"]},
)
CERTIFI_PYPI = {
    "releases": {
        "2026.7.22": [{"upload_time_iso_8601": "2026-07-22T03:35:12.644458Z"}],
        # Every file yanked: not a release.
        "2026.10.17": [
            {"upload_time_iso_8601": "2026-10-17T00:00:00Z", "yanked": True}
        ],
        "2026.10.18": [{"upload_time_iso_8601": "2026-10-18T16:21:07.655710Z"}],
        "2026.10.19": [
            {"upload_time_iso_8601": "2026-10-20T02:24:16.604828Z"},
            {"upload_time_iso_8601": "2026-10-20T02:24:15.072385Z"},
        ],
    }
}


def _registry_fetchers(
    osv: _Osv, pypi: dict, npm: dict | None = None
) -> pin_freshness.Fetchers:
    def fetch(url: str) -> object:
        if url.startswith("https://pypi.org/pypi/"):
            return pypi[url.removeprefix("https://pypi.org/pypi/").split("/")[0]]
        if url.startswith("https://registry.npmjs.org/"):
            name = url.removeprefix("https://registry.npmjs.org/")
            return (npm or {})[name.replace("%2F", "/")]
        return osv.fetch(url)

    return pin_freshness.Fetchers(fetch, post=osv.post)


@pytest.mark.parametrize(
    ("cites", "carriers", "problem"),
    [
        ("GHSA-6q48-6c7w-9gw2", ("2026.7.22", "2026.10.18"), None),
        # An alias names the same advisory.
        ("PYSEC-2026-4320", ("2026.7.22", "2026.10.18"), None),
        # 2026.10.18 already fixes it, as 21.14.9 fixed virtualenv's.
        (
            "GHSA-6q48-6c7w-9gw2",
            ("2026.7.22",),
            "cites GHSA-6q48-6c7w-9gw2, but 2026.10.19 is not the oldest release "
            "that fixes it on 2026.7.22",
        ),
        # The exempt release still carries it.
        (
            "GHSA-6q48-6c7w-9gw2",
            ("2026.7.22", "2026.10.18", "2026.10.19"),
            "is not the oldest release that fixes it",
        ),
        # The aged release below does not carry it.
        (
            "GHSA-zzzz-zzzz-zzzz",
            ("2026.7.22", "2026.10.18"),
            "is not the oldest release that fixes it",
        ),
    ],
)
def test_a_uv_exemption_citing_an_advisory_must_name_its_oldest_fix(
    tmp_path, cites, carriers, problem
) -> None:
    root = _uv_root(
        tmp_path,
        UV_RULE
        + "\n[tool.uv.exclude-newer-package]\n"
        + CERTIFI.replace("GHSA-6q48-6c7w-9gw2", cites),
        CERTIFI_LOCK,
    )
    policy = pin_freshness.uv_policy(root)
    assert policy.problems == ()
    osv = _Osv("PyPI", {("certifi", version): [GHSA_6Q48] for version in carriers})
    prove = pin_freshness.citation_proof(
        lambda: [], _registry_fetchers(osv, {"certifi": CERTIFI_PYPI}), now=NOW
    )
    out = io.StringIO()
    failures = pin_freshness.check_exemptions(
        [policy], now=NOW, expired_fail=True, prove=prove, out=out
    )
    if problem is None:
        assert failures == 0
        assert out.getvalue().startswith("EXEMPT  uv.lock: certifi 2026.10.19")
    else:
        assert failures == 1
        assert out.getvalue().startswith("ERROR   pyproject.toml: certifi 2026.10.19")
        assert problem in out.getvalue()
    # One batch query covers the window: 2026.7.22, 2026.10.18 and 2026.10.19.
    ((*window,),) = osv.batches
    assert sorted(window) == [
        ("certifi", "2026.10.18"),
        ("certifi", "2026.10.19"),
        ("certifi", "2026.7.22"),
    ]


def test_a_ledger_row_citation_needs_no_proof(tmp_path) -> None:
    root = _uv_root(
        tmp_path,
        UV_RULE
        + "\n[tool.uv.exclude-newer-package]\n"
        + CERTIFI.replace("GHSA-6q48-6c7w-9gw2", "HF-1"),
        CERTIFI_LOCK,
    )
    prove = pin_freshness.citation_proof(
        lambda: [], pin_freshness.Fetchers(_no_network, post=_no_network), now=NOW
    )
    out = io.StringIO()
    assert (
        pin_freshness.check_exemptions(
            [pin_freshness.uv_policy(root, {"HF-1"})],
            now=NOW,
            expired_fail=True,
            prove=prove,
            out=out,
        )
        == 0
    )


def test_uv_must_apply_the_release_age_rule(tmp_path) -> None:
    root = _uv_root(tmp_path, "[tool.uv]\nmanaged = true\n", {})
    assert pin_freshness.uv_policy(root).problems == (
        'pyproject.toml: [tool.uv] exclude-newer must be "7 days"',
    )


NPM_RULE = "; the rule\nmin-release-age=7\n"
NPM_EXEMPT = (
    "; exempt @s/types@5.1 until 2026-10-29T01:26:16Z, range ^5.1 (HF-1): adopted early\n"
    "min-release-age-exclude[]=@s/types\n"
)


def _npm_root(
    tmp_path: Path, npmrc: str | None, locked: str = "5.1", spec: str = "5.1"
) -> Path:
    project = tmp_path / "web"
    project.mkdir()
    manifest = {"name": "web", "devDependencies": {"@s/types": spec}}
    (project / "package.json").write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )
    lock = {"lockfileVersion": 3, "packages": {"": {"name": "web"}}}
    lock["packages"]["node_modules/@s/types"] = {"version": locked}
    lock["packages"]["node_modules/left"] = {"version": "1.0.0"}
    (project / "package-lock.json").write_text(json.dumps(lock), encoding="utf-8")
    if npmrc is not None:
        (project / ".npmrc").write_text(npmrc, encoding="utf-8")
    for argv in (
        ["git", "init", "-q", str(tmp_path)],
        ["git", "-C", str(tmp_path), "add", "web"],
    ):
        assert run_guarded_test_process(argv).returncode == 0
    return tmp_path


def _published(package: str, version: str) -> datetime:
    assert (package, version) == ("@s/types", "5.1")
    return datetime(2026, 10, 22, 1, 26, 15, 272000, tzinfo=UTC)


def test_an_npm_exemption_pins_one_release_until_it_ages_out(tmp_path) -> None:
    root = _npm_root(tmp_path, NPM_RULE + NPM_EXEMPT)
    policy = pin_freshness.npm_policy(root, _published)
    assert policy.problems == ()
    (exemption,) = policy.exemptions
    assert (exemption.subject, exemption.reason) == (
        "web/package-lock.json",
        "adopted early",
    )
    assert exemption.until == datetime(2026, 10, 29, 1, 26, 16, tzinfo=UTC)
    out = io.StringIO()
    assert (
        pin_freshness.check_exemptions(
            [policy], now=exemption.until, expired_fail=True, out=out
        )
        == 1
    )
    assert "EXPIRED web/package-lock.json: @s/types 5.1 aged out" in out.getvalue()


@pytest.mark.parametrize(
    ("npmrc", "locked", "spec", "problem"),
    [
        (None, "5.1", "5.1", "no .npmrc sets min-release-age"),
        ("min-release-age=3\n", "5.1", "5.1", "min-release-age must be 7"),
        (
            NPM_RULE + "min-release-age-exclude[]=@s/types\n",
            "5.1",
            "5.1",
            "needs a comment",
        ),
        (
            NPM_RULE + NPM_EXEMPT.replace("01:26:16", "02:00:00"),
            "5.1",
            "5.1",
            "ages out 2026-10-29T01:26:16Z",
        ),
        # The exclusion admits any release; only the exact pin holds it to one.
        (NPM_RULE + NPM_EXEMPT, "5.1", "^5.1", "pin @s/types exactly to 5.1"),
        (NPM_RULE + NPM_EXEMPT, "5.2", "5.1", "the lock holds 5.2"),
    ],
)
def test_an_npm_project_must_apply_the_rule_and_explain_exemptions(
    tmp_path, npmrc, locked, spec, problem
) -> None:
    root = _npm_root(tmp_path, npmrc, locked, spec)
    (found,) = pin_freshness.npm_policy(root, _published).problems
    assert problem in found


@pytest.mark.parametrize(("carriers", "failures"), [(("5.0",), 0), ((), 1)])
def test_an_npm_exemption_citing_an_advisory_must_name_its_oldest_fix(
    tmp_path, carriers, failures
) -> None:
    root = _npm_root(
        tmp_path, NPM_RULE + NPM_EXEMPT.replace("(HF-1)", "(GHSA-aaaa-bbbb-cccc)")
    )
    policy = pin_freshness.npm_policy(root, _published)
    assert policy.problems == ()
    advisory = ("GHSA-aaaa-bbbb-cccc", [{"introduced": "0"}, {"fixed": "5.1"}])
    osv = _Osv("npm", {("@s/types", version): [advisory] for version in carriers})
    registry = {
        "@s/types": {
            "versions": {"5.0": {}, "5.1": {}},
            "time": {"5.0": "2026-10-01T00:00:00Z", "5.1": "2026-10-22T01:26:15.272Z"},
        }
    }
    prove = pin_freshness.citation_proof(
        lambda: [], _registry_fetchers(osv, {}, registry), now=NOW
    )
    out = io.StringIO()
    assert (
        pin_freshness.check_exemptions(
            [policy], now=NOW, expired_fail=True, prove=prove, out=out
        )
        == failures
    )
    if failures:
        assert (
            "ERROR   web/.npmrc: @s/types 5.1 cites GHSA-aaaa-bbbb-cccc, but 5.1 is "
            "not the oldest release that fixes it on 5.0"
        ) in out.getvalue()


def test_every_repository_resolver_and_pin_applies_the_release_age_rule() -> None:
    """uv, every tracked npm project and the pin record apply the 7-day rule,
    and every exemption names its release, its reason and when it ages out."""
    assert pin_freshness.uv_policy(ROOT).problems == ()
    assert pin_freshness.npm_policy(ROOT).problems == ()
    rows = set(finding_status.read_ledger(ROOT).open_keys)
    assert pin_freshness.pin_policy(ROOT, rows).problems == ()
    # Every pin record names a pin the tool reads.
    names = {
        pin.name
        for pin in pin_freshness.collect_pins(ROOT, fetch=_no_network, post=_no_network)
    }
    assert {e.package for e in pin_freshness.pin_policy(ROOT).exemptions} <= names
    # Prose about the rule wraps at 100 columns; only a one-line record (an
    # exclude-newer-package entry, which the tool parses) runs longer.
    in_uv = False
    for line in (ROOT / "pyproject.toml").read_text(encoding="utf-8").splitlines():
        if line.startswith("["):
            in_uv = line.startswith("[tool.uv")
        assert not (in_uv and line.lstrip().startswith("#") and len(line) > 100), line


# --prune-expired


def test_prune_deletes_aged_out_exemptions_relocks_and_keeps_versions(tmp_path) -> None:
    root = _npm_root(tmp_path, NPM_RULE + NPM_EXEMPT)
    pyproject = (
        '[tool.uv]\nexclude-newer = "7 days"\nconstraint-dependencies = [\n'
        '    "certifi>=2026.10.19",\n    "certifi==2026.10.19",\n]\n'
        "\n[tool.uv.exclude-newer-package]\n" + CERTIFI
    )
    _uv_root(root, pyproject, CERTIFI_LOCK)
    (root / "config").mkdir()
    (root / pin_freshness.PIN_EXEMPTIONS).write_text(
        "# pins\nschema_version = 1\n\n[[pin]]\n"
        'name = "lune"\nversion = "0.11.1"\nuntil = 2026-10-28T10:05:45Z\n'
        'cites = "HF-1"\nreason = "adopted early"\n\n[[pin]]\nname = "uv"\n'
        'version = "0.13.5"\nuntil = 2026-11-30T00:00:00Z\ncites = "HF-1"\n'
        'reason = "later"\n',
        encoding="utf-8",
    )
    calls = []

    def run(argv, cwd: Path) -> None:
        calls.append((list(argv), cwd))
        if argv[:2] == ["uv", "lock"]:
            (cwd / "uv.lock").write_text(_uv_lock(CERTIFI_LOCK), encoding="utf-8")

    pruned = pin_freshness.prune_expired(
        root, now=datetime(2026, 10, 30, tzinfo=UTC), run=run
    )

    assert pruned == [
        "uv.lock: certifi 2026.10.19",
        "web/package-lock.json: @s/types 5.1",
        "pin: lune 0.11.1",
    ]
    assert calls == [
        (["uv", "lock"], root),
        (pin_freshness.NPM_RELOCK, root / "web"),
    ]
    text = (root / "pyproject.toml").read_text(encoding="utf-8")
    assert "certifi==" not in text and "certifi = " not in text
    assert '"certifi>=2026.10.19"' in text
    assert json.loads((root / "web/package.json").read_text(encoding="utf-8"))[
        "devDependencies"
    ] == {"@s/types": "^5.1"}
    assert (root / "web/.npmrc").read_text(encoding="utf-8") == NPM_RULE
    assert [e.package for e in pin_freshness.pin_policy(root).exemptions] == ["uv"]


def test_prune_restores_every_file_when_a_relock_moves_a_version(tmp_path) -> None:
    root = _npm_root(tmp_path, NPM_RULE + NPM_EXEMPT)
    pyproject = UV_RULE + "\n[tool.uv.exclude-newer-package]\n" + CERTIFI
    _uv_root(root, pyproject, CERTIFI_LOCK)
    (root / "config").mkdir()
    (root / pin_freshness.PIN_EXEMPTIONS).write_text(
        "schema_version = 1\n", encoding="utf-8"
    )
    before = {
        path: path.read_bytes()
        for path in (root / "pyproject.toml", root / "uv.lock", root / "web/.npmrc")
    }

    def run(argv, cwd: Path) -> None:
        if argv[:2] == ["uv", "lock"]:
            moved = {"certifi": ("2026.10.20", "2026-10-21T00:00:00Z")}
            (cwd / "uv.lock").write_text(_uv_lock(moved), encoding="utf-8")

    with pytest.raises(PinFreshnessError, match="moved a locked version"):
        pin_freshness.prune_expired(
            root, now=datetime(2026, 10, 30, tzinfo=UTC), run=run
        )
    assert before == {path: path.read_bytes() for path in before}


def test_prune_leaves_live_exemptions_alone(tmp_path) -> None:
    root = _npm_root(tmp_path, NPM_RULE + NPM_EXEMPT)
    _uv_root(
        root, UV_RULE + "\n[tool.uv.exclude-newer-package]\n" + CERTIFI, CERTIFI_LOCK
    )
    (root / "config").mkdir()
    (root / pin_freshness.PIN_EXEMPTIONS).write_text(
        "schema_version = 1\n", encoding="utf-8"
    )
    assert pin_freshness.prune_expired(root, now=NOW, run=_no_network) == []


# The lock advisory audit: a typed table names each accepted advisory.


def _audit_root(tmp_path: Path) -> Path:
    root = _npm_root(tmp_path, NPM_RULE)
    (root / "uv.lock").write_text(
        _uv_lock({"alpha": ("1.0", "2026-09-01T00:00:00Z")}), encoding="utf-8"
    )
    return root


def _audit_fetchers() -> pin_freshness.Fetchers:
    pypi = _Osv("PyPI", {("alpha", "1.0"): [("PYSEC-2026-9", [{"fixed": "1.1"}])]})
    npm = _Osv(
        "npm",
        {
            ("left", "1.0.0"): [
                ("GHSA-aaaa", [{"introduced": "0"}, {"last_affected": "1.0.0"}]),
                ("GHSA-bbbb", [{"fixed": "1.0.1"}], {"aliases": ["CVE-2026-7"]}),
            ]
        },
    )

    def post(url: str, payload: dict) -> object:
        ecosystem = payload["queries"][0]["package"]["ecosystem"]
        return (pypi if ecosystem == "PyPI" else npm).post(url, payload)

    def fetch(url: str) -> object:
        vuln_id = url.rsplit("/", 1)[-1]
        return (pypi if vuln_id.startswith("PYSEC") else npm).fetch(url)

    return pin_freshness.Fetchers(fetch, post=post)


LOCK = "web/package-lock.json"


def _accept(advisory, package="left", version="1.0.0", row="HF-1", lock=LOCK):
    return pin_freshness.AcceptedAdvisory(advisory, lock, package, version, row)


def test_the_audit_warns_on_accepted_advisories_and_fails_on_the_rest(tmp_path) -> None:
    out = io.StringIO()
    failures = pin_freshness.audit_locks(
        _audit_root(tmp_path),
        {"HF-1"},
        _audit_fetchers(),
        # GHSA-bbbb is accepted by its alias.
        accepted=(_accept("GHSA-aaaa"), _accept("CVE-2026-7")),
        out=out,
    )
    assert failures == 1
    assert out.getvalue().splitlines() == [
        "ADVISORY uv.lock: alpha 1.0 carries PYSEC-2026-9 (fixed in 1.1); "
        "no accepted entry names it",
        "WARNING web/package-lock.json: left 1.0.0 carries GHSA-aaaa "
        "(affects up to 1.0.0); accepted by HF-1",
        "WARNING web/package-lock.json: left 1.0.0 carries GHSA-bbbb "
        "(fixed in 1.0.1); accepted by HF-1",
    ]


@pytest.mark.parametrize(
    ("entry", "open_rows", "message"),
    [
        # The entry's row is closed.
        (_accept("GHSA-aaaa"), set(), "its entry cites HF-1, not an open ledger row"),
        # An entry for another version, lock or package accepts nothing.
        (_accept("GHSA-aaaa", version="1.0.01"), {"HF-1"}, "carries GHSA-aaaa"),
        (_accept("GHSA-aaaa", lock="uv.lock"), {"HF-1"}, "carries GHSA-aaaa"),
    ],
)
def test_an_accepted_entry_must_match_exactly_and_cite_an_open_row(
    tmp_path, entry, open_rows, message
) -> None:
    out = io.StringIO()
    failures = pin_freshness.audit_locks(
        _audit_root(tmp_path),
        open_rows,
        _audit_fetchers(),
        accepted=(
            entry,
            _accept("GHSA-bbbb"),
            _accept("PYSEC-2026-9", "alpha", "1.0", lock="uv.lock"),
        ),
        out=out,
    )
    assert failures >= 1
    assert message in out.getvalue()


def test_an_entry_whose_advisory_left_the_lock_must_be_dropped(tmp_path) -> None:
    out = io.StringIO()
    failures = pin_freshness.audit_locks(
        _audit_root(tmp_path),
        {"HF-1"},
        _audit_fetchers(),
        accepted=(
            _accept("GHSA-aaaa"),
            _accept("GHSA-bbbb"),
            _accept("PYSEC-2026-9", "alpha", "1.0", lock="uv.lock"),
            _accept("GHSA-gone", "left", "0.9.0"),
        ),
        out=out,
    )
    assert failures == 1
    assert (
        "left 0.9.0 no longer carries GHSA-gone; drop its entry (HF-1)"
        in out.getvalue()
    )


def test_repository_accepted_advisories_cite_open_ledger_rows() -> None:
    open_rows = finding_status.read_ledger(ROOT).open_keys
    for entry in pin_freshness.ACCEPTED_ADVISORIES:
        assert entry.row in open_rows, entry
        assert (ROOT / entry.lock).is_file(), entry


def _main_root(tmp_path: Path, uploaded: str, cites: str = "HF-62") -> Path:
    """A checkout whose one exemption, certifi in uv.lock, cites `cites` and
    ages out 7 days after `uploaded`."""
    root = _npm_root(tmp_path, NPM_RULE)
    upload = datetime.fromisoformat(uploaded)
    entry = (
        f'certifi = "{upload + timedelta(seconds=1):%Y-%m-%dT%H:%M:%SZ}"  '
        f"# 2026.10.19 ({cites}): early; ages out {upload + WEEK:%Y-%m-%d}\n"
    )
    _uv_root(
        root,
        UV_RULE + "\n[tool.uv.exclude-newer-package]\n" + entry,
        {"certifi": ("2026.10.19", uploaded)},
    )
    (root / "config").mkdir()
    (root / pin_freshness.PIN_EXEMPTIONS).write_text(
        "schema_version = 1\n", encoding="utf-8"
    )
    ledger = root / "docs/agent/V1_HANDOFF_FINDINGS.md"
    ledger.parent.mkdir(parents=True)
    ledger.write_text(
        "## Open\n\n| ID | Finding | Evidence |\n|--|--|--|\n| HF-62 | rule | here |\n",
        encoding="utf-8",
    )
    return root


@pytest.mark.parametrize(
    ("uploaded", "argv", "status"),
    [
        ("2099-01-01T00:00:00Z", ["--audit"], 0),
        ("2020-01-01T00:00:00Z", ["--audit"], 1),
        ("2020-01-01T00:00:00Z", ["--audit", "--allow-expired"], 0),
    ],
)
def test_the_audit_also_checks_every_exemption(
    tmp_path, monkeypatch, capsys, uploaded, argv, status
) -> None:
    """`molt update` runs --audit: an aged-out lock exemption fails it there,
    and only CI on every change may report it without failing."""
    root = _main_root(tmp_path, uploaded)
    monkeypatch.setattr(pin_freshness, "audit_locks", lambda *args, **kwargs: 0)
    assert pin_freshness.main([*argv, "--root", str(root)]) == status
    assert ("EXPIRED uv.lock: certifi 2026.10.19" in capsys.readouterr().out) == (
        uploaded.startswith("2020")
    )


@pytest.mark.parametrize("argv", [["--audit"], ["--check"]])
def test_both_modes_prove_cited_advisories(tmp_path, monkeypatch, capsys, argv) -> None:
    # 2026.10.18 already fixes the cited advisory, so 2026.10.19 is not its
    # oldest fix, whatever the day.
    root = _main_root(tmp_path, "2099-01-01T00:00:00Z", "GHSA-6q48-6c7w-9gw2")
    osv = _Osv("PyPI", {("certifi", "2026.7.22"): [GHSA_6Q48]})
    fetchers = _registry_fetchers(osv, {"certifi": CERTIFI_PYPI})
    monkeypatch.setattr(pin_freshness, "Fetchers", lambda: fetchers)
    monkeypatch.setattr(pin_freshness, "audit_locks", lambda *args, **kwargs: 0)
    monkeypatch.setattr(pin_freshness, "collect_pins", lambda *args, **kwargs: [])
    monkeypatch.setattr(pin_freshness, "check_pins", lambda *args, **kwargs: 0)
    assert pin_freshness.main([*argv, "--root", str(root)]) == 1
    assert (
        "ERROR   pyproject.toml: certifi 2026.10.19 cites GHSA-6q48-6c7w-9gw2, but "
        "2026.10.19 is not the oldest release that fixes it"
    ) in capsys.readouterr().out


def test_reference_pins_reread_policy_and_track_their_own_patch_line(tmp_path) -> None:
    root = _pin_root(tmp_path)
    authority = root / "config/verified_subset.toml"
    policy = load_verified_subset_policy(authority)
    original = list(policy.reference_cpython)

    def fetch(url: str) -> object:
        assert url == pin_freshness.PYTHON_RELEASES
        return [
            {"name": f"Python {line}.{patch}", "release_date": "2026-09-01T00:00:00Z"}
            for line in policy.python_versions
            for patch in (1, 999)
        ] + [{"name": "Python 9.9.9", "release_date": "2026-09-01T00:00:00Z"}]

    def references():
        return {
            pin.name: pin
            for pin in pin_freshness.collect_pins(root, fetch=fetch, post=_no_network)
            if pin.name.startswith("reference-python-")
        }

    first = references()
    for line, version in zip(policy.python_versions, original, strict=True):
        pin = first[f"reference-python-{line}"]
        assert pin.current == version
        # Each reference tracks its own line, never a newer one.
        assert {release.version.rsplit(".", 1)[0] for release in pin.releases()} == {
            line
        }

    changed = [line + ".1" for line in policy.python_versions]
    text = authority.read_text(encoding="utf-8")
    authority.write_text(
        re.sub(
            r"(?m)^reference_cpython = .*?$",
            "reference_cpython = [" + ", ".join(f'"{v}"' for v in changed) + "]",
            text,
        ),
        encoding="utf-8",
    )
    reread = references()
    assert [
        reread[f"reference-python-{line}"].current for line in policy.python_versions
    ] == changed
    failures, output = _check(list(reread.values()))
    assert failures == len(policy.python_versions)
    for line in policy.python_versions:
        assert f"STALE   reference-python-{line}: {line}.1 -> {line}.999" in output
