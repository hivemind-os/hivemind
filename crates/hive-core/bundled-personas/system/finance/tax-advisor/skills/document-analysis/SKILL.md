---
name: document-analysis
description: Parse, extract, and interpret tax and financial documents including PDFs, spreadsheets, and CSV transaction records.
---

# Document Analysis Skill

When asked to analyze tax or financial documents, follow these guidelines.

## Reading Documents

Use the `filesystem.read_document` tool to extract text from supported formats:
- **PDF**: Tax returns, W-2s, 1099s, K-1s, brokerage statements, bank statements
- **XLSX / Numbers**: Spreadsheets with transaction data, financial summaries, accounting exports
- **CSV**: Brokerage transaction exports, bank transaction downloads, payroll data
- **DOCX / Pages**: Tax letters, IRS notices, engagement letters

For large documents, read the entire file first, then focus analysis on the sections relevant to the user's question.

## Common Tax Document Structures

### W-2 (Wage and Tax Statement)
Key fields to extract and verify:
- **Box 1**: Wages, tips, other compensation (federal taxable income)
- **Box 2**: Federal income tax withheld
- **Box 3/5**: Social Security / Medicare wages (may differ from Box 1 due to pre-tax deductions)
- **Box 4/6**: Social Security / Medicare tax withheld
- **Box 12**: Coded items — watch for 401(k) contributions (D/DD), HSA (W), dependent care (10)
- **Box 13**: Statutory employee, retirement plan, third-party sick pay checkboxes
- Cross-check: Box 1 + pre-tax deductions should approximately equal gross pay

### 1099 Variants
- **1099-INT**: Interest income. Box 1 = taxable interest, Box 3 = savings bond interest, Box 4 = federal tax withheld
- **1099-DIV**: Dividends. Box 1a = ordinary dividends, Box 1b = qualified dividends (lower rate), Box 2a = capital gain distributions
- **1099-B**: Brokerage proceeds. Watch for cost basis reported (Box 1e) vs. not reported. Check wash sale adjustments in Box 1g
- **1099-MISC / 1099-NEC**: Independent contractor income. NEC Box 1 = nonemployee compensation (subject to self-employment tax)
- **1099-R**: Retirement distributions. Box 2a = taxable amount, Box 7 = distribution code (early withdrawal penalties, rollovers)
- **1099-K**: Payment card / third-party network transactions. Gross amounts — does NOT equal taxable income

### Schedule K-1 (Form 1065 / 1120-S)
- **Part III**: Partner's/shareholder's share of income, deductions, credits
- Line 1: Ordinary business income
- Lines 2–4c: Rental income, other income, guaranteed payments
- Line 11: Other deductions (may need statement)
- Line 13: Credits
- Line 14: Self-employment earnings (partnerships only)
- Watch for: basis limitations, at-risk rules, passive activity rules

### Tax Returns (Form 1040)
- **Page 1**: Filing status, dependents, income summary
- **Page 2**: Tax computation, credits, other taxes, payments
- **Schedules**: 1 (additional income/adjustments), 2 (additional taxes), 3 (additional credits), A (itemized deductions), B (interest/dividends), C (business income), D (capital gains), E (supplemental income), SE (self-employment tax)

## Data Extraction Tips

1. **Tabular data**: When reading spreadsheets or CSV files, identify the column headers first. Map columns to their financial meaning before analyzing.
2. **Multi-page PDFs**: Look for page numbers and section headers. Tax returns follow a consistent page order.
3. **Scanned documents**: If `filesystem.read_document` returns no text or reports "scanned/image-only", activate the **pdf-ocr** skill to render pages as images for vision analysis. For PDFs that do return text but with OCR artifacts, watch for common misreads: `1` vs `l`, `0` vs `O`, `$` dropped from amounts.
4. **Cross-referencing**: Always cross-reference figures across documents. For example:
   - W-2 Box 1 should appear on 1040 Line 1
   - 1099-INT Box 1 should appear on Schedule B
   - K-1 income flows to Schedule E
5. **Rounding**: Tax forms often round to whole dollars. Small discrepancies (< $1) between documents are normal.

## Verification Checklist

When reviewing a complete tax return:
- [ ] All income sources accounted for (compare W-2s, 1099s, K-1s to return)
- [ ] Filing status is optimal (married filing jointly vs. separately)
- [ ] Standard vs. itemized deduction — confirm the better choice was taken
- [ ] All eligible credits claimed (child tax credit, earned income credit, education credits)
- [ ] Estimated tax payments reconciled with payment records
- [ ] Prior-year overpayment applied correctly
- [ ] State return is consistent with federal return
