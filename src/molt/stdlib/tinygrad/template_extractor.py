"""
Template extractor: scan invoice OCR output and generate a reusable template.

Given OCR output (text blocks with bounding boxes), this module:
  1. Classifies each block into a section (header, parties, metadata,
     line_items, totals, notes, terms, footer).
  2. Infers visual styles: font sizes from bounding box heights,
     alignment from x-positions, spacing from vertical gaps.
  3. Infers layout: section order, column structure, page dimensions.
  4. Produces a TemplateDefinition compatible with enjoice's template
     editor (see @plugin/templates/types.ts).

The output preserves visual structure and branding signals but does NOT
include any actual data (line items, amounts, names, addresses).

Usage:
    from molt.stdlib.tinygrad.template_extractor import extract_template_from_ocr
    template = extract_template_from_ocr(ocr_result)
"""

from __future__ import annotations

import math
import re
import uuid
from dataclasses import dataclass, field
from typing import Any, Optional


# ---------------------------------------------------------------------------
# OCR result types (mirrors the Worker's OCR output structure)
# ---------------------------------------------------------------------------

@dataclass
class BoundingBox:
    """Axis-aligned bounding box in pixel coordinates (top-left origin)."""
    x: float
    y: float
    width: float
    height: float

    @property
    def x2(self) -> float:
        return self.x + self.width

    @property
    def y2(self) -> float:
        return self.y + self.height

    @property
    def center_x(self) -> float:
        return self.x + self.width / 2

    @property
    def center_y(self) -> float:
        return self.y + self.height / 2


@dataclass
class OcrBlock:
    """A single text block from OCR output."""
    text: str
    bbox: BoundingBox
    confidence: float = 0.0


@dataclass
class OcrResult:
    """Complete OCR result for one document."""
    blocks: list[OcrBlock]
    page_width: float = 0.0
    page_height: float = 0.0


# ---------------------------------------------------------------------------
# Section classification
# ---------------------------------------------------------------------------

# Section types matching enjoice's TemplateSectionConfig.type
SECTION_TYPES = (
    "header", "parties", "metadata", "line_items",
    "totals", "notes", "terms", "footer",
)

# Regex patterns for section classification heuristics.
# Each pattern maps to one or more candidate section types.
_HEADER_PATTERNS = re.compile(
    r"(?i)(invoice|credit\s*note|quote|purchase\s*order|receipt|"
    r"tax\s*invoice|proforma|estimate|bill|statement)",
)
_PARTY_PATTERNS = re.compile(
    r"(?i)(bill\s*to|ship\s*to|from|sold\s*to|buyer|seller|"
    r"remit\s*to|vendor|customer|client)",
)
_METADATA_PATTERNS = re.compile(
    r"(?i)(invoice\s*#|invoice\s*no|inv\s*no|date|due\s*date|"
    r"issued|payment\s*terms|terms|p\.?o\.?\s*#|reference|"
    r"order\s*#|account\s*#)",
)
_LINE_ITEM_PATTERNS = re.compile(
    r"(?i)(description|qty|quantity|unit\s*price|amount|"
    r"item|service|product|rate|hours|total)",
)
_TOTALS_PATTERNS = re.compile(
    r"(?i)(subtotal|sub\s*total|tax|vat|gst|discount|"
    r"total\s*due|grand\s*total|balance\s*due|amount\s*due|"
    r"total$)",
)
_NOTES_PATTERNS = re.compile(
    r"(?i)(notes?|memo|comments?|remarks?|additional\s*info)",
)
_TERMS_PATTERNS = re.compile(
    r"(?i)(terms?\s*(&|and)\s*conditions?|payment\s*instructions?|"
    r"bank\s*details|wire\s*transfer|ach|iban|swift|routing|"
    r"late\s*fee|penalty)",
)
_FOOTER_PATTERNS = re.compile(
    r"(?i)(thank\s*you|page\s*\d|www\.|https?://|@|"
    r"all\s*rights\s*reserved|copyright|\u00a9)",
)

# Currency amount pattern (used to boost totals/line_items classification)
_CURRENCY_PATTERN = re.compile(
    r"[\$\u20ac\u00a3\u00a5]?\s*[\d,]+\.?\d*|"
    r"\d[\d,]*\.?\d*\s*(?:USD|EUR|GBP|JPY|CAD|AUD)",
)


