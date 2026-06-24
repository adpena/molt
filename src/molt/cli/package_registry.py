from __future__ import annotations

import base64
import http.client
import os
from pathlib import Path
import posixpath
from typing import Any
import urllib.parse
import uuid

from molt.cli.compiler_metadata import _compiler_metadata


REMOTE_REGISTRY_SCHEMES = {"http", "https"}


def _is_remote_registry(registry: str) -> bool:
    scheme = urllib.parse.urlparse(registry).scheme.lower()
    return scheme in REMOTE_REGISTRY_SCHEMES


def _validate_registry_url(registry: str) -> str | None:
    parsed = urllib.parse.urlparse(registry)
    if parsed.scheme.lower() not in REMOTE_REGISTRY_SCHEMES:
        return f"Unsupported registry scheme: {parsed.scheme or 'none'}"
    if not parsed.netloc:
        return "Registry URL is missing a host"
    if parsed.username or parsed.password:
        return (
            "Registry URL must not include credentials "
            "(use --registry-token or --registry-user/--registry-password)"
        )
    return None


def _read_secret_value(
    value: str | None, *, env_name: str, label: str, use_env: bool = True
) -> tuple[str | None, str | None]:
    source = None
    if value is None and use_env:
        env_val = os.environ.get(env_name)
        if env_val is not None:
            value = env_val
            source = "env"
    else:
        source = "arg"
    if value is None:
        return None, None
    if value.startswith("@"):
        secret_path = Path(value[1:]).expanduser()
        if not secret_path.exists():
            raise RuntimeError(f"{label} file not found: {secret_path}")
        value = secret_path.read_text()
        source = "file"
    value = value.strip()
    if not value:
        raise RuntimeError(f"{label} is empty")
    return value, source


