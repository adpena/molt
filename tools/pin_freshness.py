#!/usr/bin/env python3
"""Hold pins and locks to the release-age rule, and audit the locks.

Owner rule (2026-10-22): a dependency, toolchain or Action moves to its
newest release that has been public for at least 7 days. A release that
fixes a published advisory moves at once, through a named, dated exemption.
Each resolver applies the rule itself:

  uv.lock      `[tool.uv] exclude-newer = "7 days"` in pyproject.toml; uv
               records the span in the lock, so `uv lock --check` holds.
  npm locks    `min-release-age=7` in each project's `.npmrc`.
  Dependabot   `cooldown: default-days: 7`. Its security updates skip the
               cooldown, but uv and npm still refuse a fix younger than 7
               days: such a fix enters only through an exemption.
  this tool    the toolchain pins in config/ and rust-toolchain.toml, the
               CPython reference pins in config/verified_subset.toml (each on
               its own patch line, never a new line), the Actions pinned under
               .github, and each npm `packageManager`.

An exemption admits exactly one release, names its reason and ages out when
that release is 7 days old:

  uv           an `[tool.uv.exclude-newer-package]` entry (the upload time
               just after the release's newest file) commented
               `# <version>: <reason>; ages out <date>`, plus a
               `<name>==<version>` entry in `constraint-dependencies`.
  npm          `min-release-age-exclude[]=<package>` after a comment
               `; exempt <package>@<version> until <time>, range <range>:
               <reason>`, with package.json pinning exactly <version>.
  pins         a `[[pin]]` entry in config/release_age_exemptions.toml.

  --check          report every pin below its target or younger than 7 days
                   without an exemption, and every exemption that is
                   malformed or has aged out; exit 1 when any exists. A pin's
                   target is its newest clean release at least 7 days old;
                   when the pin carries an advisory and no such release
                   exists, the oldest clean release above it. The scheduled
                   `security.pin-freshness` proof command runs it.
  --audit          query OSV for every locked version in uv.lock and the npm
                   locks; an advisory that ACCEPTED_ADVISORIES names, under an
                   open ledger row, is a WARNING, and any other fails. It also
                   checks every exemption. `molt update` runs it last, and
                   `security.lock-audit` runs it with --allow-expired.
  --prune-expired  delete every exemption that has aged out, relock, and prove
                   no locked version moved. `molt update` runs it first.
  --update NAME    move one `config/tool_releases.toml` tool, Binaryen or the
                   WASI SDK to its target, accepting each asset only when its
                   SHA-256 matches the upstream's own record.

Advisories come from OSV (https://osv.dev) for pins and locks with an OSV
coordinate, and from the `security` flag of nodejs.org's release index for
Node. Python, Rust, LLVM, Lean, Binaryen, the WASI SDK and elan publish no
advisory feed this tool reads: their pins follow the age rule alone, and an
advisory fix for them moves by hand. A pin may trail its newest release only
under a hold that names an open row of the findings ledger; no hold excuses
an advisory. Exit status: 0 clean, 1 findings, 2 the tool could not decide.
No third-party packages; set GITHUB_TOKEN to raise the GitHub API rate limit.
"""

from __future__ import annotations

import argparse
from collections.abc import Callable, Iterable, Mapping, Sequence
from dataclasses import dataclass, replace
from datetime import UTC, datetime, timedelta
import functools
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import sys
import tarfile
import tempfile
import tomllib
import traceback
from typing import TextIO
import urllib.request
import zipfile

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

ROOT = bind_repository_imports(__file__)

from molt import tool_releases  # noqa: E402
from molt.binaryen_toolchain import (  # noqa: E402
    BinaryenConfigError,
    load_binaryen_manifest,
)
from molt.llvm_toolchain import (  # noqa: E402
    LlvmToolchainConfigError,
    load_llvm_releases,
)
from molt.rust_toolchain import rust_channel  # noqa: E402
from molt.verified_subset import load_verified_subset_policy  # noqa: E402
from molt.wasi_sdk_identity import (  # noqa: E402
    WasiSdkIdentityError,
    read_wasi_sdk_version_identity,
)
from tools import (  # noqa: E402
    finding_status,
    harness_memory_guard,
    provision_binaryen,
    provision_wasi_sdk,
)
from tools.dependency_manifests import tracked_manifest_dirs  # noqa: E402

PROOF_PLAN = Path("tools/proof_plan.toml")
USER_AGENT = "molt-pin-freshness"
TIMEOUT_SECONDS = 60
LOCK_TIMEOUT_SECONDS = 900

# The owner's release-age rule. pyproject.toml, every npm project's .npmrc
# and .github/dependabot.yml state the same number of days; the tests and
# `--check` hold them to it.
MIN_RELEASE_AGE = timedelta(days=7)
UV_EXCLUDE_NEWER = f"{MIN_RELEASE_AGE.days} days"
NPM_MIN_RELEASE_AGE = str(MIN_RELEASE_AGE.days)
# An exemption may admit files up to this long after the exempt release's
# newest file, and no later, so it cannot admit a newer release.
EXEMPTION_SLACK = timedelta(minutes=1)

# Pins that trail upstream on purpose, each while an open ledger row moves it.
HOLDS = {
    "llvm": "HF-64",
    "python": "HF-65",
}

OSV_API = "https://api.osv.dev/v1"

FetchJson = Callable[[str], object]
FetchBytes = Callable[[str], bytes]
FetchFile = Callable[[str, Path], None]
PostJson = Callable[[str, object], object]


class PinFreshnessError(RuntimeError):
    pass


def _request(
    url: str, accept: str, body: bytes | None = None
) -> urllib.request.Request:
    headers = {"User-Agent": USER_AGENT, "Accept": accept}
    if body is not None:
        headers["Content-Type"] = "application/json"
    token = os.environ.get("GITHUB_TOKEN", "").strip()
    if token and url.startswith("https://api.github.com/"):
        headers["Authorization"] = f"Bearer {token}"
    return urllib.request.Request(url, data=body, headers=headers)


def fetch_bytes(url: str) -> bytes:
    request = _request(url, "*/*")
    with urllib.request.urlopen(request, timeout=TIMEOUT_SECONDS) as response:
        return response.read()


def fetch_file(url: str, destination: Path) -> None:
    """Stream one download to disk; release archives reach hundreds of MB."""
    request = _request(url, "*/*")
    with (
        urllib.request.urlopen(request, timeout=TIMEOUT_SECONDS) as response,
        destination.open("xb") as stream,
    ):
        shutil.copyfileobj(response, stream, 1024 * 1024)


def fetch_json(url: str) -> object:
    request = _request(url, "application/json")
    with urllib.request.urlopen(request, timeout=TIMEOUT_SECONDS) as response:
        return json.load(response)


def post_json(url: str, payload: object) -> object:
    body = json.dumps(payload).encode("utf-8")
    request = _request(url, "application/json", body)
    with urllib.request.urlopen(request, timeout=TIMEOUT_SECONDS) as response:
        return json.load(response)


@dataclass(frozen=True)
class Fetchers:
    fetch: FetchJson = fetch_json
    fetch_text: FetchBytes = fetch_bytes
    fetch_file: FetchFile = fetch_file
    post: PostJson = post_json


def version_key(version: str) -> tuple[int, ...]:
    parts = re.findall(r"\d+", version)
    if not parts:
        raise PinFreshnessError(f"version {version!r} has no numeric part")
    return tuple(int(part) for part in parts)


# A stable release spells only dot-separated numbers; pre-releases, release
# candidates and development builds never become a target.
STABLE_VERSION = re.compile(r"\d+(?:\.\d+)*")


def parse_time(text: str) -> datetime:
    """An upstream publication time in UTC.

    A date-only record (nodejs.org) counts from the end of its day, so it
    never makes a release eligible early.
    """
    if re.fullmatch(r"\d{4}-\d{2}-\d{2}", text):
        return datetime.fromisoformat(text).replace(tzinfo=UTC) + timedelta(days=1)
    value = datetime.fromisoformat(text)
    if value.tzinfo is None:
        raise PinFreshnessError(f"publication time {text!r} has no time zone")
    return value.astimezone(UTC)


def utc_text(moment: datetime) -> str:
    """RFC 3339 at whole seconds, rounded up, so a printed age-out time is
    never early and matches the records, which keep whole seconds."""
    moment = moment.astimezone(UTC)
    if moment.microsecond:
        moment += timedelta(microseconds=1_000_000 - moment.microsecond)
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


@dataclass(frozen=True)
class Release:
    version: str
    published: datetime
    # The upstream marks it a security release (nodejs.org's index does).
    security: bool = False


def stable_release(
    version: str, published: object, *, security: bool = False
) -> Release | None:
    if STABLE_VERSION.fullmatch(version) is None or not isinstance(published, str):
        return None
    return Release(version, parse_time(published), security)


# Upstream release lists: each returns every stable release it can see, with
# its publication time, in the spelling the pin uses.


def github_releases(repo: str, tag_prefix: str, fetch: FetchJson) -> list[Release]:
    records = fetch(f"https://api.github.com/repos/{repo}/releases?per_page=100")
    if not isinstance(records, list):
        raise PinFreshnessError(f"{repo}: release list is not a list")
    releases = []
    for record in records:
        tag = record.get("tag_name")
        if record.get("draft") or record.get("prerelease") or not isinstance(tag, str):
            continue
        if tag.startswith(tag_prefix):
            release = stable_release(
                tag.removeprefix(tag_prefix), record.get("published_at")
            )
            if release is not None:
                releases.append(release)
    return releases