def _classify_block(block: OcrBlock, page_height: float) -> str:
    """Classify a single OCR block into a section type.

    Uses a weighted scoring system combining:
      - Text content pattern matching
      - Vertical position on page (header at top, footer at bottom)
      - Currency amounts (boost totals/line_items)

    Returns the section type with the highest score.
    """
    text = block.text.strip()
    if not text:
        return "notes"

    scores: dict[str, float] = {s: 0.0 for s in SECTION_TYPES}

    # --- Content-based scoring ---
    if _HEADER_PATTERNS.search(text):
        scores["header"] += 3.0
    if _PARTY_PATTERNS.search(text):
        scores["parties"] += 3.0
    if _METADATA_PATTERNS.search(text):
        scores["metadata"] += 3.0
    if _LINE_ITEM_PATTERNS.search(text):
        scores["line_items"] += 2.5
    if _TOTALS_PATTERNS.search(text):
        scores["totals"] += 3.0
    if _NOTES_PATTERNS.search(text):
        scores["notes"] += 3.0
    if _TERMS_PATTERNS.search(text):
        scores["terms"] += 3.0
    if _FOOTER_PATTERNS.search(text):
        scores["footer"] += 3.0

    # Currency amounts boost financial sections
    currency_matches = len(_CURRENCY_PATTERN.findall(text))
    if currency_matches > 0:
        scores["totals"] += 1.0 * min(currency_matches, 3)
        scores["line_items"] += 0.5 * min(currency_matches, 3)

    # --- Position-based scoring ---
    if page_height > 0:
        relative_y = block.bbox.y / page_height
        # Top 15%: header/metadata zone
        if relative_y < 0.15:
            scores["header"] += 2.0
            scores["metadata"] += 1.0
        # 15-30%: parties/metadata zone
        elif relative_y < 0.30:
            scores["parties"] += 1.5
            scores["metadata"] += 1.0
        # 30-70%: line items zone
        elif relative_y < 0.70:
            scores["line_items"] += 1.5
        # 70-85%: totals zone
        elif relative_y < 0.85:
            scores["totals"] += 1.5
        # 85-100%: footer/terms zone
        else:
            scores["footer"] += 1.5
            scores["terms"] += 1.0

    # Pick the section with the highest score, defaulting to notes
    best_section = max(scores, key=lambda k: scores[k])
    if scores[best_section] < 0.5:
        # No strong signal: assign based on vertical position alone
        if page_height > 0:
            relative_y = block.bbox.y / page_height
            if relative_y < 0.15:
                return "header"
            elif relative_y < 0.30:
                return "parties"
            elif relative_y < 0.70:
                return "line_items"
            elif relative_y < 0.85:
                return "totals"
            else:
                return "footer"
        return "notes"

    return best_section


def classify_sections(
    blocks: list[OcrBlock],
    page_height: float = 0.0,
) -> dict[str, list[OcrBlock]]:
    """Classify all OCR blocks into sections.

    Returns a mapping of section_type -> list of blocks assigned to that
    section, sorted by vertical position within each section.
    """
    sections: dict[str, list[OcrBlock]] = {s: [] for s in SECTION_TYPES}
    for block in blocks:
        section = _classify_block(block, page_height)
        sections[section].append(block)

    # Sort blocks within each section by vertical position
    for section_blocks in sections.values():
        section_blocks.sort(key=lambda b: (b.bbox.y, b.bbox.x))

    return sections


# ---------------------------------------------------------------------------
# Style inference
# ---------------------------------------------------------------------------

@dataclass
class InferredStyles:
    """Styles inferred from OCR block geometry."""
    primary_color: str = "#111111"
    secondary_color: str = "#666666"
    accent_color: str = "#2563EB"
    heading_font_size_pt: float = 16.0
    body_font_size_pt: float = 10.0
    is_compact: bool = False
    logo_position: str = "left"
    header_alignment: str = "left"
    title_alignment: str = "left"


def _estimate_font_size_pt(bbox_height_px: float, page_height_px: float) -> float:
    """Estimate font size in points from bounding box height.

    Assumes standard 11" page height (792pt) when page_height_px is the
    full page.  A typical line of 10pt text occupies ~14px at 72 DPI
    (with line-height ~1.4).
    """
    if page_height_px <= 0:
        return 10.0
    # Map bbox height to points assuming 792pt page
    pt_height = (bbox_height_px / page_height_px) * 792.0
    # Font size is roughly 70% of line height
    font_pt = pt_height * 0.7
    # Clamp to reasonable range
    return max(6.0, min(48.0, font_pt))


