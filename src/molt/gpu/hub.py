"""
molt.gpu.hub — Download models from HuggingFace Hub.

Usage:
    from molt.gpu.hub import download_model, list_files

    path = download_model("TinyLlama/TinyLlama-1.1B-Chat-v1.0", filename="model.safetensors")
    weights = load_safetensors(path)
"""

import os
import json
import re
import urllib.parse
import urllib.request
import urllib.error
from pathlib import Path

HF_API_URL = "https://huggingface.co/api/models"
HF_CDN_URL = "https://huggingface.co"
CACHE_DIR = Path.home() / ".cache" / "molt" / "hub"

_REVISION_RE = re.compile(r'^[a-zA-Z0-9._-]+$')


def _validate_revision(revision: str) -> None:
    """Reject revision strings that could cause path traversal or URL injection."""
    if not revision or not _REVISION_RE.match(revision):
        raise ValueError(f"Invalid revision: {revision!r}")


def download_model(repo_id: str, filename: str = None, revision: str = "main",
                   cache_dir: str = None) -> str:
    """Download a model file from HuggingFace Hub.

    Returns the local file path.

    Example:
        path = download_model("bert-base-uncased", "model.safetensors")
    """
    if not re.match(r'^[a-zA-Z0-9._-]+/[a-zA-Z0-9._-]+$', repo_id):
        raise ValueError(f"Invalid repo_id: {repo_id}")
    _validate_revision(revision)

    cache = Path(cache_dir) if cache_dir else CACHE_DIR
    repo_dir = cache / repo_id.replace("/", "--")
    repo_dir.mkdir(parents=True, exist_ok=True)

    if filename is None:
        # Try common filenames
        for candidate in ["model.safetensors", "pytorch_model.bin", "model.gguf"]:
            try:
                return download_model(repo_id, candidate, revision, cache_dir)
            except Exception:
                continue
        raise FileNotFoundError(f"No model file found in {repo_id}")

    if '/' in filename or '..' in filename:
        raise ValueError(f"Invalid filename: {filename}")

    local_path = repo_dir / filename
    if local_path.exists():
        return str(local_path)

    # Download
    url = f"{HF_CDN_URL}/{repo_id}/resolve/{urllib.parse.quote(revision, safe='')}/{filename}"
    print(f"Downloading {url}...")

    try:
        # Check for HF token
        token = os.environ.get("HF_TOKEN") or os.environ.get("HUGGING_FACE_HUB_TOKEN")
        req = urllib.request.Request(url)
        if token:
            req.add_header("Authorization", f"Bearer {token}")

        tmp_path = str(local_path) + ".tmp"
        with urllib.request.urlopen(req) as response:
            total = int(response.headers.get('Content-Length', 0))
            downloaded = 0
            chunk_size = 8 * 1024 * 1024  # 8MB chunks

            with open(tmp_path, 'wb') as f:
                while True:
                    chunk = response.read(chunk_size)
                    if not chunk:
                        break
                    f.write(chunk)
                    downloaded += len(chunk)
                    if total > 0:
                        pct = downloaded * 100 // total
                        print(f"\r  {downloaded // (1024*1024)}MB / {total // (1024*1024)}MB ({pct}%)", end="", flush=True)

            print(f"\n  Saved to {local_path}")

        # Atomic rename on success — avoids partial files on interrupt
        os.rename(tmp_path, str(local_path))
        return str(local_path)

    except urllib.error.HTTPError as e:
        if e.code == 404:
            raise FileNotFoundError(f"File '{filename}' not found in {repo_id}")
        elif e.code == 401:
            raise PermissionError(f"Authentication required. Set HF_TOKEN environment variable.")
        raise


def list_files(repo_id: str, revision: str = "main") -> list:
    """List files in a HuggingFace model repository."""
    if not re.match(r'^[a-zA-Z0-9._-]+/[a-zA-Z0-9._-]+$', repo_id):
        raise ValueError(f"Invalid repo_id: {repo_id}")
    _validate_revision(revision)
    url = f"{HF_API_URL}/{repo_id}?revision={urllib.parse.quote(revision, safe='')}"
    try:
        with urllib.request.urlopen(url) as response:
            data = json.loads(response.read())
            siblings = data.get("siblings", [])
            return [s["rfilename"] for s in siblings]
    except Exception as e:
        raise ConnectionError(f"Failed to list files for {repo_id}: {e}")


def model_info(repo_id: str) -> dict:
    """Get metadata about a HuggingFace model."""
    if not re.match(r'^[a-zA-Z0-9._-]+/[a-zA-Z0-9._-]+$', repo_id):
        raise ValueError(f"Invalid repo_id: {repo_id}")
    url = f"{HF_API_URL}/{repo_id}"
    try:
        with urllib.request.urlopen(url) as response:
            return json.loads(response.read())
    except Exception as e:
        raise ConnectionError(f"Failed to get info for {repo_id}: {e}")