def _resolve_registry_auth(
    registry_token: str | None,
    registry_user: str | None,
    registry_password: str | None,
) -> tuple[dict[str, str], dict[str, str]]:
    explicit_token = registry_token is not None
    explicit_user = registry_user is not None or registry_password is not None
    if explicit_token and explicit_user:
        raise RuntimeError(
            "Use --registry-token or --registry-user/--registry-password, not both."
        )
    token: str | None = None
    token_source: str | None = None
    if explicit_token:
        token, token_source = _read_secret_value(
            registry_token,
            env_name="MOLT_REGISTRY_TOKEN",
            label="Registry token",
            use_env=False,
        )
    elif not explicit_user:
        token, token_source = _read_secret_value(
            None,
            env_name="MOLT_REGISTRY_TOKEN",
            label="Registry token",
            use_env=True,
        )
    user = None
    user_source = None
    password = None
    password_source = None
    if token is None:
        user = registry_user
        user_source = "arg" if registry_user is not None else None
        if user is None:
            env_user = os.environ.get("MOLT_REGISTRY_USER")
            if env_user is not None:
                user = env_user
                user_source = "env"
        password, password_source = _read_secret_value(
            registry_password,
            env_name="MOLT_REGISTRY_PASSWORD",
            label="Registry password",
            use_env=registry_password is None,
        )
    if user and not password:
        raise RuntimeError("Registry password is required when using --registry-user.")
    if password and not user:
        raise RuntimeError("Registry user is required when using --registry-password.")
    headers: dict[str, str] = {}
    auth_info = {"mode": "none", "source": "none"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
        auth_info["mode"] = "bearer"
        auth_info["source"] = token_source or "unknown"
    elif user:
        credential = f"{user}:{password}"
        encoded = base64.b64encode(credential.encode("utf-8")).decode("ascii")
        headers["Authorization"] = f"Basic {encoded}"
        auth_info["mode"] = "basic"
        sources = {
            source for source in (user_source, password_source) if source is not None
        }
        if len(sources) == 1:
            auth_info["source"] = sources.pop()
        elif len(sources) > 1:
            auth_info["source"] = "mixed"
        else:
            auth_info["source"] = "unknown"
    return headers, auth_info


def _resolve_registry_timeout(value: float | None) -> float:
    timeout = value
    if timeout is None:
        env_val = os.environ.get("MOLT_REGISTRY_TIMEOUT")
        if env_val:
            try:
                timeout = float(env_val)
            except ValueError as exc:
                raise RuntimeError(
                    f"Invalid MOLT_REGISTRY_TIMEOUT value: {env_val}"
                ) from exc
    if timeout is None:
        timeout = 30.0
    if timeout <= 0:
        raise RuntimeError("Registry timeout must be greater than zero.")
    return timeout


def _remote_registry_destination(registry_url: str, filename: str) -> str:
    parsed = urllib.parse.urlparse(registry_url)
    path = parsed.path or ""
    if not path or path.endswith("/"):
        base_path = path or "/"
        if not base_path.endswith("/"):
            base_path += "/"
        dest_path = posixpath.join(base_path, filename)
    else:
        dest_path = path
    return urllib.parse.urlunparse(parsed._replace(path=dest_path))


def _remote_sidecar_url(dest_url: str, suffix: str) -> str:
    parsed = urllib.parse.urlparse(dest_url)
    path = parsed.path
    if not path:
        raise RuntimeError("Remote destination URL is missing a path")
    dir_name, file_name = posixpath.split(path)
    stem = Path(file_name).stem
    sidecar_name = f"{stem}{suffix}"
    if dir_name and not dir_name.endswith("/"):
        sidecar_path = posixpath.join(dir_name, sidecar_name)
    elif dir_name:
        sidecar_path = f"{dir_name}{sidecar_name}"
    else:
        sidecar_path = f"/{sidecar_name}"
    return urllib.parse.urlunparse(parsed._replace(path=sidecar_path))


def _registry_content_type(path: Path) -> str:
    suffix = path.suffix.lower()
    if suffix in {".moltpkg", ".whl"}:
        return "application/zip"
    if suffix == ".json":
        return "application/json"
    return "application/octet-stream"


def _upload_registry_file(
    source: Path,
    dest_url: str,
    headers: dict[str, str],
    timeout: float,
) -> dict[str, Any]:
    parsed = urllib.parse.urlparse(dest_url)
    scheme = parsed.scheme.lower()
    host = parsed.hostname
    if not host:
        raise RuntimeError(f"Invalid registry URL: {dest_url}")
    if scheme not in REMOTE_REGISTRY_SCHEMES:
        raise RuntimeError(f"Unsupported registry scheme: {scheme}")
    port = parsed.port
    path = parsed.path or "/"
    if parsed.params:
        path = f"{path};{parsed.params}"
    if parsed.query:
        path = f"{path}?{parsed.query}"
    conn_cls: type[http.client.HTTPConnection]
    if scheme == "https":
        conn_cls = http.client.HTTPSConnection
    else:
        conn_cls = http.client.HTTPConnection
    content_length = source.stat().st_size
    upload_headers = {
        "Content-Type": _registry_content_type(source),
        "Content-Length": str(content_length),
        "User-Agent": f"molt/{_compiler_metadata()[0] or 'unknown'}",
        "X-Molt-Upload-Id": str(uuid.uuid4()),
    }
    upload_headers.update(headers)
    conn = conn_cls(host, port, timeout=timeout)
    try:
        conn.putrequest("PUT", path)
        for key, value in upload_headers.items():
            conn.putheader(key, value)
        conn.endheaders()
        with source.open("rb") as handle:
            while True:
                chunk = handle.read(1024 * 64)
                if not chunk:
                    break
                conn.send(chunk)
        response = conn.getresponse()
        body = response.read()
    finally:
        conn.close()
    status = response.status
    if status < 200 or status >= 300:
        detail = body.decode("utf-8", errors="replace").strip()
        if detail:
            detail = f" {detail}"
        raise RuntimeError(
            f"Registry upload failed ({status} {response.reason}).{detail}"
        )
    return {
        "status": status,
        "reason": response.reason,
        "bytes": content_length,
        "etag": response.getheader("ETag"),
    }