def node_releases(fetch: FetchJson) -> list[Release]:
    index = fetch("https://nodejs.org/dist/index.json")
    if not isinstance(index, list):
        raise PinFreshnessError("nodejs.org: release index is not a list")
    return [
        release
        for entry in index
        if (
            release := stable_release(
                str(entry["version"]).removeprefix("v"),
                entry.get("date"),
                security=entry.get("security") is True,
            )
        )
        is not None
    ]


PYTHON_RELEASES = "https://www.python.org/api/v2/downloads/release/?is_published=true"


def python_releases(fetch: FetchJson) -> list[Release]:
    """Every published CPython release from python.org."""
    releases = []
    for entry in fetch(PYTHON_RELEASES):
        match = re.fullmatch(r"Python (\d+\.\d+\.\d+)", str(entry.get("name", "")))
        if match is None or entry.get("pre_release"):
            continue
        release = stable_release(match.group(1), entry.get("release_date"))
        if release is not None:
            releases.append(release)
    return releases


def npm_releases(package: str, fetch: FetchJson) -> list[Release]:
    record = fetch(f"https://registry.npmjs.org/{package.replace('/', '%2F')}")
    times = record["time"]
    return [
        release
        for version in record["versions"]
        if (release := stable_release(version, times.get(version))) is not None
    ]


def pypi_releases(package: str, fetch: FetchJson) -> list[Release]:
    """Every stable PyPI release with a file not yanked, published at its
    newest file's upload: uv admits a release by its files' upload times."""
    record = fetch(f"https://pypi.org/pypi/{package}/json")
    if not isinstance(record, dict) or not isinstance(record.get("releases"), dict):
        raise PinFreshnessError(f"PyPI: {package} lists no releases")
    releases = []
    for version, files in record["releases"].items():
        uploads = [
            parse_time(file["upload_time_iso_8601"])
            for file in files
            if not file.get("yanked")
        ]
        if uploads and STABLE_VERSION.fullmatch(version):
            releases.append(Release(version, max(uploads)))
    return releases


def crate_releases(name: str, fetch: FetchJson) -> list[Release]:
    record = fetch(f"https://crates.io/api/v1/crates/{name}")
    return [
        release
        for version in record["versions"]
        if not version.get("yanked")
        and (release := stable_release(version["num"], version.get("created_at")))
        is not None
    ]


# Advisories: OSV answers which published advisories affect one version.


@dataclass(frozen=True)
class Advisory:
    id: str
    aliases: tuple[str, ...]
    # How the record says the affected version is fixed, for the report.
    remedy: str


def _same_package(ecosystem: str, left: str, right: str) -> bool:
    if ecosystem == "PyPI":  # PEP 503 name normalization
        return (
            re.sub(r"[-_.]+", "-", left).lower()
            == re.sub(r"[-_.]+", "-", right).lower()
        )
    return left == right


def osv_remedy(vuln: Mapping, ecosystem: str, name: str, version: str) -> str:
    """The oldest `fixed` event above `version`, or the last affected release
    when the record names no fix. Only for the report: the choice of a move
    queries OSV for each candidate release instead."""
    fixed: list[str] = []
    last_affected: list[str] = []
    for affected in vuln.get("affected", []):
        package = affected.get("package", {})
        if package.get("ecosystem") != ecosystem or not _same_package(
            ecosystem, str(package.get("name", "")), name
        ):
            continue
        for affected_range in affected.get("ranges", []):
            if affected_range.get("type") not in ("ECOSYSTEM", "SEMVER"):
                continue
            for event in affected_range.get("events", []):
                for kind, bucket in (
                    ("fixed", fixed),
                    ("last_affected", last_affected),
                ):
                    value = event.get(kind)
                    if isinstance(value, str) and re.search(r"\d", value):
                        if version_key(value) >= version_key(version):
                            bucket.append(value)
    above = [value for value in fixed if version_key(value) > version_key(version)]
    if above:
        return f"fixed in {min(above, key=version_key)}"
    if last_affected:
        return f"affects up to {max(last_affected, key=version_key)}"
    return "no fixed release"


def osv_affected(
    ecosystem: str, packages: Iterable[tuple[str, str]], fetchers: Fetchers
) -> dict[tuple[str, str], list[Advisory]]:
    """Advisories for many (name, version) pairs: one batch query, then the
    full record of each advisory found, so withdrawn records drop out."""
    queries = sorted(set(packages))
    found: dict[tuple[str, str], list[Advisory]] = {}
    records: dict[str, Mapping] = {}
    for start in range(0, len(queries), 1000):  # OSV's batch limit
        chunk = queries[start : start + 1000]
        response = fetchers.post(
            f"{OSV_API}/querybatch",
            {
                "queries": [
                    {"package": {"ecosystem": ecosystem, "name": name}, "version": v}
                    for name, v in chunk
                ]
            },
        )
        results = response.get("results") if isinstance(response, dict) else None
        if not isinstance(results, list) or len(results) != len(chunk):
            raise PinFreshnessError(f"OSV: batch answer does not match {len(chunk)}")
        for (name, version), result in zip(chunk, results, strict=True):
            if result.get("next_page_token"):
                raise PinFreshnessError(f"OSV: {name} {version} has paged results")
            for vuln_id in (vuln["id"] for vuln in result.get("vulns", [])):
                if vuln_id not in records:
                    records[vuln_id] = fetchers.fetch(f"{OSV_API}/vulns/{vuln_id}")
                record = records[vuln_id]
                if record.get("withdrawn"):
                    continue
                found.setdefault((name, version), []).append(
                    Advisory(
                        vuln_id,
                        tuple(record.get("aliases", ())),
                        osv_remedy(record, ecosystem, name, version),
                    )
                )
    return found


# The advisories on each of a set of versions of one package; None: no feed.
VersionAdvisories = Callable[[Sequence[str]], Mapping[str, tuple[Advisory, ...]]]


def osv_version_advisories(
    ecosystem: str, package: str, fetchers: Fetchers
) -> VersionAdvisories:
    def lookup(versions: Sequence[str]) -> Mapping[str, tuple[Advisory, ...]]:
        found = osv_affected(ecosystem, ((package, v) for v in versions), fetchers)
        return {
            version: tuple(found.get((package, version), ())) for version in versions
        }

    return lookup


@dataclass(frozen=True)
class Pin:
    name: str
    authority: str
    current: str
    releases: Callable[[], Sequence[Release]]
    advisories: VersionAdvisories | None = None


@dataclass(frozen=True)
class Target:
    """Where the release-age rule puts one pin at one moment."""

    version: str | None  # None: no release qualifies; stay
    newest: Release
    fixes: tuple[str, ...]  # advisories on the pin that the move fixes
    unfixed: tuple[str, ...]  # advisories on the pin that no release fixes
    waiting: Release | None  # the newest release, while it is too young


def _release_line(version: str) -> str:
    return version.split(".", 1)[0]


@dataclass(frozen=True)
class Carries:
    """What keeps one release from being clean."""

    advisories: tuple[str, ...] = ()  # OSV advisory IDs on it
    superseded_by: tuple[str, ...] = ()  # newer security releases of its line

    def __bool__(self) -> bool:
        return bool(self.advisories or self.superseded_by)

    def labels(self) -> tuple[str, ...]:
        return self.advisories + tuple(
            f"security release {version}" for version in self.superseded_by
        )


def _carries(
    version: str,
    ordered: Sequence[Release],
    listed: Mapping[str, tuple[Advisory, ...]],
    cites: str | None = None,
) -> Carries:
    """What `version` carries. With `cites`, only that advisory counts, by
    its ID or an alias."""
    advisories = tuple(
        advisory.id
        for advisory in listed.get(version, ())
        if cites is None or cites in (advisory.id, *advisory.aliases)
    )
    if cites is not None:
        return Carries(advisories)
    return Carries(
        advisories,
        tuple(
            release.version
            for release in ordered
            if release.security
            and _release_line(release.version) == _release_line(version)
            and version_key(release.version) > version_key(version)
        ),
    )


def select_target(
    current: str,
    releases: Sequence[Release],
    now: datetime,
    advisories: VersionAdvisories | None = None,
) -> Target:
    """The newest clean release at least MIN_RELEASE_AGE old above `current`.
    When `current` carries an advisory and no such release exists, the
    oldest clean release above it, whatever its age. A release is clean when
    OSV lists no advisory for it and no newer security release of its line
    exists."""
    if not releases:
        raise PinFreshnessError("upstream lists no stable release")
    ordered = sorted(releases, key=lambda release: version_key(release.version))
    above = [r for r in ordered if version_key(r.version) > version_key(current)]
    listed = (
        dict(advisories([current, *(r.version for r in above)]))
        if advisories is not None
        else {}
    )

    clean = [r for r in above if not _carries(r.version, ordered, listed)]
    aged = [release for release in clean if release.published <= now - MIN_RELEASE_AGE]
    target = aged[-1].version if aged else None
    pinned = _carries(current, ordered, listed).labels()
    if pinned and target is None and clean:
        target = clean[0].version
    reached = max(
        (version for version in (current, target) if version is not None),
        key=version_key,
    )
    newest = ordered[-1]
    return Target(
        version=target,
        newest=newest,
        fixes=pinned if target is not None else (),
        unfixed=pinned if target is None else (),
        waiting=newest if version_key(newest.version) > version_key(reached) else None,
    )


