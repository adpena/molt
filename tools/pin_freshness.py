#!/usr/bin/env python3
"""Report and refresh pinned upstream releases.

Owner rule: every dependency, toolchain and tool runs at its newest stable
release, pinned exactly (version plus digest). This tool compares each pin,
read through its one authority's loader, with its upstream:

  --check        report every pin that trails its upstream's newest stable
                 release; exit 1 when any unheld pin does (network; the
                 scheduled `security.pin-freshness` proof command runs it)
  --update NAME  move one `config/tool_releases.toml` tool to its newest
                 release. Every asset is downloaded and accepted only when
                 its SHA-256 matches the upstream's own record (the GitHub
                 release asset digest, or the release's SHASUMS256.txt) and
                 the archive holds the pinned member; the tool's proof-plan
                 toolchain policy moves with it.

A pin may trail upstream only under a hold that names an open row of the
findings ledger; a hold on a current pin, or on a closed row, is an error.
Pins outside `config/tool_releases.toml` move at their own authority. No
third-party packages; set GITHUB_TOKEN to raise the GitHub API rate limit.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import sys
import tarfile
import tempfile
import tomllib
import urllib.request
import zipfile
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import TextIO

if __package__ in (None, ""):
    from import_file import bind_repository_imports
else:
    from tools.import_file import bind_repository_imports

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
from molt.wasi_sdk_identity import (  # noqa: E402
    WasiSdkIdentityError,
    read_wasi_sdk_version_identity,
)
from tools import provision_binaryen, provision_wasi_sdk  # noqa: E402

PROOF_PLAN = Path("tools/proof_plan.toml")
LEDGER = Path("docs/agent/V1_HANDOFF_FINDINGS.md")
USER_AGENT = "molt-pin-freshness"
TIMEOUT_SECONDS = 60

# Pins that trail upstream on purpose, each while an open ledger row moves it.
HOLDS = {
    "llvm": "HF-64",
    "python": "HF-65",
    "uv": "HF-62",
}

FetchJson = Callable[[str], object]
FetchBytes = Callable[[str], bytes]
FetchFile = Callable[[str, Path], None]


class PinFreshnessError(RuntimeError):
    pass


def _request(url: str, accept: str) -> urllib.request.Request:
    headers = {"User-Agent": USER_AGENT, "Accept": accept}
    token = os.environ.get("GITHUB_TOKEN", "").strip()
    if token and url.startswith("https://api.github.com/"):
        headers["Authorization"] = f"Bearer {token}"
    return urllib.request.Request(url, headers=headers)


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


def version_key(version: str) -> tuple[int, ...]:
    parts = re.findall(r"\d+", version)
    if not parts:
        raise PinFreshnessError(f"version {version!r} has no numeric part")
    return tuple(int(part) for part in parts)


# Upstream resolvers: each returns the newest stable version in the spelling
# the pin uses.


def github_latest(repo: str, tag_prefix: str, fetch: FetchJson) -> str:
    release = fetch(f"https://api.github.com/repos/{repo}/releases/latest")
    tag = release.get("tag_name") if isinstance(release, dict) else None
    if not isinstance(tag, str) or not tag.startswith(tag_prefix):
        raise PinFreshnessError(f"{repo}: unexpected latest tag {tag!r}")
    return tag.removeprefix(tag_prefix)


def node_latest(fetch: FetchJson) -> str:
    index = fetch("https://nodejs.org/dist/index.json")
    if not isinstance(index, list) or not index:
        raise PinFreshnessError("nodejs.org: empty release index")
    return str(index[0]["version"]).removeprefix("v")


def rust_stable(fetch_text: FetchBytes) -> str:
    url = "https://static.rust-lang.org/dist/channel-rust-stable.toml"
    manifest = tomllib.loads(fetch_text(url).decode("utf-8"))
    return str(manifest["pkg"]["rust"]["version"]).split(" ", 1)[0]


def npm_latest(package: str, fetch: FetchJson) -> str:
    quoted = package.replace("/", "%2F")
    return str(fetch(f"https://registry.npmjs.org/{quoted}/latest")["version"])


def crate_latest(name: str, fetch: FetchJson) -> str:
    record = fetch(f"https://crates.io/api/v1/crates/{name}")
    return str(record["crate"]["max_stable_version"])


def python_latest(fetch: FetchJson) -> str:
    """The newest CPython release of the newest release line."""
    return str(fetch("https://endoflife.date/api/python.json")[0]["latest"])


def python_latest_patch(current: str, fetch: FetchJson) -> str:
    """The newest CPython patch release of `current`'s release line."""
    line = ".".join(current.split(".")[:2])
    return str(fetch(f"https://endoflife.date/api/python/{line}.json")["latest"])


@dataclass(frozen=True)
class Pin:
    name: str
    authority: str
    current: str
    latest: Callable[[], str]


def _toolchain_policies(root: Path) -> dict[str, str]:
    plan = tomllib.loads((root / PROOF_PLAN).read_text(encoding="utf-8"))
    return {
        str(policy["name"]): str(policy["setup_value"])
        for policy in plan["toolchain_policy"]
        if "setup_value" in policy
    }