def _infer_alignment(
    blocks: list[OcrBlock],
    page_width: float,
) -> str:
    """Infer predominant text alignment from block x-positions.

    Uses the distribution of block left edges and centers relative to
    page width to determine left / center / right alignment.
    """
    if not blocks or page_width <= 0:
        return "left"

    left_count = 0
    center_count = 0
    right_count = 0

    left_zone = page_width * 0.35
    right_zone = page_width * 0.65
    center_tolerance = page_width * 0.1

    for block in blocks:
        center = block.bbox.center_x
        if abs(center - page_width / 2) < center_tolerance:
            center_count += 1
        elif block.bbox.x < left_zone:
            left_count += 1
        elif block.bbox.x2 > right_zone:
            right_count += 1
        else:
            left_count += 1  # Default to left for ambiguous

    counts = {"left": left_count, "center": center_count, "right": right_count}
    return max(counts, key=lambda k: counts[k])


def infer_styles(
    blocks: list[OcrBlock],
    page_width: float = 0.0,
    page_height: float = 0.0,
) -> InferredStyles:
    """Infer template styles from OCR block geometry.

    Analyzes bounding box dimensions and positions to estimate:
      - Font sizes (heading vs body)
      - Alignment patterns
      - Spacing density (compact vs normal)
      - Logo position
    """
    styles = InferredStyles()

    if not blocks:
        return styles

    # Collect all bbox heights for font size estimation
    heights = [b.bbox.height for b in blocks if b.bbox.height > 0]
    if not heights:
        return styles

    # Heading: largest blocks (top 20th percentile)
    # Body: median block height
    sorted_heights = sorted(heights)
    median_idx = len(sorted_heights) // 2
    p80_idx = int(len(sorted_heights) * 0.8)

    body_height = sorted_heights[median_idx]
    heading_height = sorted_heights[min(p80_idx, len(sorted_heights) - 1)]

    styles.body_font_size_pt = round(
        _estimate_font_size_pt(body_height, page_height), 1,
    )
    styles.heading_font_size_pt = round(
        _estimate_font_size_pt(heading_height, page_height), 1,
    )

    # Ensure heading is meaningfully larger than body
    if styles.heading_font_size_pt <= styles.body_font_size_pt * 1.2:
        styles.heading_font_size_pt = round(styles.body_font_size_pt * 1.6, 1)

    # Spacing: compact if average vertical gap between consecutive blocks
    # is less than body font size
    if len(blocks) >= 2:
        sorted_by_y = sorted(blocks, key=lambda b: b.bbox.y)
        gaps = []
        for i in range(len(sorted_by_y) - 1):
            gap = sorted_by_y[i + 1].bbox.y - sorted_by_y[i].bbox.y2
            if gap > 0:
                gaps.append(gap)
        if gaps:
            avg_gap = sum(gaps) / len(gaps)
            # Compact if average gap is less than 1.5x the median block height
            styles.is_compact = avg_gap < body_height * 1.5

    # Header alignment: analyze blocks in top 15% of page
    if page_height > 0:
        header_blocks = [
            b for b in blocks if b.bbox.y < page_height * 0.15
        ]
        styles.header_alignment = _infer_alignment(header_blocks, page_width)
        styles.title_alignment = styles.header_alignment

    # Logo position: the largest block in the top 10% of the page
    # that's wider than it is tall is likely a logo
    if page_height > 0 and page_width > 0:
        top_blocks = [
            b for b in blocks
            if b.bbox.y < page_height * 0.10 and b.bbox.width > b.bbox.height
        ]
        if top_blocks:
            widest = max(top_blocks, key=lambda b: b.bbox.width)
            rel_x = widest.bbox.center_x / page_width
            if rel_x < 0.35:
                styles.logo_position = "left"
            elif rel_x > 0.65:
                styles.logo_position = "right"
            else:
                styles.logo_position = "center"

    return styles


# ---------------------------------------------------------------------------
# Layout inference
# ---------------------------------------------------------------------------

@dataclass
class InferredLayout:
    """Layout structure inferred from OCR block positions."""
    section_order: list[str] = field(default_factory=list)
    parties_layout: str = "two-column"  # "two-column" | "stacked"
    totals_position: str = "right"  # "left" | "right" | "full-width"
    page_orientation: str = "portrait"  # "portrait" | "landscape"


