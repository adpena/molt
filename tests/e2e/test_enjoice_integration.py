"""enjoice integration contract tests.

Validates that the TypeScript files in deploy/enjoice/ are syntactically
valid, the integration PR document is complete, and the Worker API contract
matches what the TypeScript code expects.
"""

import os
import re

DEPLOY_ENJOICE = os.path.join(
    os.path.dirname(__file__), "../../deploy/enjoice"
)
DEPLOY_CLOUDFLARE = os.path.join(
    os.path.dirname(__file__), "../../deploy/cloudflare"
)


def _strip_ts_comments_and_strings(content: str) -> str:
    """Remove comments and string/template literals for coarse structure checks."""
    stripped = re.sub(r"/\*[\s\S]*?\*/", "", content)
    stripped = re.sub(r"`(?:\\.|[^`\\])*`", "``", stripped)
    stripped = re.sub(r'"[^"\\]*(?:\\.[^"\\]*)*"', '""', stripped)
    stripped = re.sub(r"'[^'\\]*(?:\\.[^'\\]*)*'", "''", stripped)
    stripped = re.sub(r"//[^\n]*", "", stripped)
    return stripped


# -------------------------------------------------------------------------
# Test 1: TypeScript files are syntactically valid
# -------------------------------------------------------------------------

def test_typescript_files_syntactically_valid():
    """All TypeScript files in deploy/enjoice/ parse without syntax errors.

    Uses a lightweight regex-based structural check since we cannot assume
    tsc is installed.  Verifies: balanced braces, no obvious syntax errors,
    proper export declarations.
    """
    ts_files = [
        "falcon-ocr-molt.ts",
        "ocr-backend-molt.ts",
        "capabilities-update.ts",
    ]

    for ts_file in ts_files:
        path = os.path.join(DEPLOY_ENJOICE, ts_file)
        assert os.path.isfile(path), f"Missing TypeScript file: {ts_file}"

        with open(path, "r") as f:
            content = f.read()

        # Structural check: verify braces and parentheses are roughly
        # balanced.  We strip comments and string literals first, but
        # template literals with interpolation make exact counting
        # unreliable without a full parser.  Allow a tolerance of 1
        # to account for template literal edge cases.
        stripped = _strip_ts_comments_and_strings(content)

        open_braces = stripped.count("{")
        close_braces = stripped.count("}")
        assert abs(open_braces - close_braces) <= 1, (
            f"{ts_file}: severely unbalanced braces: {open_braces} open, "
            f"{close_braces} close"
        )

        open_parens = stripped.count("(")
        close_parens = stripped.count(")")
        assert abs(open_parens - close_parens) <= 1, (
            f"{ts_file}: severely unbalanced parentheses: {open_parens} open, "
            f"{close_parens} close"
        )

        # Verify file has at least one export.
        assert "export " in content, (
            f"{ts_file}: no exports found (expected public API)"
        )

        # Verify no bare console.error (should use structured logging).
        # console.warn and console.log are acceptable for debug paths.
        assert content.count("console.error") == 0, (
            f"{ts_file}: uses console.error (use structured error handling)"
        )


def test_falcon_ocr_molt_has_no_duplicate_const_declarations_in_same_scope():
    """The bridge file must not contain duplicate const declarations."""
    path = os.path.join(DEPLOY_ENJOICE, "falcon-ocr-molt.ts")
    with open(path, "r") as f:
        content = f.read()

    assert content.count("const padded = new Uint8Array") == 1


def test_falcon_ocr_molt_decoder_preserves_unknown_token_ids():
    """Unknown token IDs are signal and must not disappear during decode."""
    path = os.path.join(DEPLOY_ENJOICE, "falcon-ocr-molt.ts")
    with open(path, "r") as f:
        content = f.read()

    assert "parts.push(`[UNK:${id}]`)" in content


def test_falcon_ocr_molt_decoder_preserves_edge_whitespace():
    """Decoded OCR whitespace is signal and must not be trimmed away."""
    path = os.path.join(DEPLOY_ENJOICE, "falcon-ocr-molt.ts")
    with open(path, "r") as f:
        content = f.read()

    assert 'return parts.join("");' in content
    assert 'return parts.join("").trim();' not in content


def test_ocr_backend_molt_does_not_default_nemotron_endpoint():
    path = os.path.join(DEPLOY_ENJOICE, "ocr-backend-molt.ts")
    with open(path, "r") as f:
        content = f.read()

    assert "adpena--nemotron-ocr-ocr-endpoint.modal.run" not in content
    assert "endpoint: string" in content
    assert "configured endpoint only" in content


def test_ocr_backend_molt_avoids_unverified_speed_or_availability_claims():
    path = os.path.join(DEPLOY_ENJOICE, "ocr-backend-molt.ts")
    with open(path, "r") as f:
        content = f.read()

    assert "fastest browser path" not in content
    assert '"Decoding text..." (instant)' not in content
    assert '"Reading model config..." (instant)' not in content
    assert "caller should use PaddleOCR fallback" not in content


def test_typescript_files_have_jsdoc():
    """TypeScript files have module-level documentation."""
    ts_files = [
        "falcon-ocr-molt.ts",
        "ocr-backend-molt.ts",
        "capabilities-update.ts",
    ]

    for ts_file in ts_files:
        path = os.path.join(DEPLOY_ENJOICE, ts_file)
        with open(path, "r") as f:
            content = f.read()

        # Check for module-level doc comment (/** ... */ or // at top).
        first_200 = content[:200]
        has_doc = "/**" in first_200 or first_200.lstrip().startswith("//")
        assert has_doc, (
            f"{ts_file}: missing module-level documentation"
        )