def _toolchain_policies(root: Path) -> dict[str, str]:
    plan = tomllib.loads((root / PROOF_PLAN).read_text(encoding="utf-8"))
    return {
        str(policy["name"]): str(policy["setup_value"])
        for policy in plan["toolchain_policy"]
        if "setup_value" in policy
    }


# Upstream repositories of `config/tool_releases.toml` tools published as
# GitHub releases, with their tag prefix and OSV package coordinate.
GITHUB_TOOLS = {
    "uv": ("astral-sh/uv", "", ("PyPI", "uv")),
    "wasm-tools": ("bytecodealliance/wasm-tools", "v", ("crates.io", "wasm-tools")),
    "lune": ("lune-org/lune", "v", ("crates.io", "lune")),
    "sccache": ("mozilla/sccache", "v", ("crates.io", "sccache")),
}


_ACTION_USE = re.compile(
    r"uses:\s*(?P<repo>[\w.-]+/[\w.-]+)(?:/[\w./-]+)?@[0-9a-f]{40}\s*#\s*"
    r"v?(?P<version>\d+(?:\.\d+)*)\b"
)


def action_pins(root: Path) -> dict[str, str]:
    """{owner/repo: version} of every Action pinned by commit SHA with a
    `# v<version>` comment under .github; every use of one repo must agree."""
    found: dict[str, set[str]] = {}
    for path in sorted((root / ".github").rglob("*.y*ml")):
        for match in _ACTION_USE.finditer(path.read_text(encoding="utf-8")):
            found.setdefault(match["repo"], set()).add(match["version"])
    split = {repo: sorted(v) for repo, v in found.items() if len(v) > 1}
    if split:
        raise PinFreshnessError(f".github pins one Action at several versions: {split}")
    return {repo: versions.pop() for repo, versions in found.items()}


def collect_pins(
    root: Path,
    *,
    fetch: FetchJson = fetch_json,
    post: PostJson = post_json,
) -> list[Pin]:
    tools = tool_releases.load_tool_releases(root)
    llvm = load_llvm_releases(root)
    policies = _toolchain_policies(root)
    supply_chain = tomllib.loads(
        (root / "config/release_supply_chain.toml").read_text(encoding="utf-8")
    )
    lean = (root / "formal/lean/lean-toolchain").read_text(encoding="utf-8").strip()
    plan = str(PROOF_PLAN)
    manifest = tool_releases.TOOL_RELEASES_PATH
    fetchers = Fetchers(fetch=fetch, post=post)
    python = functools.cache(lambda: python_releases(fetch))
    python_line = policies["python"].rsplit(".", 1)[0]

    def github(repo: str, prefix: str) -> Callable[[], list[Release]]:
        return lambda: github_releases(repo, prefix, fetch)

    pins = [
        Pin(
            "rust",
            "rust-toolchain.toml",
            rust_channel((root / "rust-toolchain.toml").read_bytes()),
            github("rust-lang/rust", ""),
        ),
        # The pinned line must run its newest patch; moving to a newer line is
        # its own held arc.
        Pin(
            "python-patch",
            plan,
            policies["python"],
            lambda: [
                release
                for release in python()
                if release.version.rsplit(".", 1)[0] == python_line
            ],
        ),
        Pin("python", plan, policies["python"], python),
        Pin("node", manifest, tools["node"].version, lambda: node_releases(fetch)),
        Pin(
            "llvm",
            "config/llvm_toolchain_releases.toml",
            llvm.default_release,
            github("llvm/llvm-project", "llvmorg-"),
        ),
        Pin(
            "wasi-sdk",
            "config/llvm_toolchain_releases.toml",
            llvm.wasi_sdk.archive_version,
            github("WebAssembly/wasi-sdk", "wasi-sdk-"),
        ),
        Pin(
            "binaryen",
            "config/binaryen_releases.toml",
            load_binaryen_manifest(root).release.version,
            github("WebAssembly/binaryen", "version_"),
        ),
        Pin(
            "lean",
            "formal/lean/lean-toolchain",
            lean.rsplit(":v", 1)[-1],
            github("leanprover/lean4", "v"),
        ),
        Pin(
            "elan",
            "config/release_supply_chain.toml",
            str(supply_chain["downloads"]["elan"]["version"]),
            github("leanprover/elan", "v"),
        ),
        Pin(
            "quint",
            plan,
            policies["quint"],
            lambda: npm_releases("@informalsystems/quint", fetch),
            osv_version_advisories("npm", "@informalsystems/quint", fetchers),
        ),
    ]
    reference_authority = Path("config/verified_subset.toml")
    references = load_verified_subset_policy(root / reference_authority)
    for line, reference in zip(
        references.python_versions, references.reference_cpython, strict=True
    ):
        pins.append(
            Pin(
                f"reference-python-{line}",
                str(reference_authority),
                reference,
                lambda line=line: [
                    release
                    for release in python()
                    if release.version.rsplit(".", 1)[0] == line
                ],
            )
        )
    for crate in ("cargo-deny", "cargo-audit"):
        pins.append(
            Pin(
                crate,
                plan,
                policies[crate],
                lambda c=crate: crate_releases(c, fetch),
                osv_version_advisories("crates.io", crate, fetchers),
            )
        )
    for name, (repo, prefix, (ecosystem, package)) in GITHUB_TOOLS.items():
        if name in tools:
            pins.append(
                Pin(
                    name,
                    manifest,
                    tools[name].version,
                    github(repo, prefix),
                    osv_version_advisories(ecosystem, package, fetchers),
                )
            )
    for repo, version in sorted(action_pins(root).items()):
        pins.append(
            Pin(
                repo,
                ".github (uses: ... # v<version>)",
                version,
                github(repo, "v"),
                osv_version_advisories("GitHub Actions", repo, fetchers),
            )
        )
    npm = functools.cache(lambda: npm_releases("npm", fetch))
    for project in _npm_projects(root):
        relative = project.relative_to(root).as_posix()
        manager = json.loads((project / "package.json").read_text(encoding="utf-8"))
        spelled = str(manager.get("packageManager", ""))
        if spelled.startswith("npm@"):
            pins.append(
                Pin(
                    f"npm ({relative})",
                    f"{relative}/package.json packageManager",
                    spelled.removeprefix("npm@").split("+", 1)[0],
                    npm,
                    osv_version_advisories("npm", "npm", fetchers),
                )
            )
    unowned = set(tools) - {pin.name for pin in pins}
    if unowned:
        raise PinFreshnessError(
            f"{manifest} tools without an upstream resolver: {sorted(unowned)!r}"
        )
    # Each release list is read once, however many checks ask for it.
    return [replace(pin, releases=functools.cache(pin.releases)) for pin in pins]


def _waiting_note(target: Target) -> str:
    if target.waiting is None:
        return ""
    eligible = target.waiting.published + MIN_RELEASE_AGE
    return (
        f"; {target.waiting.version} (public {target.waiting.published:%Y-%m-%d})"
        f" waits until {utc_text(eligible)}"
    )


# Exemptions: the releases admitted before they are MIN_RELEASE_AGE old. Each
# names one release, its reason and the moment it ages out.


@dataclass(frozen=True)
class Exemption:
    record: str  # the file that records it
    subject: str  # the lock it governs, or "pin"
    package: str  # the package, or the pin's name
    version: str
    until: datetime  # when the release is MIN_RELEASE_AGE old
    reason: str
    cites: str  # the advisory it fixes, or the open ledger row that owns it
    # The registry of a locked package ("PyPI", "npm"); None for a pin. A lock
    # exemption's `==` pin freezes its package until it is pruned; a pin's
    # record does nothing once the pin is MIN_RELEASE_AGE old.
    ecosystem: str | None


@dataclass(frozen=True)
class Policy:
    exemptions: tuple[Exemption, ...]
    problems: tuple[str, ...]


PIN_EXEMPTIONS = Path("config/release_age_exemptions.toml")
# What an exemption cites: an open ledger row, or a published advisory.
CITATION = re.compile(
    r"HF-\d+|GHSA(?:-[0-9a-z]{4}){3}|PYSEC-\d{4}-\d+|CVE-\d{4}-\d{4,}|RUSTSEC-\d{4}-\d{4}"
)


def _cites_row(cites: str) -> bool:
    return cites.startswith("HF-")


def _citation_problem(where: str, cites: str, open_rows: set[str] | None) -> str | None:
    """The citation's spelling, and that a cited row is open; `citation_proof`
    proves a cited advisory."""
    if not CITATION.fullmatch(cites):
        return f"{where} must cite an advisory ID or a ledger row, not {cites!r}"
    if _cites_row(cites) and open_rows is not None and cites not in open_rows:
        return f"{where} cites {cites}, not an open ledger row"
    return None


