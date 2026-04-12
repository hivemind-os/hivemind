/// Estimates token count for text. Used to budget context assembly.
pub trait TokenCounter: Send + Sync {
    fn count(&self, text: &str) -> usize;
}

/// Simple approximation: 1 token ≈ 4 characters.
/// Rounds up so even a single character counts as 1 token.
pub struct ApproxTokenCounter;

impl TokenCounter for ApproxTokenCounter {
    fn count(&self, text: &str) -> usize {
        text.len().div_ceil(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_approx_empty() {
        let c = ApproxTokenCounter;
        assert_eq!(c.count(""), 0);
    }

    #[test]
    fn test_approx_single_char() {
        let c = ApproxTokenCounter;
        assert_eq!(c.count("a"), 1);
    }

    #[test]
    fn test_approx_exact_boundary() {
        let c = ApproxTokenCounter;
        assert_eq!(c.count("abcd"), 1); // 4 chars = 1 token
        assert_eq!(c.count("abcde"), 2); // 5 chars = 2 tokens
    }

    #[test]
    fn test_approx_hello_world() {
        let c = ApproxTokenCounter;
        // "hello world" = 11 chars → (11+3)/4 = 3
        assert_eq!(c.count("hello world"), 3);
    }

    #[test]
    fn test_approx_longer_text() {
        let c = ApproxTokenCounter;
        let text = "The quick brown fox jumps over the lazy dog"; // 43 chars
        assert_eq!(c.count(text), 43_usize.div_ceil(4)); // 11
    }
}