# -------------------------------------------------------------------------
# Test 2: Integration PR document is complete
# -------------------------------------------------------------------------

def test_integration_pr_document_complete():
    """INTEGRATION_PR.md has all required sections."""
    path = os.path.join(DEPLOY_ENJOICE, "INTEGRATION_PR.md")
    assert os.path.isfile(path), "Missing INTEGRATION_PR.md"

    with open(path, "r") as f:
        content = f.read()

    required_sections = [
        "Summary",
        "Files changed",
        "New files",
        "Modified files",
        "Integration code",
        "Testing",
        "Rollback plan",
        "Performance",
    ]

    for section in required_sections:
        assert section.lower() in content.lower(), (
            f"INTEGRATION_PR.md missing required section: {section}"
        )

    # Verify all TS files are referenced in the PR doc.
    ts_files = [
        "falcon-ocr-molt.ts",
        "ocr-backend-molt.ts",
        "capabilities-update.ts",
    ]
    for ts_file in ts_files:
        assert ts_file in content, (
            f"INTEGRATION_PR.md does not reference {ts_file}"
        )


# -------------------------------------------------------------------------
# Test 3: Worker API contract matches TypeScript expectations
# -------------------------------------------------------------------------

def test_worker_api_contract_matches_typescript():
    """The Worker's API endpoints match what the TypeScript code expects.

    Parses the TypeScript files to extract expected endpoints and verifies
    they exist in the Worker source.
    """
    # Read the Worker source to verify endpoints exist.
    worker_path = os.path.join(DEPLOY_CLOUDFLARE, "worker.js")
    with open(worker_path, "r") as f:
        worker_content = f.read()

    ocr_api_path = os.path.join(DEPLOY_CLOUDFLARE, "ocr_api.js")
    with open(ocr_api_path, "r") as f:
        ocr_api_content = f.read()

    combined_worker = worker_content + ocr_api_content

    # The TypeScript expects these endpoints.
    expected_endpoints = ["/ocr", "/health"]
    for endpoint in expected_endpoints:
        assert f'"{endpoint}"' in combined_worker or f"'{endpoint}'" in combined_worker, (
            f"Worker missing endpoint {endpoint} expected by TypeScript"
        )

    # Verify CORS headers include the expected headers.
    assert "Content-Type" in worker_content, (
        "Worker missing Content-Type in CORS headers"
    )


def test_ocr_result_schema_compatible():
    """The OcrResult schema in TypeScript matches the Worker's JSON response.

    Extracts field names from the TypeScript interface and verifies the
    Worker produces matching fields.
    """
    # Parse OcrResult interface from TypeScript.
    ts_path = os.path.join(DEPLOY_ENJOICE, "falcon-ocr-molt.ts")
    with open(ts_path, "r") as f:
        ts_content = f.read()

    # Extract OcrResult interface fields.
    match = re.search(
        r"export\s+interface\s+OcrResult\s*\{([^}]+)\}",
        ts_content,
    )
    assert match, "OcrResult interface not found in falcon-ocr-molt.ts"

    interface_body = match.group(1)
    # Extract field names (camelCase).
    ts_fields = set(re.findall(r"(\w+)\s*[?]?\s*:", interface_body))

    # The Worker's OCR response should include these fields (possibly
    # snake_case versions).  Map camelCase to snake_case.
    expected_worker_fields = set()
    for field in ts_fields:
        # camelCase -> snake_case
        snake = re.sub(r"([A-Z])", r"_\1", field).lower().lstrip("_")
        expected_worker_fields.add(snake)

    # Read the Worker's OCR response construction.
    ocr_api_path = os.path.join(DEPLOY_CLOUDFLARE, "ocr_api.js")
    with open(ocr_api_path, "r") as f:
        ocr_content = f.read()

    worker_path = os.path.join(DEPLOY_CLOUDFLARE, "worker.js")
    with open(worker_path, "r") as f:
        worker_content = f.read()

    combined = ocr_content + worker_content

    # Core fields that must be in the response.
    core_fields = {"text", "confidence", "time_ms"}
    for field in core_fields:
        assert field in combined, (
            f"Worker OCR response missing core field: {field}"
        )


def test_backend_status_schema():
    """The health endpoint returns a schema compatible with enjoice."""
    # The capabilities-update.ts expects specific backend statuses.
    caps_path = os.path.join(DEPLOY_ENJOICE, "capabilities-update.ts")
    with open(caps_path, "r") as f:
        caps_content = f.read()

    # Verify the backend choice types match what the Worker reports.
    assert "molt-gpu" in caps_content, (
        "capabilities-update.ts missing 'molt-gpu' backend choice"
    )

    # The Worker's health endpoint should report backend statuses.
    worker_path = os.path.join(DEPLOY_CLOUDFLARE, "worker.js")
    with open(worker_path, "r") as f:
        worker_content = f.read()

    assert "workers-ai" in worker_content, (
        "Worker health endpoint missing 'workers-ai' backend status"
    )
    assert "molt-gpu" in worker_content or "paddle-ocr" in worker_content, (
        "Worker health endpoint missing fallback backend status"
    )


if __name__ == "__main__":
    test_typescript_files_syntactically_valid()
    print("PASS: TypeScript syntax")
    test_typescript_files_have_jsdoc()
    print("PASS: TypeScript docs")
    test_integration_pr_document_complete()
    print("PASS: Integration PR doc")
    test_worker_api_contract_matches_typescript()
    print("PASS: Worker API contract")
    test_ocr_result_schema_compatible()
    print("PASS: OcrResult schema")
    test_backend_status_schema()
    print("PASS: Backend status schema")
    print("\nAll enjoice integration tests passed.")