def pin_policy(root: Path, open_rows: set[str] | None = None) -> Policy:
    """The pins taken early, from `config/release_age_exemptions.toml`."""
    data = tomllib.loads((root / PIN_EXEMPTIONS).read_text(encoding="utf-8"))
    problems = []
    if data.get("schema_version") != 1:
        problems.append(f"{PIN_EXEMPTIONS}: schema_version must be 1")
    exemptions = []
    for entry in data.get("pin", []):
        fields = {
            key: entry.get(key)
            for key in ("name", "version", "until", "reason", "cites")
        }
        if (
            set(entry) != set(fields)
            or not all(
                isinstance(fields[k], str) and fields[k]
                for k in ("name", "version", "reason", "cites")
            )
            or not isinstance(fields["until"], datetime)
            or fields["until"].tzinfo is None
        ):
            problems.append(
                f"{PIN_EXEMPTIONS}: each [[pin]] needs exactly name, version, "
                f"until (an RFC 3339 UTC time), reason and cites, not {entry!r}"
            )
            continue
        exemptions.append(
            Exemption(
                str(PIN_EXEMPTIONS),
                "pin",
                fields["name"],
                fields["version"],
                fields["until"].astimezone(UTC),
                fields["reason"],
                fields["cites"],
                None,
            )
        )
        problem = _citation_problem(
            f"{PIN_EXEMPTIONS}: {fields['name']}", fields["cites"], open_rows
        )
        if problem is not None:
            problems.append(problem)
    names = [exemption.package for exemption in exemptions]
    for name in sorted({name for name in names if names.count(name) > 1}):
        problems.append(f"{PIN_EXEMPTIONS}: {name} has more than one exemption")
    return Policy(tuple(exemptions), tuple(problems))


def _oldest_fix(
    version: str,
    releases: Sequence[Release],
    advisories: VersionAdvisories | None,
    now: datetime,
    cites: str | None = None,
) -> tuple[Release | None, Carries]:
    """The one rule that lets a release younger than MIN_RELEASE_AGE stand:
    it is the oldest release that fixes what the newest aged release below it
    carries. That aged release carries something, so does every release
    between them, and `version` carries nothing. With `cites` (an
    exemption's advisory), only that advisory counts. Returns the aged
    release below (None: there is none) and what `version` fixes on it
    (empty: `version` is not that fix)."""
    ordered = sorted(releases, key=lambda r: version_key(r.version))
    below = [
        release
        for release in ordered
        if version_key(release.version) < version_key(version)
        and release.published <= now - MIN_RELEASE_AGE
    ]
    if not below:
        return None, Carries()
    previous = below[-1]
    between = [
        release.version
        for release in ordered
        if version_key(previous.version)
        < version_key(release.version)
        < version_key(version)
    ]
    window = [previous.version, *between, version]
    listed = advisories(window) if advisories is not None else {}

    def on(release: str) -> Carries:
        return _carries(release, ordered, listed, cites)

    fixed = on(previous.version)
    if fixed and not on(version) and all(on(release) for release in between):
        return previous, fixed
    return previous, Carries()


def _early_note(
    pin: Pin, releases: Sequence[Release], current: Release, now: datetime
) -> tuple[str | None, str]:
    """Why a pin younger than the rule may stand without a record
    (`_oldest_fix`), and when it is eligible; None when it is not the fix."""
    eligible = utc_text(current.published + MIN_RELEASE_AGE)
    previous, fixed = _oldest_fix(pin.current, releases, pin.advisories, now)
    if previous is None or not fixed:
        return None, eligible
    reasons = []
    if fixed.advisories:
        reasons.append(f"fixes {', '.join(fixed.advisories)} on {previous.version}")
    if fixed.superseded_by:
        reasons.append(f"supersedes {previous.version} as a security release")
    return " and ".join(reasons), eligible


def _ages_out_with(until: datetime, published: datetime) -> bool:
    """`until` is the moment `published` turns MIN_RELEASE_AGE old, rounded
    up to a whole second (records keep whole seconds)."""
    return timedelta(0) <= until - (published + MIN_RELEASE_AGE) < timedelta(seconds=1)


def check_pins(
    pins: Sequence[Pin],
    open_rows: set[str],
    *,
    now: datetime,
    holds: Mapping[str, str] = HOLDS,
    early: Mapping[str, Exemption] | None = None,
    out: TextIO | None = None,
) -> int:
    """Report each pin against its target; return the number of failures."""
    out = sys.stdout if out is None else out
    early = {} if early is None else early
    failures = 0
    names = {pin.name for pin in pins}
    for name in sorted(set(holds) - names):
        failures += 1
        print(f"ERROR   hold names unknown pin {name!r}", file=out)
    for name in sorted(set(early) - names):
        failures += 1
        print(f"ERROR   {PIN_EXEMPTIONS} names unknown pin {name!r}", file=out)
    for pin in pins:
        releases = pin.releases()
        target = select_target(pin.current, releases, now, pin.advisories)
        stale = target.version is not None and version_key(
            target.version
        ) > version_key(pin.current)
        trails = version_key(target.newest.version) > version_key(pin.current)
        current = next((r for r in releases if r.version == pin.current), None)
        young = current is not None and current.published > now - MIN_RELEASE_AGE
        exempt = early.get(pin.name)
        note = _waiting_note(target)
        row = holds.get(pin.name)
        if row is not None and row not in open_rows:
            failures += 1
            print(
                f"ERROR   {pin.name}: hold cites {row}, not an open ledger row",
                file=out,
            )
        elif row is not None and not trails:
            failures += 1
            print(
                f"ERROR   {pin.name}: current at {pin.current}; drop its hold", file=out
            )
        elif stale and target.fixes:
            failures += 1
            print(
                f"ADVISORY {pin.name}: {pin.current} -> {target.version} fixes "
                f"{', '.join(target.fixes)} ({pin.authority}){note}",
                file=out,
            )
        elif target.unfixed:
            failures += 1
            print(
                f"ADVISORY {pin.name}: {pin.current} carries "
                f"{', '.join(target.unfixed)}, which no release fixes",
                file=out,
            )
        elif exempt is not None and exempt.version != pin.current:
            failures += 1
            print(
                f"ERROR   {pin.name}: {PIN_EXEMPTIONS} exempts {exempt.version}, "
                f"but the pin is {pin.current}",
                file=out,
            )
        elif young:
            fixes, eligible = _early_note(pin, releases, current, now)
            if fixes is not None:
                print(
                    f"EARLY   {pin.name}: {pin.current} {fixes}; eligible {eligible}",
                    file=out,
                )
            elif exempt is not None and not _ages_out_with(
                exempt.until, current.published
            ):
                failures += 1
                print(
                    f"ERROR   {pin.name}: {PIN_EXEMPTIONS} says it ages out "
                    f"{utc_text(exempt.until)}; {pin.current} ages out {eligible}",
                    file=out,
                )
            elif exempt is not None:
                print(
                    f"EARLY   {pin.name}: {pin.current} until {eligible} "
                    f"({exempt.reason})",
                    file=out,
                )
            else:
                failures += 1
                print(
                    f"EARLY   {pin.name}: {pin.current} is younger than "
                    f"{MIN_RELEASE_AGE.days} days (eligible {eligible}); record "
                    f"it in {PIN_EXEMPTIONS} or move back ({pin.authority})",
                    file=out,
                )
        elif row is not None:
            moved = target.version if stale else target.newest.version
            print(
                f"HELD    {pin.name}: {pin.current} -> {moved} ({row}){note}", file=out
            )
        elif stale:
            failures += 1
            print(
                f"STALE   {pin.name}: {pin.current} -> {target.version} "
                f"({pin.authority}){note}",
                file=out,
            )
        elif target.waiting is not None:
            print(f"WAITING {pin.name}: {pin.current}{note}", file=out)
        else:
            print(f"current {pin.name}: {pin.current}", file=out)
    return failures


def _npm_projects(root: Path) -> list[Path]:
    return [
        root / directory
        for directory in tracked_manifest_dirs(root, "*package-lock.json")
    ]


def _uv_registry_packages(lock_text: str) -> list[Mapping]:
    return [
        package
        for package in tomllib.loads(lock_text).get("package", [])
        if "registry" in package.get("source", {})
    ]


def _newest_upload(package: Mapping) -> datetime | None:
    times = [
        parse_time(distribution["upload-time"])
        for distribution in (package.get("sdist"), *package.get("wheels", []))
        if distribution and "upload-time" in distribution
    ]
    return max(times) if times else None


_UV_TABLE = "[tool.uv.exclude-newer-package]"
_UV_REASON = re.compile(
    r"#\s*(?P<version>[^\s:(]+) \((?P<cites>[^)\s]+)\): (?P<reason>\S.*\S); "
    r"ages out (?P<date>\d{4}-\d{2}-\d{2})\s*$"
)


def _uv_exemption_lines(text: str) -> dict[str, str]:
    """{package: its line} inside `[tool.uv.exclude-newer-package]`."""
    lines: dict[str, str] = {}
    in_table = False
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("["):
            in_table = stripped == _UV_TABLE
        elif in_table and "=" in stripped and not stripped.startswith("#"):
            lines[stripped.split("=", 1)[0].strip().strip('"')] = line
    return lines


def _uv_constraint_lines(text: str) -> list[int]:
    """Indices (into `text.splitlines()`) of the lines inside `[tool.uv]`'s
    multi-line `constraint-dependencies` array."""
    lines = text.splitlines()
    in_uv = False
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("["):
            in_uv = stripped == "[tool.uv]"
        elif in_uv and re.fullmatch(r"constraint-dependencies\s*=\s*\[", stripped):
            end = next(
                (j for j in range(index + 1, len(lines)) if lines[j].strip() == "]"),
                index,
            )
            return list(range(index + 1, end))
    return []


