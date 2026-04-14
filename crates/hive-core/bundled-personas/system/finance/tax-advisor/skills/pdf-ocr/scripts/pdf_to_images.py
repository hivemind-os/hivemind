#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.8"
# dependencies = ["pymupdf"]
# ///
"""Render PDF pages to images or extract embedded text.

This script is bundled with the pdf-ocr skill for the Tax Advisor persona.
It uses PyMuPDF (pymupdf) to handle image-based/scanned PDFs that cannot
be read by text-only extractors.

Usage:
    # Preferred: run via uv (handles pymupdf dependency automatically)
    uv run pdf_to_images.py render <pdf_path> <output_dir> [--dpi 300] [--pages 1-3]

    # Fallback: standard python (pymupdf must already be installed)
    python3 pdf_to_images.py render <pdf_path> <output_dir> [--dpi 300] [--pages 1-3]

    # Extract embedded text from PDF pages (not OCR — reads the text layer)
    python pdf_to_images.py extract <pdf_path> [--pages 1-3]

Requirements:
    pip install pymupdf

Output (render mode):
    JSON object with "images" array of {page, path, width, height}

Output (extract mode):
    JSON object with "pages" array of {page, text}
"""

import argparse
import json
import os
import sys


def ensure_pymupdf():
    """Import pymupdf, installing it as a fallback if necessary."""
    # Try the modern import name first, then the legacy alias.
    try:
        import pymupdf  # noqa: F811
        return pymupdf
    except ImportError:
        pass
    try:
        import fitz  # noqa: F811
        return fitz
    except ImportError:
        pass

    # Auto-install as a last resort.
    print("pymupdf not found. Attempting install...", file=sys.stderr)
    import subprocess
    try:
        subprocess.check_call(
            [sys.executable, "-m", "pip", "install", "pymupdf"],
            stdout=sys.stderr,
            stderr=sys.stderr,
        )
    except (subprocess.CalledProcessError, FileNotFoundError) as exc:
        print(json.dumps({
            "error": (
                "pymupdf is not installed and automatic installation failed. "
                "Please install it manually with:  "
                "python -m pip install pymupdf"
            ),
            "details": str(exc),
        }), file=sys.stdout)
        sys.exit(1)

    # Verify the install actually worked.
    try:
        import pymupdf  # noqa: F811
        return pymupdf
    except ImportError:
        pass
    try:
        import fitz  # noqa: F811
        return fitz
    except ImportError:
        print(json.dumps({
            "error": (
                "pymupdf was installed but still cannot be imported. "
                "The package may have been installed into a different Python "
                "environment. Please run:  "
                f"{sys.executable} -m pip install pymupdf"
            ),
        }), file=sys.stdout)
        sys.exit(1)


def parse_page_range(page_spec, total_pages):
    """Parse a page range spec like '1-3' or '1,3,5' into 0-based indices."""
    if not page_spec:
        return list(range(total_pages))

    pages = set()
    for part in page_spec.split(","):
        part = part.strip()
        if "-" in part:
            start, end = part.split("-", 1)
            start = max(1, int(start))
            end = min(total_pages, int(end))
            pages.update(range(start - 1, end))
        else:
            p = int(part)
            if 1 <= p <= total_pages:
                pages.add(p - 1)
    return sorted(pages)


def render_pages(pdf_path, output_dir, dpi=300, page_spec=None):
    """Render PDF pages as PNG images."""
    fitz = ensure_pymupdf()

    os.makedirs(output_dir, exist_ok=True)
    doc = fitz.open(pdf_path)
    pages = parse_page_range(page_spec, len(doc))
    zoom = dpi / 72.0
    matrix = fitz.Matrix(zoom, zoom)

    total_pages = len(doc)
    results = []
    for page_num in pages:
        page = doc[page_num]
        pix = page.get_pixmap(matrix=matrix)

        filename = f"page_{page_num + 1:04d}.png"
        out_path = os.path.join(output_dir, filename)
        pix.save(out_path)

        results.append({
            "page": page_num + 1,
            "path": out_path,
            "width": pix.width,
            "height": pix.height,
        })

    doc.close()
    return {"images": results, "total_pages": total_pages, "rendered": len(results)}


def extract_text(pdf_path, page_spec=None):
    """Extract embedded text from PDF pages.

    This reads the text layer already encoded in the PDF — it is NOT OCR.
    It works for digitally-created PDFs but will return empty text for
    pure scanned images. Use the render mode + vision model for scans.
    """
    fitz = ensure_pymupdf()

    doc = fitz.open(pdf_path)
    pages = parse_page_range(page_spec, len(doc))

    total_pages = len(doc)
    results = []
    has_text = False
    for page_num in pages:
        page = doc[page_num]
        text = page.get_text("text").strip()
        if text:
            has_text = True
        results.append({"page": page_num + 1, "text": text})

    doc.close()
    return {
        "pages": results,
        "total_pages": total_pages,
        "extracted": len(results),
        "has_text": has_text,
    }


def main():
    parser = argparse.ArgumentParser(
        description="Render PDF pages to images or extract text."
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    # render subcommand
    render_parser = subparsers.add_parser(
        "render", help="Render PDF pages as PNG images"
    )
    render_parser.add_argument("pdf_path", help="Path to the PDF file")
    render_parser.add_argument("output_dir", help="Directory to save images")
    render_parser.add_argument(
        "--dpi", type=int, default=300, help="Resolution in DPI (default: 300)"
    )
    render_parser.add_argument(
        "--pages", help="Page range (e.g. '1-3' or '1,3,5')"
    )

    # extract subcommand
    extract_parser = subparsers.add_parser(
        "extract", help="Extract text from PDF pages"
    )
    extract_parser.add_argument("pdf_path", help="Path to the PDF file")
    extract_parser.add_argument(
        "--pages", help="Page range (e.g. '1-3' or '1,3,5')"
    )

    args = parser.parse_args()

    if not os.path.isfile(args.pdf_path):
        print(
            json.dumps({"error": f"File not found: {args.pdf_path}"}),
            file=sys.stdout,
        )
        sys.exit(1)

    try:
        if args.command == "render":
            result = render_pages(
                args.pdf_path, args.output_dir, dpi=args.dpi, page_spec=args.pages
            )
        elif args.command == "extract":
            result = extract_text(args.pdf_path, page_spec=args.pages)
        else:
            parser.print_help()
            sys.exit(1)

        print(json.dumps(result, indent=2))
    except Exception as e:
        print(json.dumps({"error": str(e)}), file=sys.stdout)
        sys.exit(1)


if __name__ == "__main__":
    main()
