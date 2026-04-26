//! Extract executable Python code blocks from LLM responses.
//!
//! The CodeAct strategy needs to parse fenced code blocks from the model's
//! text output. This module provides extraction for Python-only code blocks,
//! distinguishing executable blocks from display/example blocks.

use serde::{Deserialize, Serialize};

/// A single extracted code block from an LLM response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeBlock {
    /// The source code to execute.
    pub code: String,
    /// The language tag from the fenced block (e.g. "python", "py").
    pub language_tag: String,
    /// Byte offset of the opening fence in the original text.
    pub start_offset: usize,
    /// Byte offset past the closing fence in the original text.
    pub end_offset: usize,
}

/// Extract all executable Python code blocks from an LLM response.
///
/// Recognises fenced blocks with language tags: `python`, `py`, `python3`.
/// Blocks tagged with other languages or without a language tag are ignored.
///
/// Code blocks whose language tag ends with `:noexec` (e.g. ` ```python:noexec`)
/// are skipped — this convention lets the LLM show code examples without
/// triggering execution.
///
/// **Streaming invariant:** This function expects `content` to be the *complete*
/// assistant response (i.e. after streaming has finished). Calling it on a
/// partial/still-streaming message may produce incorrect results because an
/// incomplete fenced block (no closing ```) is treated as unclosed and ignored.
/// The CodeAct loop satisfies this by only calling `extract_python_blocks` on
/// the fully-assembled response text.
pub fn extract_python_blocks(content: &str) -> Vec<CodeBlock> {
    let mut blocks = Vec::new();
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut pos = 0;

    while pos < len {
        // Find opening fence: ``` at start of line (or start of string)
        let (fence_start, fence_len) = match find_fence_open(content, pos) {
            Some(result) => result,
            None => break,
        };

        // Extract the language tag (everything after the backticks on the same line)
        let tag_start = fence_start + fence_len;
        let line_end = content[tag_start..]
            .find('\n')
            .map(|i| tag_start + i)
            .unwrap_or(len);
        let language_tag = content[tag_start..line_end].trim().to_lowercase();

        // Find closing fence: matching or longer backtick run on its own line
        let code_start = if line_end < len { line_end + 1 } else { len };
        let (code_end, fence_close_end) = match find_fence_close(content, code_start, fence_len) {
            Some((ce, fce)) => (ce, fce),
            None => {
                // Unclosed fence — skip to end
                pos = len;
                continue;
            }
        };

        pos = fence_close_end;

        // Skip non-Python blocks
        if !is_python_tag(&language_tag) {
            continue;
        }

        // Skip :noexec annotated blocks
        if language_tag.contains(":noexec") {
            continue;
        }

        let code = content[code_start..code_end].to_string();

        // Skip empty or whitespace-only blocks
        if code.trim().is_empty() {
            continue;
        }

        blocks.push(CodeBlock {
            code,
            language_tag,
            start_offset: fence_start,
            end_offset: fence_close_end,
        });
    }

    blocks
}

/// Check whether a language tag indicates Python.
fn is_python_tag(tag: &str) -> bool {
    let base = tag.split(':').next().unwrap_or(tag).trim();
    matches!(base, "python" | "py" | "python3")
}

/// Find the start of a ``` fence at or after `from`.
/// The fence must be at the beginning of a line (or at position 0).
/// Returns `(offset, fence_len)` where fence_len is the number of backticks.
fn find_fence_open(content: &str, from: usize) -> Option<(usize, usize)> {
    let mut pos = from;
    let bytes = content.as_bytes();
    let len = bytes.len();

    while pos + 2 < len {
        // Check for ``` 
        if bytes[pos] == b'`' && bytes[pos + 1] == b'`' && bytes[pos + 2] == b'`' {
            // Verify it's at line start
            if pos == 0 || bytes[pos - 1] == b'\n' {
                // Count total backticks in this fence
                let mut fence_len = 3;
                while pos + fence_len < len && bytes[pos + fence_len] == b'`' {
                    fence_len += 1;
                }
                return Some((pos, fence_len));
            }
        }
        // Advance to next line
        match content[pos..].find('\n') {
            Some(nl) => pos += nl + 1,
            None => break,
        }
    }
    None
}

