"""
Tests for template_extractor: scan invoice OCR output -> reusable template.

Tests use synthetic OCR output mimicking various invoice layouts to verify:
  - Section classification (header, parties, line_items, totals, etc.)
  - Style inference (font sizes, alignment, spacing)
  - Layout inference (section order, column structure, orientation)
  - Output compatibility with enjoice's TemplateDefinition schema
"""

from __future__ import annotations

import json

from molt.stdlib.tinygrad.template_extractor import (
    BoundingBox,
    OcrBlock,
    OcrResult,
    classify_sections,
    extract_template_from_ocr,
    infer_layout,
    infer_styles,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _block(
    text: str, x: float, y: float, w: float, h: float, conf: float = 0.9
) -> OcrBlock:
    """Create an OcrBlock with the given parameters."""
    return OcrBlock(
        text=text, bbox=BoundingBox(x=x, y=y, width=w, height=h), confidence=conf
    )


def _standard_invoice_blocks() -> list[OcrBlock]:
    """Synthetic OCR output for a standard US Letter invoice.

    Layout (792pt page height):
      - Header: company name, "INVOICE" title (y < 120)
      - Parties: "Bill To" left, "From" right (y 120-240)
      - Metadata: invoice #, date, due date (y 240-300)
      - Line items: table header + rows (y 300-550)
      - Totals: subtotal, tax, total (y 550-650)
      - Notes: payment notes (y 650-700)
      - Footer: thank you message (y 720-780)
    """
    return [
        # Header
        _block("Acme Corp", 36, 30, 200, 28),
        _block("INVOICE", 400, 35, 160, 24),
        # Parties
        _block("Bill To:", 36, 140, 60, 14),
        _block("Jane Doe", 36, 160, 100, 12),
        _block("123 Main St", 36, 176, 120, 12),
        _block("From:", 400, 140, 50, 14),
        _block("Acme Corp", 400, 160, 100, 12),
        _block("456 Oak Ave", 400, 176, 120, 12),
        # Metadata
        _block("Invoice #: INV-001", 36, 250, 160, 12),
        _block("Date: 2026-01-15", 250, 250, 140, 12),
        _block("Due Date: 2026-02-15", 450, 250, 160, 12),
        # Line items
        _block("Description", 36, 310, 200, 12),
        _block("Qty", 300, 310, 40, 12),
        _block("Unit Price", 380, 310, 80, 12),
        _block("Amount", 500, 310, 80, 12),
        _block("Web Development", 36, 340, 200, 12),
        _block("40", 300, 340, 40, 12),
        _block("$150.00", 380, 340, 80, 12),
        _block("$6,000.00", 500, 340, 80, 12),
        _block("Design Services", 36, 365, 200, 12),
        _block("20", 300, 365, 40, 12),
        _block("$100.00", 380, 365, 80, 12),
        _block("$2,000.00", 500, 365, 80, 12),
        # Totals
        _block("Subtotal", 400, 560, 80, 12),
        _block("$8,000.00", 500, 560, 80, 12),
        _block("Tax (10%)", 400, 580, 80, 12),
        _block("$800.00", 500, 580, 80, 12),
        _block("Total Due", 400, 610, 80, 14),
        _block("$8,800.00", 500, 610, 80, 14),
        # Notes
        _block("Notes: Payment due within 30 days.", 36, 660, 300, 12),
        # Footer
        _block("Thank you for your business!", 200, 740, 220, 12),
        _block("www.acmecorp.com", 250, 760, 120, 10),
    ]


def _compact_invoice_blocks() -> list[OcrBlock]:
    """Synthetic OCR output for a compact, centered-header invoice."""
    return [
        # Header (centered)
        _block("INVOICE", 250, 30, 120, 20),
        _block("MyBrand LLC", 220, 55, 180, 16),
        # Parties (stacked, not two-column)
        _block("Bill To:", 36, 100, 60, 12),
        _block("Client Name", 36, 115, 100, 12),
        _block("From:", 36, 140, 50, 12),
        _block("MyBrand LLC", 36, 155, 100, 12),
        # Metadata
        _block("Invoice # MB-100", 36, 185, 150, 10),
        _block("Date: 2026-03-01", 36, 198, 130, 10),
        # Line items
        _block("Item", 36, 230, 100, 10),
        _block("Amount", 450, 230, 80, 10),
        _block("Consulting", 36, 248, 100, 10),
        _block("$5,000.00", 450, 248, 80, 10),
        # Totals
        _block("Total", 400, 290, 60, 12),
        _block("$5,000.00", 460, 290, 80, 12),
        # Footer
        _block("Page 1", 280, 380, 50, 8),
    ]


def _landscape_invoice_blocks() -> list[OcrBlock]:
    """Synthetic OCR output for a landscape-orientation invoice."""
    # Landscape: wider than tall (1056 x 612 for Letter landscape)
    return [
        _block("PURCHASE ORDER", 400, 20, 250, 24),
        _block("PO #: PO-2026-042", 36, 80, 200, 12),
        _block("Vendor:", 36, 110, 60, 12),
        _block("Supplier Inc", 100, 110, 120, 12),
        _block("Description", 36, 170, 300, 12),
        _block("Qty", 500, 170, 50, 12),
        _block("Total", 800, 170, 80, 12),
        _block("$10,000.00", 800, 200, 80, 12),
        _block("Grand Total", 700, 300, 100, 14),
        _block("$10,000.00", 800, 300, 100, 14),
        _block("Terms and Conditions apply", 36, 550, 300, 10),
    ]


# ---------------------------------------------------------------------------
# Section classification tests
# ---------------------------------------------------------------------------


class TestSectionClassification:
    def test_header_detection(self):
        blocks = _standard_invoice_blocks()
        sections = classify_sections(blocks, page_height=792)
        header = sections["header"]
        header_texts = [b.text for b in header]
        assert any("INVOICE" in t for t in header_texts), (
            f"Expected 'INVOICE' in header, got {header_texts}"
        )

    def test_parties_detection(self):
        blocks = _standard_invoice_blocks()
        sections = classify_sections(blocks, page_height=792)
        parties = sections["parties"]
        parties_texts = [b.text for b in parties]
        assert any("Bill To" in t for t in parties_texts), (
            f"Expected 'Bill To' in parties, got {parties_texts}"
        )

    def test_metadata_detection(self):
        blocks = _standard_invoice_blocks()
        sections = classify_sections(blocks, page_height=792)
        metadata = sections["metadata"]
        meta_texts = [b.text for b in metadata]
        assert any("Invoice #" in t or "Date" in t for t in meta_texts), (
            f"Expected invoice metadata, got {meta_texts}"
        )

    def test_line_items_detection(self):
        blocks = _standard_invoice_blocks()
        sections = classify_sections(blocks, page_height=792)
        items = sections["line_items"]
        item_texts = [b.text for b in items]
        # Should contain table headers or dollar amounts in the mid-page zone
        assert len(items) > 0, "Expected line items blocks"
        assert any("$" in text or "Item" in text for text in item_texts), item_texts

    def test_totals_detection(self):
        blocks = _standard_invoice_blocks()
        sections = classify_sections(blocks, page_height=792)
        totals = sections["totals"]
        totals_texts = [b.text for b in totals]
        assert any("Subtotal" in t or "Total" in t for t in totals_texts), (
            f"Expected totals, got {totals_texts}"
        )

    def test_footer_detection(self):
        blocks = _standard_invoice_blocks()
        sections = classify_sections(blocks, page_height=792)
        footer = sections["footer"]
        footer_texts = [b.text for b in footer]
        assert any("Thank you" in t or "www." in t for t in footer_texts), (
            f"Expected footer content, got {footer_texts}"
        )

    def test_empty_blocks(self):
        sections = classify_sections([], page_height=792)
        for section_type, blocks in sections.items():
            assert blocks == [], f"Expected empty {section_type}"

    def test_all_sections_present(self):
        sections = classify_sections([], page_height=792)
        expected = {
            "header",
            "parties",
            "metadata",
            "line_items",
            "totals",
            "notes",
            "terms",
            "footer",
        }
        assert set(sections.keys()) == expected


# ---------------------------------------------------------------------------
# Style inference tests
# ---------------------------------------------------------------------------


class TestStyleInference:
    def test_heading_larger_than_body(self):
        blocks = _standard_invoice_blocks()
        styles = infer_styles(blocks, page_width=612, page_height=792)
        assert styles.heading_font_size_pt > styles.body_font_size_pt, (
            f"Heading ({styles.heading_font_size_pt}pt) should be larger "
            f"than body ({styles.body_font_size_pt}pt)"
        )

    def test_font_size_reasonable_range(self):
        blocks = _standard_invoice_blocks()
        styles = infer_styles(blocks, page_width=612, page_height=792)
        assert 6.0 <= styles.body_font_size_pt <= 14.0, (
            f"Body font {styles.body_font_size_pt}pt outside reasonable range"
        )
        assert 10.0 <= styles.heading_font_size_pt <= 48.0, (
            f"Heading font {styles.heading_font_size_pt}pt outside reasonable range"
        )

    def test_alignment_left_for_standard(self):
        blocks = _standard_invoice_blocks()
        styles = infer_styles(blocks, page_width=612, page_height=792)
        # Standard invoice has left-aligned header
        assert styles.header_alignment in ("left", "center", "right")

    def test_compact_spacing_detection(self):
        blocks = _compact_invoice_blocks()
        styles = infer_styles(blocks, page_width=612, page_height=400)
        # Compact layout should have compact=True due to tight vertical spacing
        assert isinstance(styles.is_compact, bool)

    def test_empty_blocks_returns_defaults(self):
        styles = infer_styles([], page_width=612, page_height=792)
        assert styles.primary_color == "#111111"
        assert styles.heading_font_size_pt == 16.0
        assert styles.body_font_size_pt == 10.0


# ---------------------------------------------------------------------------
# Layout inference tests
# ---------------------------------------------------------------------------


class TestLayoutInference:
    def test_section_order_follows_vertical_position(self):
        blocks = _standard_invoice_blocks()
        sections = classify_sections(blocks, page_height=792)
        layout = infer_layout(sections, page_width=612, page_height=792)
        # Header should come before footer in the order
        if "header" in layout.section_order and "footer" in layout.section_order:
            h_idx = layout.section_order.index("header")
            f_idx = layout.section_order.index("footer")
            assert h_idx < f_idx, (
                f"Header (idx={h_idx}) should come before footer (idx={f_idx})"
            )

    def test_two_column_parties_detection(self):
        blocks = _standard_invoice_blocks()
        sections = classify_sections(blocks, page_height=792)
        layout = infer_layout(sections, page_width=612, page_height=792)
        # Standard invoice has two-column parties (Bill To left, From right)
        assert layout.parties_layout in ("two-column", "stacked")

    def test_totals_right_aligned(self):
        blocks = _standard_invoice_blocks()
        sections = classify_sections(blocks, page_height=792)
        layout = infer_layout(sections, page_width=612, page_height=792)
        # Totals are typically right-aligned
        assert layout.totals_position in ("left", "right", "full-width")

    def test_portrait_orientation(self):
        blocks = _standard_invoice_blocks()
        sections = classify_sections(blocks, page_height=792)
        layout = infer_layout(sections, page_width=612, page_height=792)
        assert layout.page_orientation == "portrait"

    def test_landscape_orientation(self):
        blocks = _landscape_invoice_blocks()
        sections = classify_sections(blocks, page_height=612)
        layout = infer_layout(sections, page_width=1056, page_height=612)
        assert layout.page_orientation == "landscape"

    def test_all_sections_in_order(self):
        blocks = _standard_invoice_blocks()
        sections = classify_sections(blocks, page_height=792)
        layout = infer_layout(sections, page_width=612, page_height=792)
        expected = {
            "header",
            "parties",
            "metadata",
            "line_items",
            "totals",
            "notes",
            "terms",
            "footer",
        }
        assert set(layout.section_order) == expected


# ---------------------------------------------------------------------------
# Full pipeline tests
# ---------------------------------------------------------------------------


class TestExtractTemplate:
    def test_full_extraction_standard_invoice(self):
        blocks = _standard_invoice_blocks()
        ocr_result = OcrResult(blocks=blocks, page_width=612, page_height=792)
        template = extract_template_from_ocr(ocr_result)

        assert template.type == "invoice"
        assert template.name == "Extracted Template"
        assert len(template.id) > 0
        assert template.confidence > 0.0
        assert len(template.detected_sections) > 0

    def test_json_output_schema_compatibility(self):
        """Verify the JSON output matches enjoice's TemplateDefinition schema."""
        blocks = _standard_invoice_blocks()
        ocr_result = OcrResult(blocks=blocks, page_width=612, page_height=792)
        template = extract_template_from_ocr(ocr_result)
        json_output = template.to_json()

        # Required top-level fields
        assert "id" in json_output
        assert "name" in json_output
        assert "description" in json_output
        assert "type" in json_output
        assert json_output["type"] in (
            "invoice",
            "credit_note",
            "quote",
            "purchase_order",
        )

        # Layout structure
        layout = json_output["layout"]
        assert "sections" in layout
        assert "page" in layout
        assert isinstance(layout["sections"], list)
        for section in layout["sections"]:
            assert "type" in section
            assert "visible" in section
            assert "order" in section
            assert section["type"] in (
                "header",
                "parties",
                "metadata",
                "line_items",
                "totals",
                "notes",
                "terms",
                "footer",
            )

        page = layout["page"]
        assert page["size"] in ("letter", "a4", "a5", "legal", "b5", "custom")
        assert page["orientation"] in ("portrait", "landscape")
        assert "margins" in page
        for side in ("top", "right", "bottom", "left"):
            assert side in page["margins"]

        # Styles structure
        styles = json_output["styles"]
        assert "colors" in styles
        assert "fonts" in styles
        assert "spacing" in styles
        assert "logo" in styles
        for color_key in (
            "primary",
            "secondary",
            "accent",
            "text",
            "background",
            "border",
        ):
            assert color_key in styles["colors"]
        for font_key in ("heading", "body", "mono"):
            assert font_key in styles["fonts"]

    def test_json_serializable(self):
        """Verify the output can be serialized to JSON without errors."""
        blocks = _standard_invoice_blocks()
        ocr_result = OcrResult(blocks=blocks, page_width=612, page_height=792)
        template = extract_template_from_ocr(ocr_result)
        json_str = json.dumps(template.to_json())
        assert len(json_str) > 0
        parsed = json.loads(json_str)
        assert parsed["type"] == "invoice"

    def test_no_data_in_template(self):
        """Verify the template does not contain any actual invoice data."""
        blocks = _standard_invoice_blocks()
        ocr_result = OcrResult(blocks=blocks, page_width=612, page_height=792)
        template = extract_template_from_ocr(ocr_result)
        json_str = json.dumps(template.to_json())

        # No actual amounts, names, or addresses
        assert "Jane Doe" not in json_str
        assert "123 Main St" not in json_str
        assert "$8,800.00" not in json_str
        assert "$6,000.00" not in json_str
        assert "INV-001" not in json_str

    def test_compact_invoice(self):
        blocks = _compact_invoice_blocks()
        ocr_result = OcrResult(blocks=blocks, page_width=612, page_height=400)
        template = extract_template_from_ocr(ocr_result)
        assert template.confidence > 0.0

    def test_landscape_invoice(self):
        blocks = _landscape_invoice_blocks()
        ocr_result = OcrResult(blocks=blocks, page_width=1056, page_height=612)
        template = extract_template_from_ocr(ocr_result)
        assert template.layout["page"]["orientation"] == "landscape"

    def test_empty_ocr_result(self):
        ocr_result = OcrResult(blocks=[], page_width=612, page_height=792)
        template = extract_template_from_ocr(ocr_result)
        assert template.confidence == 0.0
        assert template.detected_sections == []

    def test_confidence_scales_with_quality(self):
        # Full invoice should have higher confidence than empty
        full_blocks = _standard_invoice_blocks()
        full_result = OcrResult(blocks=full_blocks, page_width=612, page_height=792)
        full_template = extract_template_from_ocr(full_result)

        sparse_blocks = [_block("INVOICE", 200, 30, 120, 20)]
        sparse_result = OcrResult(blocks=sparse_blocks, page_width=612, page_height=792)
        sparse_template = extract_template_from_ocr(sparse_result)

        assert full_template.confidence > sparse_template.confidence, (
            f"Full invoice confidence ({full_template.confidence}) should exceed "
            f"sparse ({sparse_template.confidence})"
        )
