---
name: cost-basis
description: Calculate cost basis and capital gains/losses using FIFO, LIFO, Specific Identification, or Average Cost methods, with awareness of wash sales, corporate actions, and cryptocurrency specifics.
---

# Cost Basis Calculation Skill

When asked to calculate cost basis or capital gains/losses, follow these guidelines.

## Cost Basis Methods

### FIFO (First In, First Out)
- Shares acquired **earliest** are sold first
- Default method for most brokerages and the IRS if no method is specified
- In a rising market, FIFO typically produces **higher gains** (selling cheapest shares first)
- Simple to track and widely accepted

### LIFO (Last In, First Out)
- Shares acquired **most recently** are sold first
- In a rising market, LIFO typically produces **lower gains** (selling most expensive shares first)
- Less common for securities; more common in inventory accounting
- Must be elected and consistently applied

### Specific Identification
- Seller **chooses** which specific lots to sell
- Most flexible — allows tax-loss harvesting and gain management
- Requires identifying the specific shares at the time of sale
- Must maintain adequate records linking each sale to specific purchase lots
- Broker confirmation required for adequate identification

### Average Cost
- Cost basis = total cost of all shares ÷ number of shares
- Only permitted for **mutual fund shares** and certain dividend reinvestment plans under US rules
- Some jurisdictions (e.g., UK "Section 104 pool") use average cost for all shares
- Once elected for a fund, applies to all shares in that fund

## Calculation Process

### Step 1: Build the Lot Inventory
For each acquisition, record:
- **Date acquired**
- **Quantity**
- **Price per unit**
- **Total cost** (including commissions and fees)
- **Acquisition type** (purchase, gift, inheritance, exercise, dividend reinvestment)

### Step 2: Process Dispositions Chronologically
For each sale/disposition:
1. Identify which lots to match (based on chosen method)
2. Calculate proceeds: sale price × quantity - commissions/fees
3. Calculate basis: cost of matched lots (including any adjustments)
4. Determine gain/loss: proceeds - adjusted basis
5. Classify as **short-term** (held ≤ 1 year) or **long-term** (held > 1 year)
6. Check for wash sale violations (see below)

### Step 3: Produce Summary
Present results as a structured table:
| Date Sold | Asset | Qty | Proceeds | Basis | Gain/Loss | Term | Wash Sale Adj |
|-----------|-------|-----|----------|-------|-----------|------|---------------|

Then provide totals:
- Total short-term gain/loss
- Total long-term gain/loss
- Net overall gain/loss

## Wash Sale Rules

A **wash sale** occurs when you sell a security at a loss and buy a "substantially identical" security within **30 days before or after** the sale (61-day window).

### Effects
- The loss is **disallowed** for the current period
- The disallowed loss is **added to the basis** of the replacement shares
- The **holding period** of the replacement shares includes the period of the original shares

### What Triggers a Wash Sale
- Buying the same stock or security
- Buying a call option on the same stock
- Buying a substantially identical mutual fund
- Acquiring shares through dividend reinvestment
- Purchases in an IRA or other related accounts (under US rules)

### What Does NOT Trigger a Wash Sale
- Selling stock and buying bonds of the same company
- Selling one S&P 500 index fund and buying a different provider's S&P 500 fund (debatable — exercise caution)
- Selling at a **gain** (wash sales only apply to losses)

### Calculation Adjustment
When a wash sale occurs:
1. Disallow the loss on the original sale
2. Add the disallowed loss to the basis of the replacement shares
3. Adjust the holding period of the replacement shares

## Corporate Actions

### Stock Splits
- Forward split (e.g., 2:1): quantity doubles, basis per share halves, total basis unchanged
- Reverse split (e.g., 1:5): quantity reduced, basis per share increases proportionally
- Fractional shares from splits: treat as a small sale at the split-adjusted basis

### Stock Dividends
- Non-taxable stock dividends: allocate original basis across old + new shares
- Taxable stock dividends: new shares have basis equal to their fair market value on distribution date

### Mergers and Acquisitions
- **Tax-free reorganization**: basis carries over from old shares to new shares (adjusted for any boot received)
- **Taxable acquisition**: treat as a sale of old shares and purchase of new shares
- **Cash + stock deals**: allocate basis proportionally; cash portion may trigger gain recognition

### Spin-offs
- Allocate basis between parent and spin-off shares based on relative fair market values on the distribution date
- IRS or the company often publishes the allocation percentages

## Cryptocurrency Specifics

### General Rules
- Crypto is treated as **property** (not currency) in most jurisdictions
- Every disposal (sale, trade, spend) is a taxable event
- Trading one crypto for another is a taxable event (unlike a like-kind exchange for real estate)

### Special Situations
- **Hard forks**: New coins received have basis of $0 (or fair market value at time of receipt if reported as income)
- **Airdrops**: Generally taxable as ordinary income at fair market value when received; that FMV becomes the cost basis
- **Staking rewards**: Taxable as ordinary income when received; FMV at receipt = basis
- **Mining**: Taxable as ordinary income (possibly self-employment income); FMV at receipt = basis
- **DeFi yields / liquidity pools**: Complex — may be ordinary income, may involve multiple taxable events
- **NFTs**: Treated as property; may be classified as collectibles (higher long-term rate in some jurisdictions)
- **Lost/stolen crypto**: Theft losses have limited deductibility (check current rules)

### Tracking Challenges
- Use transaction-level matching, not wallet-level
- Account for network fees (gas) as part of cost basis or as a separate deductible expense
- Reconcile across wallets and exchanges
- Watch for internal transfers that are NOT taxable events

## Output Format

Always present cost-basis calculations with:
1. **Assumptions stated**: Method used, any judgment calls made
2. **Lot-level detail**: Show which lots were matched to which sales
3. **Summary table**: Gains/losses by short-term and long-term
4. **Tax impact estimate**: If rates are known, estimate the tax owed
5. **Caveats**: Note any areas of uncertainty or where professional review is recommended
