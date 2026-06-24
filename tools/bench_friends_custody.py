from pathlib import Path

from bench_friends_types import SourceCustody, SuiteAcquisition, SuiteSpec

import harness_memory_guard

def _run_git(
    args: list[str],
    *,
    cwd: Path | None,
    timeout_sec: int,
    dry_run: bool,
    limits: harness_memory_guard.HarnessMemoryLimits,
) -> tuple[int, str, str]:
    cmd = ["git", *args]
    if dry_run:
        return 0, "[dry-run]\n", ""
    res = harness_memory_guard.guarded_completed_process(
        cmd,
        prefix="MOLT_BENCH",
        cwd=str(cwd) if cwd is not None else None,
        capture_output=True,
        text=True,
        timeout=timeout_sec,
        limits=limits,
    )
    return res.returncode, res.stdout or "", res.stderr or ""


def _is_placeholder_ref(ref: str) -> bool:
    upper = ref.upper()
    return "PINNED" in upper or "REQUIRED" in upper or "PLACEHOLDER" in upper


def _verify_git_source_custody(
    suite: SuiteSpec,
    *,
    repo_dir: Path,
    requested_ref: str,
    timeout_sec: int,
    dry_run: bool,
    limits: harness_memory_guard.HarnessMemoryLimits,
    suite_root_overridden: bool,
    verification: str = "git_ref_and_clean_tree",
    raise_on_dirty: bool = True,
) -> SourceCustody:
    if dry_run:
        return SourceCustody(
            source=suite.source,
            requested_ref=requested_ref,
            expected_ref=None,
            head_ref=None,
            ref_verified=None,
            git_clean=None,
            git_status_porcelain=None,
            git_ignored_artifacts=None,
            suite_root_overridden=suite_root_overridden,
            verification="dry_run",
        )

    rc, out, err = _run_git(
        ["rev-parse", "--is-inside-work-tree"],
        cwd=repo_dir,
        timeout_sec=timeout_sec,
        dry_run=False,
        limits=limits,
    )
    if rc != 0 or out.strip() != "true":
        detail = err.strip() or out.strip()
        raise RuntimeError(
            f"suite {suite.id}: suite root is not a git checkout: {detail}"
        )

    rc, head_out, err = _run_git(
        ["rev-parse", "HEAD"],
        cwd=repo_dir,
        timeout_sec=timeout_sec,
        dry_run=False,
        limits=limits,
    )
    if rc != 0:
        raise RuntimeError(
            f"suite {suite.id}: git rev-parse HEAD failed: {err.strip()}"
        )
    head_ref = head_out.strip()

    rc, expected_out, err = _run_git(
        ["rev-parse", "--verify", f"{requested_ref}^{{commit}}"],
        cwd=repo_dir,
        timeout_sec=timeout_sec,
        dry_run=False,
        limits=limits,
    )
    if rc != 0:
        raise RuntimeError(
            f"suite {suite.id}: requested repo_ref {requested_ref!r} does not resolve: "
            f"{err.strip()}"
        )
    expected_ref = expected_out.strip()
    ref_verified = bool(expected_ref and expected_ref == head_ref)
    if not ref_verified:
        raise RuntimeError(
            f"suite {suite.id}: checked-out HEAD {head_ref} does not match "
            f"requested repo_ref {requested_ref!r} ({expected_ref})"
        )

    rc, status_out, err = _run_git(
        ["status", "--porcelain"],
        cwd=repo_dir,
        timeout_sec=timeout_sec,
        dry_run=False,
        limits=limits,
    )
    if rc != 0:
        raise RuntimeError(f"suite {suite.id}: git status failed: {err.strip()}")
    git_status = status_out.strip()

    rc, ignored_out, err = _run_git(
        ["ls-files", "--others", "--ignored", "--exclude-standard"],
        cwd=repo_dir,
        timeout_sec=timeout_sec,
        dry_run=False,
        limits=limits,
    )
    if rc != 0:
        raise RuntimeError(
            f"suite {suite.id}: git ignored-file custody scan failed: {err.strip()}"
        )
    ignored_files = ignored_out.strip()
    git_clean = not git_status and not ignored_files
    if raise_on_dirty and git_status:
        raise RuntimeError(
            f"suite {suite.id}: git checkout is dirty; refusing off-the-shelf "
            f"benchmark custody:\n{git_status}"
        )
    if raise_on_dirty and ignored_files:
        raise RuntimeError(
            f"suite {suite.id}: git checkout contains ignored artifacts; refusing "
            f"off-the-shelf benchmark custody:\n{ignored_files}"
        )

    return SourceCustody(
        source=suite.source,
        requested_ref=requested_ref,
        expected_ref=expected_ref,
        head_ref=head_ref,
        ref_verified=True,
        git_clean=git_clean,
        git_status_porcelain=git_status,
        git_ignored_artifacts=ignored_files,
        suite_root_overridden=suite_root_overridden,
        verification=verification,
    )