def _uv_exemption_pin(text: str, name: str, version: str) -> int | None:
    """The index of the one line `"<name>==<version>",` inside
    constraint-dependencies (a dependency group may hold the same text)."""
    lines = text.splitlines()
    found = [
        index
        for index in _uv_constraint_lines(text)
        if lines[index].strip().startswith(f'"{name}=={version}",')
    ]
    return found[0] if len(found) == 1 else None


def uv_policy(root: Path, open_rows: set[str] | None = None) -> Policy:
    """The uv release-age setting and its exemptions, checked against uv.lock.

    An exemption admits exactly one release: its
    `[tool.uv.exclude-newer-package]` entry is the upload time just after that
    release's newest file, a `<name>==<version>` entry in
    `constraint-dependencies` refuses every sibling, and its comment names the
    release, the reason and the day it ages out.
    """
    text = (root / "pyproject.toml").read_text(encoding="utf-8")
    uv = tomllib.loads(text).get("tool", {}).get("uv", {})
    problems = []
    if uv.get("exclude-newer") != UV_EXCLUDE_NEWER:
        problems.append(
            f'pyproject.toml: [tool.uv] exclude-newer must be "{UV_EXCLUDE_NEWER}"'
        )
    entries = uv.get("exclude-newer-package", {})
    lines = _uv_exemption_lines(text)
    pinned = {}
    for constraint in uv.get("constraint-dependencies", []):
        match = re.fullmatch(r"([A-Za-z0-9._-]+)==([^\s;,]+)", constraint.strip())
        if match is not None:
            pinned[match.group(1).lower()] = match.group(2)
    locked: dict[str, list[Mapping]] = {}
    for package in _uv_registry_packages(
        (root / "uv.lock").read_text(encoding="utf-8")
    ):
        locked.setdefault(package["name"], []).append(package)
    exemptions = []
    for name, value in sorted(entries.items()):
        where = f"pyproject.toml: exclude-newer-package {name}"
        if not isinstance(value, str) or not re.fullmatch(
            r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", value
        ):
            problems.append(f"{where} must be an RFC 3339 UTC time, not {value!r}")
            continue
        admits = parse_time(value)
        releases = [
            (package["version"], newest)
            for package in locked.get(name, [])
            if (newest := _newest_upload(package)) is not None
            and newest < admits <= newest + EXEMPTION_SLACK
        ]
        if len(releases) != 1:
            problems.append(
                f"{where} admits files up to {value}, which matches no locked "
                "release's newest file within one minute"
            )
            continue
        version, newest = releases[0]
        until = newest + MIN_RELEASE_AGE
        if (
            pinned.get(name) != version
            or _uv_exemption_pin(text, name, version) is None
        ):
            problems.append(
                f'{where} needs "{name}=={version}", on its own line in '
                "constraint-dependencies, so it admits no other release"
            )
        comment = _UV_REASON.search(lines.get(name, ""))
        if comment is None:
            problems.append(
                f"{where} needs a comment: # {version} (<advisory or HF row>): "
                "<reason>; ages out <date>"
            )
            continue
        if comment["version"] != version:
            problems.append(f"{where} names {comment['version']}, not {version}")
        if comment["date"] != f"{until:%Y-%m-%d}":
            problems.append(
                f"{where} says it ages out {comment['date']}; "
                f"{name} {version} ages out {until:%Y-%m-%d}"
            )
        problem = _citation_problem(where, comment["cites"], open_rows)
        if problem is not None:
            problems.append(problem)
        exemptions.append(
            Exemption(
                "pyproject.toml",
                "uv.lock",
                name,
                version,
                until,
                comment["reason"],
                comment["cites"],
                "PyPI",
            )
        )
    return Policy(tuple(exemptions), tuple(problems))


_NPM_EXEMPT = re.compile(
    r"[;#]\s*exempt (?P<package>(?:@[^/\s]+/)?[^@\s]+)@(?P<version>\S+) "
    r"until (?P<until>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z), range (?P<range>\S+) "
    r"\((?P<cites>[^)\s]+)\): (?P<reason>\S.*)$"
)
_NPM_DEPENDENCY_FIELDS = ("dependencies", "devDependencies", "optionalDependencies")


def _npm_spec(manifest: Mapping, package: str) -> tuple[str, str] | None:
    """(field, spec) of a direct dependency in a package.json."""
    for field in _NPM_DEPENDENCY_FIELDS:
        spec = manifest.get(field, {}).get(package)
        if spec is not None:
            return field, spec
    return None


@dataclass(frozen=True)
class NpmExemption:
    exemption: Exemption
    project: Path
    range: str
    comment: str  # the .npmrc comment line
    exclude: str  # the .npmrc exclusion line


def _npm_exemptions(
    project: Path, relative: str
) -> tuple[list[NpmExemption], list[str], str | None]:
    """Parse one project's `.npmrc`: its exemptions, problems and window."""
    npmrc = project / ".npmrc"
    exemptions: list[NpmExemption] = []
    problems: list[str] = []
    age = None
    pending = None
    for line in npmrc.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if stripped.startswith((";", "#")):
            match = _NPM_EXEMPT.match(stripped)
            pending = (match, line) if match is not None else pending
            continue
        key, _, value = (part.strip() for part in stripped.partition("="))
        if key == "min-release-age":
            age = value
        elif key == "min-release-age-exclude[]":
            if pending is None or pending[0]["package"] != value:
                problems.append(
                    f"{relative}/.npmrc: exclusion {value} needs a comment `; exempt "
                    f"{value}@<version> until <time>, range <range> (<advisory or "
                    "HF row>): <reason>`"
                )
            else:
                match, comment = pending
                exemptions.append(
                    NpmExemption(
                        Exemption(
                            "",
                            "",
                            value,
                            match["version"],
                            parse_time(match["until"]),
                            match["reason"],
                            match["cites"],
                            "npm",
                        ),
                        project,
                        match["range"],
                        comment,
                        line,
                    )
                )
        pending = None if key else pending
    return exemptions, problems, age


def npm_policy(
    root: Path,
    published: Callable[[str, str], datetime] | None = None,
    *,
    open_rows: set[str] | None = None,
) -> Policy:
    """Each npm project's `.npmrc` release-age setting and exemptions.

    npm exempts a package by name, so an exemption also pins its direct
    dependency to exactly the exempt release, and records the range that
    `--prune-expired` restores. With `published(package, version)`, which
    reads the registry, the exemption must age out 7 days after upstream's
    publication time.
    """
    exemptions = []
    problems = []
    for project in _npm_projects(root):
        relative = project.relative_to(root).as_posix()
        if not (project / ".npmrc").is_file():
            problems.append(f"{relative}: no .npmrc sets min-release-age")
            continue
        found, parsed_problems, age = _npm_exemptions(project, relative)
        problems.extend(parsed_problems)
        if age != NPM_MIN_RELEASE_AGE:
            problems.append(
                f"{relative}/.npmrc: min-release-age must be {NPM_MIN_RELEASE_AGE}"
            )
        manifest = json.loads((project / "package.json").read_text(encoding="utf-8"))
        lock = json.loads((project / "package-lock.json").read_text(encoding="utf-8"))
        locked = {
            path.rsplit("node_modules/", 1)[-1]: entry.get("version")
            for path, entry in lock.get("packages", {}).items()
            if path
        }
        for item in found:
            exemption = item.exemption
            name, version = exemption.package, exemption.version
            where = f"{relative}/.npmrc: {name}@{version}"
            spec = _npm_spec(manifest, name)
            if spec is None or spec[1] != version:
                problems.append(
                    f"{where} needs package.json to pin {name} exactly to {version}, "
                    "so the exclusion admits no other release"
                )
            if locked.get(name) != version:
                problems.append(f"{where}, but the lock holds {locked.get(name)}")
            if published is not None:
                until = published(name, version) + MIN_RELEASE_AGE
                if not _ages_out_with(exemption.until, until - MIN_RELEASE_AGE):
                    problems.append(
                        f"{where} ages out {utc_text(until)}, not "
                        f"{utc_text(exemption.until)}"
                    )
            problem = _citation_problem(where, exemption.cites, open_rows)
            if problem is not None:
                problems.append(problem)
            exemptions.append(
                replace(
                    exemption,
                    record=f"{relative}/.npmrc",
                    subject=f"{relative}/package-lock.json",
                )
            )
    return Policy(tuple(exemptions), tuple(problems))


def npm_published(fetch: FetchJson) -> Callable[[str, str], datetime]:
    @functools.cache
    def published(package: str, version: str) -> datetime:
        record = fetch(f"https://registry.npmjs.org/{package.replace('/', '%2F')}")
        stamp = record["time"].get(version)
        if not isinstance(stamp, str):
            raise PinFreshnessError(f"npm lists no {package}@{version}")
        return parse_time(stamp)

    return published


# Proves one exemption's citation: the problem, or None.
CitationProof = Callable[[Exemption], str | None]


