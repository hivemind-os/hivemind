/// Streaming filter that suppresses `<tool_call>`, `<function_call>`,
/// `<tool_use>`, and `<tool_result>` XML blocks from streamed token
/// deltas so they are not shown to the user.
///
/// Accumulates text in a buffer while a potential tag is being formed.
/// Once a complete opening tag is recognised, all content until the
/// matching close tag is swallowed.  If the buffer turns out not to
/// match any known tag, it is flushed as normal output.
pub(crate) struct StreamingToolCallFilter {
    /// Buffered text that might be the start of a tool tag.
    buffer: String,
    /// When `true`, we are inside a recognised tool-call block and
    /// suppressing all content until the matching close tag.
    suppressing: bool,
    /// The close tag we are looking for (e.g. `</tool_call>`).
    close_tag: &'static str,
}

impl StreamingToolCallFilter {
    const OPEN_TAGS: [(&'static str, &'static str); 4] = [
        ("<tool_call>", "</tool_call>"),
        ("<function_call>", "</function_call>"),
        ("<tool_use>", "</tool_use>"),
        ("<tool_result>", "</tool_result>"),
    ];

    pub(crate) fn new() -> Self {
        Self { buffer: String::new(), suppressing: false, close_tag: "" }
    }

    /// Feed a streaming delta and return text that should be emitted
    /// to the user.  Returns an empty string when the content is being
    /// suppressed (inside a tool block) or buffered (potential tag start).
    pub(crate) fn feed(&mut self, delta: &str) -> String {
        if self.suppressing {
            self.buffer.push_str(delta);
            if let Some(pos) = self.buffer.find(self.close_tag) {
                // End of suppressed block — return any text after the close tag
                let after = self.buffer[pos + self.close_tag.len()..].to_string();
                self.buffer.clear();
                self.suppressing = false;
                if after.is_empty() {
                    return String::new();
                }
                // Recursively feed the remainder in case there are more tags
                return self.feed(&after);
            }
            // Still inside the block — suppress everything
            return String::new();
        }

        self.buffer.push_str(delta);

        // Check if the buffer contains (or could start) an opening tag.
        // We need to handle the case where the tag arrives across
        // multiple deltas (e.g. "<", "tool", "_call>").
        for &(open, close) in &Self::OPEN_TAGS {
            if let Some(tag_start) = self.buffer.find(open) {
                // Full opening tag found — start suppressing
                let before = self.buffer[..tag_start].to_string();
                let rest = self.buffer[tag_start + open.len()..].to_string();
                self.buffer = rest;
                self.suppressing = true;
                self.close_tag = close;
                // Check if the close tag is already in the buffer
                let suppressed = self.feed("");
                let mut out = before;
                out.push_str(&suppressed);
                return out;
            }
            // Check if the buffer ends with a prefix of an opening tag
            if could_be_tag_prefix(&self.buffer, open) {
                // Hold the buffer — don't emit yet
                return String::new();
            }
        }

        // No tag match — flush the buffer

        std::mem::take(&mut self.buffer)
    }

    /// Flush any remaining buffered text (call at end of stream).
    pub(crate) fn flush(&mut self) -> String {
        std::mem::take(&mut self.buffer)
    }
}

/// Check if `buffer` ends with a non-empty prefix of `tag`.
pub(crate) fn could_be_tag_prefix(buffer: &str, tag: &str) -> bool {
    // Check if any suffix of buffer matches a prefix of tag
    let buf_bytes = buffer.as_bytes();
    let tag_bytes = tag.as_bytes();
    for start in (0..buf_bytes.len()).rev() {
        let suffix = &buf_bytes[start..];
        if suffix.len() >= tag_bytes.len() {
            break; // suffix is longer than the tag — already checked for full match
        }
        if tag_bytes.starts_with(suffix) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_tags_passes_through() {
        let mut f = StreamingToolCallFilter::new();
        assert_eq!(f.feed("hello "), "hello ");
        assert_eq!(f.feed("world"), "world");
        assert_eq!(f.flush(), "");
    }

    #[test]
    fn complete_tag_single_delta() {
        let mut f = StreamingToolCallFilter::new();
        let out = f.feed("before<tool_call>{\"tool\":\"x\"}</tool_call>after");
        assert_eq!(out, "beforeafter");
    }

    #[test]
    fn tag_across_two_deltas() {
        let mut f = StreamingToolCallFilter::new();
        assert_eq!(f.feed("text<tool_"), "");
        assert_eq!(f.feed("call>payload</tool_call>done"), "textdone");
    }

    #[test]
    fn tag_across_three_deltas() {
        let mut f = StreamingToolCallFilter::new();
        assert_eq!(f.feed("hi<to"), "");
        assert_eq!(f.feed("ol_ca"), "");
        assert_eq!(f.feed("ll>stuff</tool_call>end"), "hiend");
    }

    #[test]
    fn false_alarm_flushed() {
        let mut f = StreamingToolCallFilter::new();
        // "<" could be start of a tag, but "x" isn't a valid continuation
        assert_eq!(f.feed("<"), "");
        assert_eq!(f.feed("x"), "<x");
    }

    #[test]
    fn flush_partial_buffer() {
        let mut f = StreamingToolCallFilter::new();
        assert_eq!(f.feed("partial<"), "");
        assert_eq!(f.flush(), "partial<");
    }

    #[test]
    fn function_call_tag_variant() {
        let mut f = StreamingToolCallFilter::new();
        let out = f.feed("a<function_call>body</function_call>b");
        assert_eq!(out, "ab");
    }

    #[test]
    fn tool_use_tag_variant() {
        let mut f = StreamingToolCallFilter::new();
        let out = f.feed("x<tool_use>inner</tool_use>y");
        assert_eq!(out, "xy");
    }

    #[test]
    fn tool_result_tag_variant() {
        let mut f = StreamingToolCallFilter::new();
        let out = f.feed("p<tool_result>data</tool_result>q");
        assert_eq!(out, "pq");
    }

    #[test]
    fn adjacent_tool_blocks() {
        let mut f = StreamingToolCallFilter::new();
        let out = f.feed("start<tool_call>a</tool_call><function_call>b</function_call>end");
        assert_eq!(out, "startend");
    }

    #[test]
    fn long_content_between_tags() {
        let mut f = StreamingToolCallFilter::new();
        let long_text = "x".repeat(10_000);
        let input = format!("before<tool_call>{long_text}</tool_call>after");
        let out = f.feed(&input);
        assert_eq!(out, "beforeafter");
    }
}