/// Find the closing fence starting from `from` with at least `fence_len` backticks.
/// Returns (code_end, fence_close_end) — code_end is the byte before the
/// closing fence line, fence_close_end is past the closing fence and its newline.
fn find_fence_close(content: &str, from: usize, fence_len: usize) -> Option<(usize, usize)> {
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut pos = from;

    while pos < len {
        let line_start = pos;
        let line_end = content[pos..]
            .find('\n')
            .map(|i| pos + i)
            .unwrap_or(len);
        let line = content[line_start..line_end].trim();

        // Closing fence must be at least fence_len backticks and nothing else
        if line.len() >= fence_len && line.chars().all(|c| c == '`') {
            let fence_close_end = if line_end < len { line_end + 1 } else { len };
            return Some((line_start, fence_close_end));
        }

        pos = if line_end < len { line_end + 1 } else { len };
    }
    None
}

/// Return the text of the response with all executable code blocks removed,
/// leaving only the LLM's reasoning/explanation text.
pub fn strip_code_blocks(content: &str, blocks: &[CodeBlock]) -> String {
    if blocks.is_empty() {
        return content.to_string();
    }
    let mut result = String::with_capacity(content.len());
    let mut last_end = 0;
    for block in blocks {
        if block.start_offset > last_end {
            result.push_str(&content[last_end..block.start_offset]);
        }
        last_end = block.end_offset;
    }
    if last_end < content.len() {
        result.push_str(&content[last_end..]);
    }
    // Collapse multiple consecutive blank lines
    let trimmed = result.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        trimmed.to_string()
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_single_python_block() {
        let input = "Here's some code:\n```python\nprint('hello')\n```\nDone.";
        let blocks = extract_python_blocks(input);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].code, "print('hello')\n");
        assert_eq!(blocks[0].language_tag, "python");
    }

    #[test]
    fn extract_py_tag() {
        let input = "```py\nx = 42\n```";
        let blocks = extract_python_blocks(input);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].code, "x = 42\n");
    }

    #[test]
    fn extract_python3_tag() {
        let input = "```python3\nimport os\n```";
        let blocks = extract_python_blocks(input);
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn skip_non_python_blocks() {
        let input = "```javascript\nconsole.log('hi')\n```\n```python\nprint('hi')\n```";
        let blocks = extract_python_blocks(input);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].language_tag, "python");
    }

    #[test]
    fn skip_noexec_blocks() {
        let input = "```python:noexec\n# just an example\nprint('demo')\n```";
        let blocks = extract_python_blocks(input);
        assert!(blocks.is_empty());
    }

    #[test]
    fn skip_empty_blocks() {
        let input = "```python\n   \n```";
        let blocks = extract_python_blocks(input);
        assert!(blocks.is_empty());
    }

    #[test]
    fn multiple_blocks() {
        let input = "Step 1:\n```python\na = 1\n```\nStep 2:\n```python\nb = 2\n```";
        let blocks = extract_python_blocks(input);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].code, "a = 1\n");
        assert_eq!(blocks[1].code, "b = 2\n");
    }

    #[test]
    fn unclosed_fence_ignored() {
        let input = "```python\nprint('hello')";
        let blocks = extract_python_blocks(input);
        assert!(blocks.is_empty());
    }

    #[test]
    fn strip_blocks_leaves_reasoning() {
        let input = "I'll do X.\n```python\nprint('hello')\n```\nDone with X.";
        let blocks = extract_python_blocks(input);
        let stripped = strip_code_blocks(input, &blocks);
        assert!(stripped.contains("I'll do X."));
        assert!(stripped.contains("Done with X."));
        assert!(!stripped.contains("print('hello')"));
    }

    #[test]
    fn no_blocks_returns_full_text() {
        let input = "Just some text without code.";
        let blocks = extract_python_blocks(input);
        assert!(blocks.is_empty());
        let stripped = strip_code_blocks(input, &blocks);
        assert_eq!(stripped, input);
    }

    #[test]
    fn block_with_no_language_tag_skipped() {
        let input = "```\nsome text\n```";
        let blocks = extract_python_blocks(input);
        assert!(blocks.is_empty());
    }

    #[test]
    fn mixed_python_and_display_blocks() {
        let input = concat!(
            "Here's the output format:\n",
            "```json\n{\"key\": \"value\"}\n```\n",
            "Now execute:\n",
            "```python\nresult = process(data)\n```\n",
            "And here's an example (don't run):\n",
            "```python:noexec\n# example only\nprint('demo')\n```\n",
        );
        let blocks = extract_python_blocks(input);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].code, "result = process(data)\n");
    }
}