def citation_proof(
    pins: Callable[[], Sequence[Pin]], fetchers: Fetchers, *, now: datetime
) -> CitationProof:
    """An exemption that cites an advisory must admit the oldest release
    that fixes it: `_oldest_fix`, the rule a young pin meets without a
    record, narrowed to the cited advisory. A pin's resolver and feed answer
    for a pin record; for a lock exemption the registry lists the releases
    and OSV answers in one batch query over the window. A ledger-row
    citation needs no proof and costs no query."""
    registries = {"PyPI": pypi_releases, "npm": npm_releases}

    def prove(exemption: Exemption) -> str | None:
        if _cites_row(exemption.cites):
            return None
        where = f"{exemption.record}: {exemption.package} {exemption.version}"
        if exemption.ecosystem is None:
            pin = next((p for p in pins() if p.name == exemption.package), None)
            if pin is None:
                return f"{where} names no pin the tool reads"
            releases, advisories = pin.releases(), pin.advisories
        else:
            releases = registries[exemption.ecosystem](
                exemption.package, fetchers.fetch
            )
            advisories = osv_version_advisories(
                exemption.ecosystem, exemption.package, fetchers
            )
        if advisories is None:
            return (
                f"{where} cites {exemption.cites}, but no advisory feed covers it; "
                "cite the ledger row that owns it"
            )
        previous, fixed = _oldest_fix(
            exemption.version, releases, advisories, now, exemption.cites
        )
        if fixed:
            return None
        below = f" on {previous.version}" if previous is not None else ""
        return (
            f"{where} cites {exemption.cites}, but {exemption.version} is not the "
            f"oldest release that fixes it{below}"
        )

    return prove


def check_exemptions(
    policies: Sequence[Policy],
    *,
    now: datetime,
    expired_fail: bool,
    prove: CitationProof | None = None,
    out: TextIO | None = None,
) -> int:
    """Report every exemption. A malformed one, or a live one whose cited
    advisory `prove` refutes, always fails. A lock exemption that has aged
    out fails when `expired_fail` (the scheduled `--check`): its `==` pin
    freezes the package. An aged-out pin record does nothing, so it warns."""
    out = sys.stdout if out is None else out
    failures = 0
    for policy in policies:
        for problem in policy.problems:
            failures += 1
            print(f"ERROR   {problem}", file=out)
        for exemption in policy.exemptions:
            where = f"{exemption.subject}: {exemption.package} {exemption.version}"
            expired = now >= exemption.until
            if expired and exemption.ecosystem is None:
                print(
                    f"WARNING {where} aged out {utc_text(exemption.until)}; the pin "
                    "no longer needs it, and `tools/pin_freshness.py "
                    "--prune-expired` deletes it",
                    file=out,
                )
            elif expired:
                failures += int(expired_fail)
                print(
                    f"EXPIRED {where} aged out {utc_text(exemption.until)}; "
                    "`tools/pin_freshness.py --prune-expired` deletes it",
                    file=out,
                )
            elif prove is not None and (problem := prove(exemption)) is not None:
                failures += 1
                print(f"ERROR   {problem}", file=out)
            else:
                print(
                    f"EXEMPT  {where} until {utc_text(exemption.until)} "
                    f"({exemption.reason})",
                    file=out,
                )
    return failures


# The lock advisory audit.


@dataclass(frozen=True)
class AcceptedAdvisory:
    """An advisory on one locked release that an open ledger row accepts."""

    advisory: str
    lock: str
    package: str
    version: str
    row: str


# Each entry needs an open ledger row; an entry whose advisory has left the
# lock must be dropped (the audit fails on both).
ACCEPTED_ADVISORIES = (
    AcceptedAdvisory(
        "GHSA-7pqw-9j4j-h8q3",
        "deploy/cloudflare/package-lock.json",
        "extract-zip",
        "2.0.1",
        "HF-454",
    ),
    AcceptedAdvisory(
        "GHSA-jmr9-qjv8-65gv",
        "deploy/cloudflare/package-lock.json",
        "extract-zip",
        "2.0.1",
        "HF-454",
    ),
)


def locked_packages(root: Path) -> list[tuple[str, str, set[tuple[str, str]]]]:
    """(lockfile, OSV ecosystem, {(name, version)}) for uv.lock and every
    tracked npm lock."""
    locks = [
        (
            "uv.lock",
            "PyPI",
            {
                (package["name"], package["version"])
                for package in _uv_registry_packages(
                    (root / "uv.lock").read_text(encoding="utf-8")
                )
            },
        )
    ]
    for project in _npm_projects(root):
        lock = json.loads((project / "package-lock.json").read_text(encoding="utf-8"))
        locks.append(
            (
                (project / "package-lock.json").relative_to(root).as_posix(),
                "npm",
                {
                    (path.rsplit("node_modules/", 1)[-1], str(entry["version"]))
                    for path, entry in lock.get("packages", {}).items()
                    if path and "version" in entry and not entry.get("link")
                },
            )
        )
    return locks


def audit_locks(
    root: Path,
    open_rows: set[str],
    fetchers: Fetchers,
    *,
    accepted: Sequence[AcceptedAdvisory] = ACCEPTED_ADVISORIES,
    out: TextIO | None = None,
) -> int:
    """Report every advisory on a locked package; return how many fail: an
    advisory no entry of `accepted` names, an entry whose ledger row is not
    open, and an entry whose advisory is no longer on its lock."""
    out = sys.stdout if out is None else out
    failures = 0
    used: set[AcceptedAdvisory] = set()
    for lock, ecosystem, packages in locked_packages(root):
        affected = osv_affected(ecosystem, packages, fetchers)
        for (name, version), advisories in sorted(affected.items()):
            for advisory in advisories:
                where = (
                    f"{lock}: {name} {version} carries {advisory.id} "
                    f"({advisory.remedy})"
                )
                entry = next(
                    (
                        entry
                        for entry in accepted
                        if (entry.lock, entry.package, entry.version)
                        == (lock, name, version)
                        and entry.advisory in (advisory.id, *advisory.aliases)
                    ),
                    None,
                )
                if entry is None:
                    failures += 1
                    print(f"ADVISORY {where}; no accepted entry names it", file=out)
                    continue
                used.add(entry)
                if entry.row in open_rows:
                    print(f"WARNING {where}; accepted by {entry.row}", file=out)
                else:
                    failures += 1
                    print(
                        f"ERROR   {where}; its entry cites {entry.row}, not an open "
                        "ledger row",
                        file=out,
                    )
    for entry in accepted:
        if entry not in used:
            failures += 1
            print(
                f"ERROR   {entry.lock}: {entry.package} {entry.version} no longer "
                f"carries {entry.advisory}; drop its entry ({entry.row})",
                file=out,
            )
    return failures


# --prune-expired: delete exemptions that have aged out, then relock.

Runner = Callable[[Sequence[str], Path], None]
NPM_RELOCK = [
    "corepack",
    "npm",
    "install",
    "--package-lock-only",
    "--ignore-scripts",
    "--no-audit",
    "--no-fund",
]


def _run_locker(argv: Sequence[str], cwd: Path) -> None:
    """One `uv lock` or `npm install` child under the harness memory guard."""
    completed = harness_memory_guard.guarded_completed_process(
        list(argv),
        prefix="MOLT_PIN_FRESHNESS",
        cwd=cwd,
        capture_output=True,
        text=True,
        timeout=LOCK_TIMEOUT_SECONDS,
    )
    if completed.returncode != 0 or completed.timed_out:
        output = (completed.stderr or completed.stdout or "").strip()
        raise PinFreshnessError(
            f"{shlex.join(argv)} exited {completed.returncode} in {cwd}:\n{output}"
        )


def _drop_line(text: str, line: str, where: str) -> str:
    lines = text.splitlines(keepends=True)
    matches = [index for index, item in enumerate(lines) if item.rstrip("\n") == line]
    if len(matches) != 1:
        raise PinFreshnessError(f"{where}: found {len(matches)} copies of {line!r}")
    del lines[matches[0]]
    return "".join(lines)


def _locked_versions(root: Path) -> dict[str, set[tuple[str, str]]]:
    return {lock: packages for lock, _, packages in locked_packages(root)}


