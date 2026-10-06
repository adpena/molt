"""Project one assembled release bundle into a pip-installable platform wheel.

Ordinary ``pip install molt`` on a released platform receives the same ready
distribution as a bundle/package-manager install: the production compiler,
launcher, declared runtime cells and signed source manifest. The wheel carries
the exact bundle tree in its data scheme at ``share/molt/distribution`` and the
unchanged ``molt`` package from the canonical pure wheel. At run time the CLI
selects that packaged distribution (``molt.source_root``), admits it through
``molt.compiler_distribution`` and binds its executing package to the signed
source inventory. There is no build step and no source fallback: on platforms
without a platform wheel, pip selects the pure wheel, whose CLI requires an
explicit ``MOLT_SOURCE_ROOT`` source checkout.
"""

from __future__ import annotations

from molt.temporary_artifacts import OwnedTemporaryDirectory

import base64
import csv
import io
from pathlib import Path, PurePosixPath
import shutil
from typing import Callable
import zipfile

from packaging.utils import parse_wheel_filename

from molt.portable_paths import portable_relative_path
from molt.source_root import PACKAGED_DISTRIBUTION_PATH
from molt.toolchain_identity import stable_regular_file_content_identity

from .archive import ArchivePolicy, write_reproducible_zip
from .binary_compatibility import wheel_platform_tag_matches


def _record_hash(path: Path) -> tuple[str, int]:
    identity = stable_regular_file_content_identity(path, label="platform wheel member")
    digest = base64.urlsafe_b64encode(bytes.fromhex(str(identity["sha256"])))
    return "sha256=" + digest.rstrip(b"=").decode("ascii"), int(identity["size"])


def write_platform_wheel(
    bundle_root: Path,
    pure_wheel: Path,
    output_dir: Path,
    *,
    platform: str,
    arch: str,
    platform_tag: str,
    source_date_epoch: int,
    bundle_mode: Callable[[PurePosixPath], int],
    policy: ArchivePolicy,
) -> Path:
    """Write ``molt-<v>-py3-none-<tag>.whl`` from the assembled bundle tree.

    ``platform_tag`` comes from ``binary_compatibility`` for this same tree; the
    caller re-audits the written wheel with that authority.
    """
    if not wheel_platform_tag_matches(platform, arch, platform_tag):
        raise ValueError(f"wheel tag {platform_tag} does not name {platform}/{arch}")
    name, version, build, tags = parse_wheel_filename(pure_wheel.name)
    if name != "molt" or build or {str(tag) for tag in tags} != {"py3-none-any"}:
        raise ValueError(f"not the canonical pure Molt wheel: {pure_wheel.name}")
    distribution = f"molt-{version}"
    dist_info = f"{distribution}.dist-info"
    data = PurePosixPath(f"{distribution}.data", "data", *PACKAGED_DISTRIBUTION_PATH)
    filename = f"{distribution}-py3-none-{platform_tag}.whl"
    with OwnedTemporaryDirectory(prefix="molt-platform-wheel-") as temporary:
        stage = Path(temporary) / "wheel"
        stage.mkdir()
        with zipfile.ZipFile(pure_wheel) as source:
            for info in source.infolist():
                if info.is_dir():
                    continue
                member = portable_relative_path(info.filename)
                if member.parts[0].endswith(".data"):
                    raise ValueError(
                        "the pure Molt wheel unexpectedly has a data scheme"
                    )
                if member.parts[0] == dist_info and member.name in {"RECORD", "WHEEL"}:
                    continue
                destination = stage.joinpath(*member.parts)
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(source.read(info))
        if not (stage / dist_info / "METADATA").is_file():
            raise ValueError("the pure Molt wheel has no METADATA")
        (stage / dist_info / "WHEEL").write_bytes(
            (
                "Wheel-Version: 1.0\n"
                "Generator: molt-release (tools.release.platform_wheel)\n"
                "Root-Is-Purelib: false\n"
                f"Tag: py3-none-{platform_tag}\n"
            ).encode("utf-8")
        )
        shutil.copytree(bundle_root, stage.joinpath(*data.parts))
        record = io.StringIO(newline="")
        writer = csv.writer(record, lineterminator="\n")
        for path in sorted(p for p in stage.rglob("*") if p.is_file()):
            relative = path.relative_to(stage).as_posix()
            writer.writerow((relative, *_record_hash(path)))
        writer.writerow((f"{dist_info}/RECORD", "", ""))
        (stage / dist_info / "RECORD").write_bytes(record.getvalue().encode("utf-8"))

        def mode(relative: PurePosixPath) -> int:
            # pip keeps only the executable bit; bundle members keep the exact
            # bundle/Git mode authority so installed source admission holds.
            if relative.parts[: len(data.parts)] == data.parts and len(
                relative.parts
            ) > len(data.parts):
                return bundle_mode(PurePosixPath(*relative.parts[len(data.parts) :]))
            return 0o755 if (stage / relative).is_dir() else 0o644

        output_dir.mkdir(parents=True, exist_ok=True)
        output = output_dir / filename
        write_reproducible_zip(
            stage,
            output,
            source_date_epoch=source_date_epoch,
            policy=policy,
            mode_resolver=mode,
        )
    return output
