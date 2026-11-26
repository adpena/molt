"""Command-line implementation modules and their process entry point.

The package initializer carries no command dispatch. Executable front doors
enter through ``molt.cli.__main__.run``, and in-process callers use
``molt.cli.entrypoint.main``.
"""