def infer_layout(
    sections: dict[str, list[OcrBlock]],
    page_width: float = 0.0,
    page_height: float = 0.0,
) -> InferredLayout:
    """Infer layout structure from classified sections.

    Determines:
      - Section order (by average vertical position of blocks)
      - Parties layout (two-column if blocks span both halves)
      - Totals position (right-aligned, left-aligned, or full-width)
      - Page orientation
    """
    layout = InferredLayout()

    # Section order: sort by the minimum y-position of blocks in each section
    section_positions: list[tuple[str, float]] = []
    for section_type in SECTION_TYPES:
        blocks = sections.get(section_type, [])
        if blocks:
            min_y = min(b.bbox.y for b in blocks)
            section_positions.append((section_type, min_y))

    section_positions.sort(key=lambda x: x[1])
    layout.section_order = [s[0] for s in section_positions]

    # Fill in missing sections in default order
    for section_type in SECTION_TYPES:
        if section_type not in layout.section_order:
            layout.section_order.append(section_type)

    # Parties layout: two-column if blocks span both left and right halves
    if page_width > 0:
        party_blocks = sections.get("parties", [])
        if party_blocks:
            left_blocks = [
                b for b in party_blocks if b.bbox.center_x < page_width * 0.5
            ]
            right_blocks = [
                b for b in party_blocks if b.bbox.center_x >= page_width * 0.5
            ]
            if left_blocks and right_blocks:
                layout.parties_layout = "two-column"
            else:
                layout.parties_layout = "stacked"

    # Totals position: check x-alignment of totals blocks
    if page_width > 0:
        totals_blocks = sections.get("totals", [])
        if totals_blocks:
            avg_x = sum(b.bbox.center_x for b in totals_blocks) / len(totals_blocks)
            if avg_x > page_width * 0.6:
                layout.totals_position = "right"
            elif avg_x < page_width * 0.4:
                layout.totals_position = "left"
            else:
                layout.totals_position = "full-width"

    # Page orientation
    if page_width > 0 and page_height > 0:
        layout.page_orientation = "landscape" if page_width > page_height else "portrait"

    return layout


# ---------------------------------------------------------------------------
# Template generation
# ---------------------------------------------------------------------------

@dataclass
class TemplateDefinition:
    """Template definition compatible with enjoice's TemplateDefinition type.

    This is a Python mirror of the TypeScript interface at
    @plugin/templates/types.ts. The JSON serialization of this dataclass
    can be deserialized directly by the enjoice template editor.
    """
    id: str
    name: str
    description: str
    type: str  # "invoice" | "credit_note" | "quote" | "purchase_order"
    layout: dict[str, Any]
    styles: dict[str, Any]
    element_styles: dict[str, Any] = field(default_factory=dict)
    hidden_elements: list[str] = field(default_factory=list)
    custom_fields: list[dict[str, Any]] = field(default_factory=list)
    custom_sections: list[dict[str, Any]] = field(default_factory=list)
    confidence: float = 0.0
    detected_sections: list[str] = field(default_factory=list)

    def to_json(self) -> dict[str, Any]:
        """Serialize to JSON matching enjoice's TemplateDefinition schema.

        Uses camelCase keys to match the TypeScript interface.
        """
        return {
            "id": self.id,
            "name": self.name,
            "description": self.description,
            "type": self.type,
            "layout": self.layout,
            "styles": self.styles,
            "elementStyles": self.element_styles,
            "hiddenElements": self.hidden_elements,
            "customFields": self.custom_fields,
            "customSections": self.custom_sections,
            "confidence": self.confidence,
            "detected_sections": self.detected_sections,
        }


def _build_layout(
    inferred_layout: InferredLayout,
    page_width: float,
    page_height: float,
) -> dict[str, Any]:
    """Build the layout dict matching enjoice's TemplateLayout."""
    sections = []
    for i, section_type in enumerate(inferred_layout.section_order):
        sections.append({
            "type": section_type,
            "visible": True,
            "order": i,
        })

    # Determine page size from dimensions
    # Standard sizes in points: Letter = 612x792, A4 = 595x842
    page_size = "letter"
    if page_width > 0 and page_height > 0:
        aspect = page_width / page_height if page_height > 0 else 1.0
        if inferred_layout.page_orientation == "portrait":
            # A4 is narrower than Letter
            if 0.68 < aspect < 0.72:
                page_size = "a4"
            elif 0.76 < aspect < 0.80:
                page_size = "letter"
        else:
            if 1.30 < aspect < 1.45:
                page_size = "a4"
            elif 1.25 < aspect < 1.35:
                page_size = "letter"

    return {
        "id": "",
        "name": "Extracted Layout",
        "description": "Layout extracted from scanned invoice",
        "sections": sections,
        "page": {
            "size": page_size,
            "orientation": inferred_layout.page_orientation,
            "margins": {"top": 28, "right": 36, "bottom": 28, "left": 36},
        },
    }


