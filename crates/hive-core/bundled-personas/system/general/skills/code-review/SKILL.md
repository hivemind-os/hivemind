---
name: code-review
description: Perform thorough code reviews with focus on correctness, security, performance, and maintainability.
---

# Code Review Skill

When asked to review code, follow this structured approach:

## Review Checklist

1. **Correctness** — Does the code do what it claims? Are there logic errors, off-by-one bugs, or unhandled edge cases?
2. **Security** — Are there injection vulnerabilities, improper input validation, exposed secrets, or unsafe operations?
3. **Performance** — Are there unnecessary allocations, O(n²) where O(n) is possible, missing caching opportunities, or blocking calls in async contexts?
4. **Error Handling** — Are errors propagated correctly? Are failure modes recoverable? Are error messages helpful?
5. **Maintainability** — Is the code readable? Are names descriptive? Is complexity justified?

## Output Format

Structure your review as:

- **Summary**: One-paragraph overall assessment
- **Critical Issues**: Bugs or security problems that must be fixed
- **Suggestions**: Improvements that would meaningfully help
- Skip style-only nitpicks unless they significantly hurt readability.

## Guidelines

- Be specific: reference exact lines and explain *why* something is a problem
- Suggest concrete fixes, not just "this could be improved"
- Acknowledge good patterns when you see them
- Prioritize: fix bugs first, then security, then performance, then style