def _acquire_suite(
    suite: SuiteSpec,
    *,
    repos_root: Path,
    suite_root_override: Path | None,
    checkout: bool,
    fetch: bool,
    timeout_sec: int,
    dry_run: bool,
    limits: harness_memory_guard.HarnessMemoryLimits,
) -> SuiteAcquisition:
    if suite.source == "local":
        local_path = (
            str(suite_root_override) if suite_root_override else suite.local_path
        )
        if not local_path:
            raise ValueError(
                f"suite {suite.id}: local_path is required for source=local"
            )
        suite_root = Path(local_path).expanduser()
        if not dry_run and not suite_root.exists():
            raise FileNotFoundError(
                f"suite {suite.id}: local path not found: {suite_root}"
            )
        suite_workdir = (
            (suite_root / suite.workdir).resolve()
            if suite.workdir
            else suite_root.resolve()
        )
        return SuiteAcquisition(
            suite_root=suite_root,
            suite_workdir=suite_workdir,
            custody=SourceCustody(
                source=suite.source,
                requested_ref=None,
                expected_ref=None,
                head_ref=None,
                ref_verified=None,
                git_clean=None,
                git_status_porcelain=None,
                git_ignored_artifacts=None,
                suite_root_overridden=suite_root_override is not None,
                verification="local_path_exists" if not dry_run else "dry_run",
            ),
        )

    if suite.source != "git":
        raise ValueError(f"suite {suite.id}: unsupported source {suite.source}")
    if not suite.repo_url:
        raise ValueError(f"suite {suite.id}: repo_url is required for source=git")
    if not suite.repo_ref:
        raise ValueError(f"suite {suite.id}: repo_ref is required for source=git")
    if _is_placeholder_ref(suite.repo_ref) and not dry_run:
        raise ValueError(
            f"suite {suite.id}: repo_ref must be set to a pinned commit/tag, "
            "not a placeholder"
        )

    repo_dir = (
        suite_root_override.expanduser()
        if suite_root_override is not None
        else repos_root / suite.id
    )
    if checkout and suite_root_override is None:
        if not repo_dir.exists():
            repo_dir.parent.mkdir(parents=True, exist_ok=True)
            rc, _out, err = _run_git(
                ["clone", suite.repo_url, str(repo_dir)],
                cwd=None,
                timeout_sec=timeout_sec,
                dry_run=dry_run,
                limits=limits,
            )
            if rc != 0:
                raise RuntimeError(f"suite {suite.id}: git clone failed: {err.strip()}")
        if fetch:
            rc, _out, err = _run_git(
                ["fetch", "--all", "--tags", "--prune"],
                cwd=repo_dir,
                timeout_sec=timeout_sec,
                dry_run=dry_run,
                limits=limits,
            )
            if rc != 0:
                raise RuntimeError(f"suite {suite.id}: git fetch failed: {err.strip()}")
        rc, _out, err = _run_git(
            ["checkout", "--detach", suite.repo_ref],
            cwd=repo_dir,
            timeout_sec=timeout_sec,
            dry_run=dry_run,
            limits=limits,
        )
        if rc != 0:
            raise RuntimeError(
                f"suite {suite.id}: git checkout {suite.repo_ref} failed: {err.strip()}"
            )

    if not dry_run and not repo_dir.exists():
        raise FileNotFoundError(
            f"suite {suite.id}: repo checkout missing at {repo_dir}; "
            "run with --checkout or --suite-root <suite>=<path>"
        )
    custody = _verify_git_source_custody(
        suite,
        repo_dir=repo_dir,
        requested_ref=suite.repo_ref,
        timeout_sec=timeout_sec,
        dry_run=dry_run,
        limits=limits,
        suite_root_overridden=suite_root_override is not None,
    )
    suite_workdir = (
        (repo_dir / suite.workdir).resolve() if suite.workdir else repo_dir.resolve()
    )
    return SuiteAcquisition(
        suite_root=repo_dir,
        suite_workdir=suite_workdir,
        custody=custody,
    )


def _post_run_source_custody_failure_reason(
    suite: SuiteSpec,
    custody: SourceCustody,
) -> str | None:
    details: list[str] = []
    if custody.git_status_porcelain:
        details.append(
            "git checkout is dirty after suite execution:\n"
            f"{custody.git_status_porcelain}"
        )
    if custody.git_ignored_artifacts:
        details.append(
            "git checkout contains ignored artifacts after suite execution:\n"
            f"{custody.git_ignored_artifacts}"
        )
    if not details:
        return None
    return f"suite {suite.id}: post-run source custody check failed; " + "\n".join(
        details
    )


def _combine_suite_reasons(left: str | None, right: str | None) -> str | None:
    if not left:
        return right
    if not right:
        return left
    return f"{left}; {right}"