def _build_styles(inferred_styles: InferredStyles) -> dict[str, Any]:
    """Build the styles dict matching enjoice's TemplateStyles."""
    return {
        "colors": {
            "primary": inferred_styles.primary_color,
            "secondary": inferred_styles.secondary_color,
            "accent": inferred_styles.accent_color,
            "text": "#111111",
            "background": "#FFFFFF",
            "border": "#E5E5E5",
            "headerBg": "#F9F9F9",
            "altRowBg": "#FAFAFA",
        },
        "fonts": {
            "heading": "Inter",
            "body": "Inter",
            "mono": "JetBrains Mono",
        },
        "spacing": {"compact": inferred_styles.is_compact},
        "logo": {
            "position": inferred_styles.logo_position,
            "maxWidth": 200,
            "maxHeight": 80,
        },
        "typography": {
            "headingSize": _pt_to_size_token(inferred_styles.heading_font_size_pt),
            "bodySize": round(inferred_styles.body_font_size_pt),
        },
        "layoutHints": {
            "headerAlign": inferred_styles.header_alignment,
            "titleAlign": inferred_styles.title_alignment,
            "totalsPosition": "right",
            "partiesLayout": "two-column",
        },
    }


def _pt_to_size_token(pt: float) -> str:
    """Map a point size to enjoice's headingSize token."""
    if pt <= 10:
        return "xs"
    elif pt <= 13:
        return "sm"
    elif pt <= 16:
        return "md"
    elif pt <= 20:
        return "lg"
    elif pt <= 26:
        return "xl"
    else:
        return "2xl"


def _compute_confidence(
    sections: dict[str, list[OcrBlock]],
    blocks: list[OcrBlock],
) -> float:
    """Compute overall confidence score for the template extraction.

    Based on:
      - Number of sections detected (more = higher)
      - Block count (very few blocks = low confidence)
      - Average per-block OCR confidence
    """
    if not blocks:
        return 0.0

    score = 0.0

    # Section diversity: more detected sections = better
    non_empty_sections = sum(1 for v in sections.values() if v)
    score += min(non_empty_sections / 6.0, 1.0) * 0.4

    # Block count: need at least 5 blocks for a reasonable invoice
    block_score = min(len(blocks) / 10.0, 1.0)
    score += block_score * 0.3

    # Average OCR confidence
    avg_conf = sum(b.confidence for b in blocks) / len(blocks)
    score += avg_conf * 0.3

    return round(min(1.0, score), 3)


def extract_template_from_ocr(ocr_result: OcrResult) -> TemplateDefinition:
    """Analyze scanned invoice layout and generate a template.

    Preserves: section order, alignment, spacing ratios, field placement.
    Does NOT include: actual data (line items, amounts, names).

    Args:
        ocr_result: OCR output with text blocks and bounding boxes.

    Returns:
        A TemplateDefinition compatible with enjoice's template editor.
    """
    # Step 1: classify blocks into sections
    sections = classify_sections(
        ocr_result.blocks,
        page_height=ocr_result.page_height,
    )

    # Step 2: infer styles from block geometry
    styles = infer_styles(
        ocr_result.blocks,
        page_width=ocr_result.page_width,
        page_height=ocr_result.page_height,
    )

    # Step 3: infer layout from section positions
    layout = infer_layout(
        sections,
        page_width=ocr_result.page_width,
        page_height=ocr_result.page_height,
    )

    # Step 4: update layout hints from inferred layout
    template_styles = _build_styles(styles)
    template_styles["layoutHints"]["totalsPosition"] = layout.totals_position
    template_styles["layoutHints"]["partiesLayout"] = layout.parties_layout

    # Step 5: build the template definition
    detected = [s for s in SECTION_TYPES if sections.get(s)]
    confidence = _compute_confidence(sections, ocr_result.blocks)

    template_id = str(uuid.uuid4()) if hasattr(uuid, "uuid4") else f"tpl_{id(ocr_result)}"

    return TemplateDefinition(
        id=template_id,
        name="Extracted Template",
        description="Template extracted from scanned invoice via Falcon-OCR",
        type="invoice",
        layout=_build_layout(layout, ocr_result.page_width, ocr_result.page_height),
        styles=template_styles,
        confidence=confidence,
        detected_sections=detected,
    )