# Upstream repositories of `config/tool_releases.toml` tools published as
# GitHub releases, with their tag prefix.
GITHUB_TOOLS = {
    "wasm-tools": ("bytecodealliance/wasm-tools", "v"),
    "lune": ("lune-org/lune", "v"),
    "sccache": ("mozilla/sccache", "v"),
}


def collect_pins(
    root: Path,
    *,
    fetch: FetchJson = fetch_json,
    fetch_text: FetchBytes = fetch_bytes,
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

    def github(repo: str, prefix: str) -> Callable[[], str]:
        return lambda: github_latest(repo, prefix, fetch)

    pins = [
        Pin(
            "rust",
            "rust-toolchain.toml",
            rust_channel((root / "rust-toolchain.toml").read_bytes()),
            lambda: rust_stable(fetch_text),
        ),
        # The pinned line must run its newest patch; moving to a newer line is
        # its own held arc.
        Pin(
            "python-patch",
            plan,
            policies["python"],
            lambda: python_latest_patch(policies["python"], fetch),
        ),
        Pin("python", plan, policies["python"], lambda: python_latest(fetch)),
        Pin("uv", plan, policies["uv"], github("astral-sh/uv", "")),
        Pin("node", manifest, tools["node"].version, lambda: node_latest(fetch)),
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
            lambda: npm_latest("@informalsystems/quint", fetch),
        ),
    ]
    for crate in ("cargo-deny", "cargo-audit"):
        pins.append(
            Pin(crate, plan, policies[crate], lambda c=crate: crate_latest(c, fetch))
        )
    for name, (repo, prefix) in GITHUB_TOOLS.items():
        if name in tools:
            pins.append(Pin(name, manifest, tools[name].version, github(repo, prefix)))
    unowned = set(tools) - {pin.name for pin in pins}
    if unowned:
        raise PinFreshnessError(
            f"{manifest} tools without an upstream resolver: {sorted(unowned)!r}"
        )
    return pins


def open_ledger_rows(root: Path) -> set[str]:
    text = (root / LEDGER).read_text(encoding="utf-8")
    open_sections = text.split("\n## Fixed", 1)[0]
    return set(re.findall(r"^\| (HF-\d+) \|", open_sections, re.MULTILINE))


def check(
    pins: Sequence[Pin],
    open_rows: set[str],
    *,
    holds: Mapping[str, str] = HOLDS,
    out: TextIO = sys.stdout,
) -> int:
    failures = 0
    names = {pin.name for pin in pins}
    for name in sorted(set(holds) - names):
        failures += 1
        print(f"ERROR   hold names unknown pin {name!r}", file=out)
    for pin in pins:
        latest = pin.latest()
        stale = version_key(latest) > version_key(pin.current)
        row = holds.get(pin.name)
        if row is not None and row not in open_rows:
            failures += 1
            print(
                f"ERROR   {pin.name}: hold cites {row}, not an open ledger row",
                file=out,
            )
        elif row is not None and not stale:
            failures += 1
            print(
                f"ERROR   {pin.name}: current at {pin.current}; drop its hold", file=out
            )
        elif stale and row is not None:
            print(f"HELD    {pin.name}: {pin.current} -> {latest} ({row})", file=out)
        elif stale:
            failures += 1
            print(
                f"STALE   {pin.name}: {pin.current} -> {latest} ({pin.authority})",
                file=out,
            )
        else:
            print(f"current {pin.name}: {pin.current}", file=out)
    print(f"pin-freshness: {failures} failing of {len(pins)} pins", file=out)
    return 1 if failures else 0


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


@dataclass(frozen=True)
class Fetchers:
    fetch: FetchJson = fetch_json
    fetch_text: FetchBytes = fetch_bytes
    fetch_file: FetchFile = fetch_file


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


def update(root: Path, name: str, fetchers: Fetchers = Fetchers()) -> str:
    pins = collect_pins(root, fetch=fetchers.fetch, fetch_text=fetchers.fetch_text)
    pin = next((pin for pin in pins if pin.name == name), None)
    if pin is None:
        raise PinFreshnessError(f"unknown pin {name!r}")
    latest = pin.latest()
    if version_key(latest) <= version_key(pin.current):
        return f"{name}: already at {pin.current}"
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
    return (
        f"{name}: {pin.current} -> {latest} (every asset verified); "
        "regenerate with `tools/gen_proof_plan.py --write`"
    )


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--update", metavar="NAME")
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    try:
        if args.check:
            return check(collect_pins(args.root), open_ledger_rows(args.root))
        print(update(args.root, args.update))
        return 0
    except (
        PinFreshnessError,
        tool_releases.ToolReleaseError,
        BinaryenConfigError,
        LlvmToolchainConfigError,
        WasiSdkIdentityError,
        KeyError,
        OSError,
        ValueError,
    ) as exc:
        print(f"pin-freshness: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
