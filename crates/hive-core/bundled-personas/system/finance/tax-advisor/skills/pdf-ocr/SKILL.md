---
name: pdf-ocr
description: Process scanned or image-based PDFs by rendering pages to images for vision analysis or extracting embedded text. Use when filesystem.read_document returns no text from a PDF.
---

# PDF OCR Skill

Use this skill when `filesystem.read_document` fails to extract text from a PDF — typically because the PDF contains scanned images rather than embedded text. This is common with:
- Scanned W-2s, 1099s, and other IRS forms
- Brokerage statements saved as image scans
- Bank statements from older institutions
- Faxed or photocopied tax documents

## Bundled Script

This skill includes `scripts/pdf_to_images.py` which handles PDF processing.

## Workflow

### Step 1: Install Dependencies

This skill requires Python 3 and the `pymupdf` package. Before running any scripts, install the required packages using the **same Python interpreter** that will run the script:

```
python -m pip install -r "<skill_dir>/requirements.txt"
```

If the install command fails (e.g., pip is not available, network is restricted, or the environment is sandboxed), **stop and report to the user** that this environment cannot run PDF OCR/rendering and suggest they install pymupdf manually. Do not proceed to later steps.

### Step 2: Detect Scanned PDFs

When `filesystem.read_document` returns an error like "no text could be extracted" or "scanned/image-only", the PDF is likely image-based.

### Step 3: Try Native Text Extraction First

Use the bundled script's `extract` mode to check whether the PDF has an embedded text layer. This is **not OCR** — it reads text that is already encoded in the PDF, which succeeds for digitally-created PDFs but fails for pure scans:

```
python <skill_dir>/scripts/pdf_to_images.py extract "<pdf_path>"
```

Check the `has_text` field in the JSON output. If `true`, use the extracted text directly.

### Step 4: Render Pages as Images

If text extraction yields no results, render the PDF pages as images for vision analysis:

```
python <skill_dir>/scripts/pdf_to_images.py render "<pdf_path>" "<output_dir>" --dpi 300
```

Where `<output_dir>` is a temporary directory for the images. For large PDFs, process specific pages:

```
python <skill_dir>/scripts/pdf_to_images.py render "<pdf_path>" "<output_dir>" --dpi 300 --pages 1-5
```

The output is a JSON object listing the generated image files:
```json
{
  "images": [
    {"page": 1, "path": "/tmp/output/page_0001.png", "width": 2550, "height": 3300},
    {"page": 2, "path": "/tmp/output/page_0002.png", "width": 2550, "height": 3300}
  ],
  "total_pages": 10,
  "rendered": 2
}
```

### Step 5: Analyze with Vision

Read each rendered image using `filesystem.read_file` with the image paths from the JSON output. The images will be sent to the model as vision inputs, allowing you to read and interpret the scanned document content.

For tax documents, focus on extracting:
- All dollar amounts and their field labels
- Names, addresses, TINs/SSNs (note: report only last 4 digits for privacy)
- Form numbers and tax year
- Box numbers and their values

### Step 6: Clean Up

After analysis, delete the temporary image files to save disk space.

## Tips

- **DPI setting**: 300 DPI is the default and works well for most documents. Use 200 DPI for faster processing of large documents where fine detail isn't critical. Use 400+ DPI only if text is very small or blurry.
- **Page selection**: For multi-page PDFs, process only the pages you need rather than the entire document. Tax forms typically have key data on specific pages.
- **Multi-step analysis**: For complex documents, render and analyze a few pages at a time rather than all at once, to keep context focused.
- **Cross-reference**: After extracting data from a scanned document, cross-reference the values against other documents (e.g., W-2 Box 1 should match 1040 Line 1).
