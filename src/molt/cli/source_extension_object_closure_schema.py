"""Schema constants for source-extension object-closure custody."""

SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION = 3
SOURCE_EXTENSION_NATIVE_SYMBOL_AUTHORITY = "native_nm_command_v1"
SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY = "wasm_linking_section_v2"

SOURCE_EXTENSION_OBJECT_CLOSURE_FIELDS = frozenset(
    {
        "schema_version",
        "root_symbol",
        "init_symbol_owner",
        "objects",
        "defined_symbols",
        "undefined_symbols",
        "runtime_symbols",
        "wasm_imports",
        "required_c_api_symbols",
        "required_capsules",
        "project_generated_c_api_symbols",
        "project_generated_c_api_prefixes",
        "closure_sha256",
    }
)
SOURCE_EXTENSION_OBJECT_FIELDS = frozenset(
    {
        "source",
        "language",
        "object",
        "source_sha256",
        "object_sha256",
        "compile_command",
        "compile_command_ref",
        "compile_command_operands",
        "symbol_command",
        "symbol_command_ref",
        "symbol_authority",
        "dependencies",
        "dependencies_ref",
        "defined_symbols",
        "defined_symbols_ref",
        "undefined_symbols",
        "undefined_symbols_ref",
        "required_c_api_symbols",
        "required_c_api_symbols_ref",
        "required_capsules",
        "required_capsules_ref",
        "project_generated_c_api_symbols",
        "project_generated_c_api_symbols_ref",
        "unit_sha256",
    }
)
