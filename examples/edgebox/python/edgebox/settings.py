# edgebox/settings.py -- Configuration resolution
#
# Config hierarchy (highest priority first):
#   1. Environment variables   (EDGEBOX_<PLUGIN>_<KEY>)
#   2. Manifest YAML           (plugins.*.config section)
#   3. Plugin defaults          (PluginConfig.default_config)
#
# No YAML parser is included -- manifest config is passed as a dict
# by the loader. This module handles merging and env var overrides.

import json
import os


# ---------------------------------------------------------------------------
# Settings
# ---------------------------------------------------------------------------


class Settings:
    """Resolved configuration for a box and its plugins.

    Usage:
        settings = Settings(manifest_config={"github": {"webhook_verify": True}})
        settings.register_defaults("github", {"webhook_verify": False, "timeout": 30})
        val = settings.get("github", "webhook_verify")  # True (manifest wins)
        val = settings.get("github", "timeout")          # 30 (default)
    """

    def __init__(self, manifest_config=None):
        # manifest_config: dict of plugin_name -> {key: value}
        self._manifest = manifest_config if manifest_config is not None else {}
        self._defaults = {}  # plugin_name -> {key: value}
        self._cache = {}  # (plugin_name, key) -> resolved value

    def register_defaults(self, plugin_name, defaults):
        """Register default config values for a plugin."""
        self._defaults[plugin_name] = defaults
        # Invalidate cache for this plugin
        keys = list(self._cache.keys())
        idx = 0
        while idx < len(keys):
            k = keys[idx]
            idx = idx + 1
            if k[0] == plugin_name:
                del self._cache[k]

    def get(self, plugin_name, key, fallback=None):
        """Resolve a config value using the hierarchy.

        Resolution order:
            1. EDGEBOX_<PLUGIN>_<KEY> environment variable
            2. manifest_config[plugin_name][key]
            3. defaults[plugin_name][key]
            4. fallback parameter
        """
        cache_key = (plugin_name, key)
        if cache_key in self._cache:
            return self._cache[cache_key]

        result = self._resolve(plugin_name, key, fallback)
        self._cache[cache_key] = result
        return result

    def get_plugin_config(self, plugin_name):
        """Return the full resolved config dict for a plugin."""
        # Start with defaults
        result = {}
        defaults = self._defaults.get(plugin_name, {})
        keys = list(defaults.keys())
        idx = 0
        while idx < len(keys):
            k = keys[idx]
            idx = idx + 1
            result[k] = self.get(plugin_name, k)
        # Layer in manifest keys not in defaults
        manifest = self._manifest.get(plugin_name, {})
        keys = list(manifest.keys())
        idx = 0
        while idx < len(keys):
            k = keys[idx]
            idx = idx + 1
            if k not in result:
                result[k] = self.get(plugin_name, k)
        return result

    def set(self, plugin_name, key, value):
        """Override a config value at runtime (highest priority)."""
        cache_key = (plugin_name, key)
        self._cache[cache_key] = value

    def _resolve(self, plugin_name, key, fallback):
        """Walk the config hierarchy to resolve a value."""
        # 1. Environment variable: EDGEBOX_GITHUB_WEBHOOK_VERIFY
        env_key = "EDGEBOX_" + plugin_name.upper() + "_" + key.upper()
        env_val = os.environ.get(env_key)
        if env_val is not None:
            return _coerce_env_value(env_val)

        # 2. Manifest config
        manifest_plugin = self._manifest.get(plugin_name, {})
        if key in manifest_plugin:
            return manifest_plugin[key]

        # 3. Plugin defaults
        plugin_defaults = self._defaults.get(plugin_name, {})
        if key in plugin_defaults:
            return plugin_defaults[key]

        # 4. Fallback
        return fallback


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _coerce_env_value(val):
    """Try to parse an env var value as JSON; fall back to string.

    This allows env vars like EDGEBOX_GITHUB_TIMEOUT=30 to resolve
    as int 30, and EDGEBOX_GITHUB_VERIFY=true as bool True.
    """
    if val == "":
        return val
    try:
        return json.loads(val)
    except (ValueError, TypeError):
        return val


def load_manifest_config(manifest_dict):
    """Extract the plugins config section from a parsed manifest dict.

    Expected structure:
        {
            "name": "my-box",
            "plugins": ["edgebox.plugins.github", ...],
            "config": {
                "github": {"webhook_verify": true, ...},
                "slack": {"channel": "#ops", ...},
            }
        }

    Returns the "config" dict, or empty dict if not present.
    """
    if manifest_dict is None:
        return {}
    return manifest_dict.get("config", {})


def load_manifest_plugins(manifest_dict):
    """Extract the plugin list from a parsed manifest dict.

    Returns:
        List of plugin module paths, e.g.:
        ["edgebox.plugins.github", "edgebox.plugins.slack"]
    """
    if manifest_dict is None:
        return []
    return manifest_dict.get("plugins", [])
