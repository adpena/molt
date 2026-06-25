from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import module_graph as cli_module_graph
from molt.cli import module_source as cli_module_source


_MODULE_SOURCE_ROOT_EXPORT_NAMES = {
    "_ModuleSourceCatalog",
    "_ModuleSourceLease",
    "_build_module_source_catalog",
    "_payload_source_matches",
    "_read_module_source",
    "_source_content_sha256",
    "_source_content_sha256_cached",
}

_MODULE_SOURCE_PRIVATE_NAMES = {
    "_source_hash_stat_identity_is_strong",
    "_write_source_hash_cache_payload",
}


def test_module_source_authority_lives_in_module_source_module() -> None:
    for name in _MODULE_SOURCE_ROOT_EXPORT_NAMES:
        owner = getattr(cli_module_source, name)
        assert inspect.getmodule(owner) is cli_module_source
        assert not hasattr(cli_module_graph, name)
        assert getattr(cli, name) is owner
    for name in _MODULE_SOURCE_PRIVATE_NAMES:
        owner = getattr(cli_module_source, name)
        assert inspect.getmodule(owner) is cli_module_source
        assert not hasattr(cli_module_graph, name)
        assert not hasattr(cli, name)