def prune_expired(
    root: Path,
    *,
    now: datetime,
    run: Runner = _run_locker,
    open_rows: set[str] | None = None,
) -> list[str]:
    """Delete every exemption whose release is now MIN_RELEASE_AGE old, relock
    the affected locks, and prove that no locked version moved."""
    pruned: list[str] = []
    originals: dict[Path, bytes] = {}

    def edit(path: Path, text: str) -> None:
        originals.setdefault(path, path.read_bytes())
        path.write_text(text, encoding="utf-8")

    before = _locked_versions(root)
    try:
        expired = [e for e in uv_policy(root).exemptions if now >= e.until]
        if expired:
            path = root / "pyproject.toml"
            text = path.read_text(encoding="utf-8")
            lines = _uv_exemption_lines(text)
            for exemption in expired:
                text = _drop_line(text, lines[exemption.package], "pyproject.toml")
                index = _uv_exemption_pin(text, exemption.package, exemption.version)
                if index is None:
                    raise PinFreshnessError(
                        f"pyproject.toml: no single {exemption.package}=="
                        f"{exemption.version} line in constraint-dependencies"
                    )
                kept = text.splitlines(keepends=True)
                del kept[index]
                text = "".join(kept)
                pruned.append(f"uv.lock: {exemption.package} {exemption.version}")
            edit(path, text)
            originals.setdefault(root / "uv.lock", (root / "uv.lock").read_bytes())
            run(["uv", "lock"], root)
        for project in _npm_projects(root):
            found, _, _ = _npm_exemptions(project, project.relative_to(root).as_posix())
            gone = [item for item in found if now >= item.exemption.until]
            if not gone:
                continue
            npmrc = project / ".npmrc"
            manifest_path = project / "package.json"
            text = npmrc.read_text(encoding="utf-8")
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            for item in gone:
                text = _drop_line(text, item.comment, f"{project}/.npmrc")
                text = _drop_line(text, item.exclude, f"{project}/.npmrc")
                spec = _npm_spec(manifest, item.exemption.package)
                if spec is None:
                    raise PinFreshnessError(
                        f"{project}/package.json has no {item.exemption.package}"
                    )
                manifest[spec[0]][item.exemption.package] = item.range
                pruned.append(
                    f"{project.relative_to(root).as_posix()}/package-lock.json: "
                    f"{item.exemption.package} {item.exemption.version}"
                )
            edit(npmrc, text)
            edit(manifest_path, json.dumps(manifest, indent=2) + "\n")
            lock = project / "package-lock.json"
            originals.setdefault(lock, lock.read_bytes())
            run(NPM_RELOCK, project)
        path = root / PIN_EXEMPTIONS
        text = path.read_text(encoding="utf-8")
        kept = text
        for exemption in pin_policy(root).exemptions:
            if now < exemption.until:
                continue
            blocks = re.split(r"(?m)^(?=\[\[pin\]\]\s*$)", kept)
            matches = [
                index
                for index, block in enumerate(blocks)
                if block.startswith("[[pin]]")
                and tomllib.loads(block)["pin"][0]["name"] == exemption.package
            ]
            if len(matches) != 1:
                raise PinFreshnessError(
                    f"{PIN_EXEMPTIONS}: {len(matches)} blocks for {exemption.package}"
                )
            del blocks[matches[0]]
            kept = "".join(blocks).rstrip("\n") + "\n"
            pruned.append(f"pin: {exemption.package} {exemption.version}")
        if kept != text:
            edit(path, kept)
        if _locked_versions(root) != before:
            raise PinFreshnessError("relocking after the prune moved a locked version")
        for policy in (
            uv_policy(root, open_rows),
            npm_policy(root, open_rows=open_rows),
            pin_policy(root, open_rows),
        ):
            if policy.problems:
                raise PinFreshnessError("; ".join(policy.problems))
    except BaseException:
        for path, content in originals.items():
            path.write_bytes(content)
        raise
    return pruned


# --update: each updater downloads every asset to disk, accepts it only when
# its SHA-256 and size match the upstream's own record, derives the pinned
# identities with the provisioner's validators, and returns its file edits.
# The edits land only if the product loaders accept the result.


@dataclass(frozen=True)
class UpstreamAsset:
    sha256: str
    size: int | None


def github_release_assets(
    url: str, fetch: FetchJson
) -> tuple[dict[str, UpstreamAsset], int]:
    """Return {asset filename: digest record} and the release id."""
    release = fetch(url)
    assets = {}
    for asset in release["assets"]:
        digest = str(asset.get("digest") or "")
        if digest.startswith("sha256:"):
            assets[str(asset["name"])] = UpstreamAsset(
                digest.removeprefix("sha256:"), int(asset["size"])
            )
    return assets, int(release["id"])


def checksum_manifest_assets(
    url: str, fetch_text: FetchBytes
) -> dict[str, UpstreamAsset]:
    assets = {}
    for line in fetch_text(url).decode("utf-8").splitlines():
        parts = line.split()
        if len(parts) == 2 and re.fullmatch(r"[0-9a-f]{64}", parts[0]):
            assets[parts[1].removeprefix("*")] = UpstreamAsset(parts[0], None)
    return assets


def _sha256_file(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def verified_download(
    url: str,
    upstream: Mapping[str, UpstreamAsset],
    directory: Path,
    fetch_file: FetchFile,
    *,
    where: str,
) -> Path:
    filename = url.rsplit("/", 1)[-1]
    record = upstream.get(filename)
    if record is None:
        raise PinFreshnessError(f"{where}: upstream records no SHA-256 for {filename}")
    path = directory / filename
    fetch_file(url, path)
    actual = _sha256_file(path)
    if actual != record.sha256:
        raise PinFreshnessError(
            f"{where}: {filename} sha256 {actual} != upstream {record.sha256}"
        )
    size = path.stat().st_size
    if record.size is not None and record.size != size:
        raise PinFreshnessError(
            f"{where}: {filename} has {size} bytes, upstream records {record.size}"
        )
    return path


def _archive_members(path: Path) -> set[str]:
    if path.name.endswith(".zip"):
        with zipfile.ZipFile(path) as archive:
            return set(archive.namelist())
    with tarfile.open(path, mode="r:*") as archive:
        return {member.name.removeprefix("./") for member in archive.getmembers()}


def _toml_value(value: object) -> str:
    return json.dumps(value) if isinstance(value, str) else str(value)


TableEdits = Mapping[str | None, Mapping[str, object]]


def rewrite_toml_fields(text: str, edits: TableEdits) -> str:
    """Set `key = value` lines in named `[table]`s (None: the root table).

    Comments, order and every other line stay; each edited field must exist.
    """
    table: str | None = None
    pending = {(name, key) for name, fields in edits.items() for key in fields}
    out = []
    for line in text.splitlines(keepends=True):
        header = re.match(r"^\[([^\[\]]+)\]\s*$", line)
        if header is not None:
            table = header.group(1).strip()
        elif line.startswith("[["):
            table = "[[array]]"
        elif "=" in line and not line.lstrip().startswith("#"):
            key = line.split("=", 1)[0].strip()
            fields = edits.get(table)
            if fields is not None and key in fields:
                line = f"{key} = {_toml_value(fields[key])}\n"
                pending.discard((table, key))
        out.append(line)
    if pending:
        raise PinFreshnessError(f"fields not found: {sorted(pending, key=str)!r}")
    return "".join(out)


def rewrite_policy(text: str, name: str, old: str, new: str) -> str:
    """Move one `[[toolchain_policy]]` block's version literals."""
    blocks = re.split(r"(?m)^(?=\[\[)", text)
    marker = f'[[toolchain_policy]]\nname = "{name}"\n'
    matches = [index for index, block in enumerate(blocks) if block.startswith(marker)]
    if len(matches) != 1:
        raise PinFreshnessError(f"{PROOF_PLAN} has {len(matches)} {name!r} policies")
    block = blocks[matches[0]]
    # Quoted literals, escaped literals inside evidence strings, and the
    # regex-escaped version inside `version_pattern`.
    for before, after in (
        (f'"{old}"', f'"{new}"'),
        (f'\\"{old}\\"', f'\\"{new}\\"'),
        (re.escape(old), re.escape(new)),
    ):
        block = block.replace(before, after)
    blocks[matches[0]] = block
    return "".join(blocks)


def tool_release_edits(
    release: tool_releases.ToolRelease,
    new: str,
    directory: Path,
    fetchers: Fetchers,
) -> TableEdits:
    """Edits that move one `config/tool_releases.toml` tool to `new`."""
    old = release.version
    provenance_url = release.provenance.url.replace(old, new)
    provenance: dict[str, object] = {"url": provenance_url}
    if release.provenance.kind == tool_releases.PROVENANCE_GITHUB_RELEASE:
        upstream, provenance["release_id"] = github_release_assets(
            provenance_url, fetchers.fetch
        )
    elif release.provenance.kind == tool_releases.PROVENANCE_CHECKSUM_MANIFEST:
        upstream = checksum_manifest_assets(provenance_url, fetchers.fetch_text)
    else:
        raise PinFreshnessError(f"unsupported provenance {release.provenance.kind!r}")
    table = f"tools.{release.name}"
    edits: dict[str | None, Mapping[str, object]] = {
        table: {"version": new},
        f"{table}.provenance": provenance,
    }
    for key, asset in release.assets.items():
        url = asset.url.replace(old, new)
        member = asset.archive_member.replace(old, new)
        where = f"{release.name} {new}"
        path = verified_download(
            url, upstream, directory, fetchers.fetch_file, where=where
        )
        if member not in _archive_members(path):
            raise PinFreshnessError(f"{where}: {path.name} has no member {member}")
        edits[f"{table}.assets.{key}"] = {
            "url": url,
            "size": path.stat().st_size,
            "sha256": _sha256_file(path),
            "archive_member": member,
        }
        path.unlink()
    return edits


def binaryen_edits(
    root: Path, new: str, directory: Path, fetchers: Fetchers
) -> TableEdits:
    """Edits that move `config/binaryen_releases.toml` to `version_<new>`."""
    release = load_binaryen_manifest(root).release
    old = release.version

    def move(text: str) -> str:
        return text.replace(f"version_{old}", f"version_{new}")

    provenance_url = move(release.provenance_url)
    upstream, _ = github_release_assets(provenance_url, fetchers.fetch)
    archive_root = move(release.targets[0].archive_root)
    edits: dict[str | None, Mapping[str, object]] = {
        None: {
            "version": new,
            "archive_root": archive_root,
            "provenance_url": provenance_url,
        }
    }
    for target in release.targets:
        url = move(target.url)
        path = verified_download(
            url, upstream, directory, fetchers.fetch_file, where=f"binaryen {new}"
        )
        with tarfile.open(path, "r:gz") as archive:
            identity = provision_binaryen._archive_identity(
                archive,
                asset_id=target.id,
                expected_root=archive_root,
                executable=target.executable,
            )
        edits[f"targets.{target.id}"] = {
            "url": url,
            "size": path.stat().st_size,
            "sha256": _sha256_file(path),
            "tree_entries": identity.tree.entries,
            "tree_total_bytes": identity.tree.total_bytes,
            "tree_sha256": identity.tree.sha256,
            "executable_sha256": identity.executable_sha256,
        }
        path.unlink()
    return edits


@dataclass(frozen=True)
class WasiSdkMove:
    edits: TableEdits
    old_llvm: str
    new_llvm: str


def wasi_sdk_edits(
    root: Path, new_tag: str, directory: Path, fetchers: Fetchers
) -> WasiSdkMove:
    """Move the complete SDK asset family; no isolated C-runtime vendoring."""
    wasi = load_llvm_releases(root).wasi_sdk
    old_archive = wasi.archive_version
    old_tag = old_archive.split(".", 1)[0]
    new_archive = f"{new_tag}.0"

    def move(text: str) -> str:
        return text.replace(
            f"wasi-sdk-{old_archive}", f"wasi-sdk-{new_archive}"
        ).replace(f"wasi-sdk-{old_tag}/", f"wasi-sdk-{new_tag}/")

    tag_suffix = f"/tags/wasi-sdk-{old_tag}"
    if not wasi.provenance_url.endswith(tag_suffix):
        raise PinFreshnessError(f"WASI SDK provenance {wasi.provenance_url!r}")
    provenance_url = wasi.provenance_url.removesuffix(tag_suffix) + (
        f"/tags/wasi-sdk-{new_tag}"
    )
    upstream, _ = github_release_assets(provenance_url, fetchers.fetch)
    identities: set[tuple[str, str]] = set()
    edits: dict[str | None, Mapping[str, object]] = {}
    where = f"wasi-sdk {new_tag}"
    for target in wasi.targets:
        url = move(target.url)
        archive_root = move(target.archive_root)
        path = verified_download(
            url, upstream, directory, fetchers.fetch_file, where=where
        )
        version_file = directory / f"{target.id}.VERSION"
        with tarfile.open(path, "r:gz") as archive:
            provision_wasi_sdk._validate_archive(archive, expected_root=archive_root)
            member = archive.extractfile(f"{archive_root}/VERSION")
            if member is None:
                raise PinFreshnessError(f"{where}: {path.name} has no VERSION file")
            with member:
                version_file.write_bytes(member.read(64 * 1024 + 1))
            identity = read_wasi_sdk_version_identity(version_file)
        identities.add((identity.sdk_version, identity.llvm_version))
        edits[f"wasi_sdk.targets.{target.id}"] = {
            "url": url,
            "size": path.stat().st_size,
            "sha256": _sha256_file(path),
            "archive_root": archive_root,
        }
        path.unlink()
    if len(identities) != 1:
        raise PinFreshnessError(f"WASI SDK targets disagree: {sorted(identities)!r}")
    sdk_version, llvm_version = identities.pop()
    edits["wasi_sdk"] = {
        "archive_version": new_archive,
        "sdk_version": sdk_version,
        "llvm_version": llvm_version,
        "provenance_url": provenance_url,
    }
    return WasiSdkMove(
        edits=edits,
        old_llvm=wasi.llvm_version,
        new_llvm=llvm_version,
    )


def _apply_edits(
    edits: Mapping[Path, str | bytes], validate: Callable[[], None]
) -> None:
    """Write every edit; restore every file unless the loaders accept them."""
    originals = {path: path.read_bytes() if path.exists() else None for path in edits}
    for path, content in edits.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(
            content.encode("utf-8") if isinstance(content, str) else content
        )
    try:
        validate()
    except Exception:
        for path, original in originals.items():
            if original is None:
                path.unlink()
            else:
                path.write_bytes(original)
        raise


def update(
    root: Path, name: str, fetchers: Fetchers = Fetchers(), *, now: datetime
) -> str:
    pins = collect_pins(root, fetch=fetchers.fetch, post=fetchers.post)
    pin = next((pin for pin in pins if pin.name == name), None)
    if pin is None:
        raise PinFreshnessError(f"unknown pin {name!r}")
    target = select_target(pin.current, pin.releases(), now, pin.advisories)
    latest = target.version
    if latest is None or version_key(latest) <= version_key(pin.current):
        return f"{name}: already at {pin.current}{_waiting_note(target)}"
    plan_path = root / PROOF_PLAN
    plan_text = plan_path.read_text(encoding="utf-8")
    with tempfile.TemporaryDirectory(prefix="pin-freshness-") as scratch:
        directory = Path(scratch)
        if pin.authority == tool_releases.TOOL_RELEASES_PATH:
            manifest = root / tool_releases.TOOL_RELEASES_PATH
            release = tool_releases.load_tool_releases(root)[name]
            table_edits = tool_release_edits(release, latest, directory, fetchers)
            edits = {
                manifest: rewrite_toml_fields(
                    manifest.read_text(encoding="utf-8"), table_edits
                ),
                plan_path: rewrite_policy(plan_text, name, pin.current, latest),
            }

            def validate() -> None:
                moved = tool_releases.load_tool_releases(root)[name]
                if moved.version != latest:
                    raise PinFreshnessError(f"{manifest} still pins {moved.version}")

        elif name == "binaryen":
            manifest = root / "config/binaryen_releases.toml"
            table_edits = binaryen_edits(root, latest, directory, fetchers)
            edits = {
                manifest: rewrite_toml_fields(
                    manifest.read_text(encoding="utf-8"), table_edits
                )
            }

            def validate() -> None:
                if load_binaryen_manifest(root).release.version != latest:
                    raise PinFreshnessError(f"{manifest} did not move")

        elif name == "wasi-sdk":
            manifest = root / "config/llvm_toolchain_releases.toml"
            move = wasi_sdk_edits(root, latest, directory, fetchers)
            edits = {
                manifest: rewrite_toml_fields(
                    manifest.read_text(encoding="utf-8"), move.edits
                ),
            }
            if move.new_llvm != move.old_llvm:
                edits[plan_path] = rewrite_policy(
                    plan_text, "wasm-ld", move.old_llvm, move.new_llvm
                )

            def validate() -> None:
                if load_llvm_releases(root).wasi_sdk.archive_version != f"{latest}.0":
                    raise PinFreshnessError(f"{manifest} did not move")

        else:
            raise PinFreshnessError(
                f"{name} moves at its own authority ({pin.authority})"
            )
        _apply_edits(edits, validate)
    reason = f", fixing {', '.join(target.fixes)}" if target.fixes else ""
    return (
        f"{name}: {pin.current} -> {latest} (every asset verified{reason}); "
        "regenerate with `tools/gen_proof_plan.py --write`"
    )


EXPECTED_ERRORS = (
    PinFreshnessError,
    tool_releases.ToolReleaseError,
    BinaryenConfigError,
    LlvmToolchainConfigError,
    WasiSdkIdentityError,
    OSError,
)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--update", metavar="NAME")
    mode.add_argument("--audit", action="store_true")
    mode.add_argument("--prune-expired", action="store_true")
    parser.add_argument(
        "--allow-expired",
        action="store_true",
        help="with --audit: report aged-out exemptions without failing (CI on "
        "every change; the refresh and the scheduled --check still fail)",
    )
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    if args.allow_expired and not args.audit:
        parser.error("--allow-expired applies to --audit only")
    now = datetime.now(UTC)
    fetchers = Fetchers()
    try:
        if args.update:
            print(update(args.root, args.update, fetchers, now=now))
            return 0
        open_rows = set(finding_status.read_ledger(args.root).open_keys)
        if args.prune_expired:
            pruned = prune_expired(args.root, now=now, open_rows=open_rows)
            for line in pruned:
                print(f"pruned  {line}")
            print(f"pin-freshness: {len(pruned)} exemptions pruned")
            return 0
        records = pin_policy(args.root, open_rows)
        policies = [
            uv_policy(args.root, open_rows),
            npm_policy(args.root, npm_published(fetchers.fetch), open_rows=open_rows),
            records,
        ]
        pins = functools.cache(
            lambda: collect_pins(args.root, fetch=fetchers.fetch, post=fetchers.post)
        )
        failures = 0
        if args.check:
            failures += check_pins(
                pins(),
                open_rows,
                now=now,
                early={record.package: record for record in records.exemptions},
            )
        else:
            failures += audit_locks(args.root, open_rows, fetchers)
        failures += check_exemptions(
            policies,
            now=now,
            expired_fail=not args.allow_expired,
            prove=citation_proof(pins, fetchers, now=now),
        )
        print(f"pin-freshness: {failures} failing")
        return 1 if failures else 0
    except EXPECTED_ERRORS as exc:
        print(f"pin-freshness: {exc}", file=sys.stderr)
        return 2
    except Exception:  # an upstream answer of an unexpected shape
        traceback.print_exc()
        print("pin-freshness: could not decide (see the traceback)", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
