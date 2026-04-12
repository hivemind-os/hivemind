use hive_classification::DataClass;
use hive_contracts::connectors::{ResolvedResourcePolicy, ResourceRule};
use hive_contracts::ToolApproval;

/// Resolves the effective policy for a resource address by matching
/// against a connector's resource rules, falling back to connector defaults.
///
/// Resolution order (most specific first):
/// 1. Exact address match beats partial glob.
/// 2. At equal specificity, stricter approval wins (Deny > Ask > Auto).
/// 3. Fallback to connector-level defaults.
pub struct ResourceResolver {
    rules: Vec<ResourceRule>,
    default_input_class: DataClass,
    default_output_class: DataClass,
}

impl ResourceResolver {
    pub fn new(
        rules: Vec<ResourceRule>,
        default_input_class: DataClass,
        default_output_class: DataClass,
    ) -> Self {
        Self { rules, default_input_class, default_output_class }
    }

    /// Resolve the effective policy for the given resource address.
    pub fn resolve(&self, destination: &str) -> ResolvedResourcePolicy {
        let mut best: Option<(usize, &ResourceRule)> = None;

        for rule in &self.rules {
            if !pattern_matches(&rule.pattern, destination) {
                continue;
            }
            let specificity = pattern_specificity(&rule.pattern);

            let dominated = match best {
                Some((best_spec, best_rule)) => {
                    if specificity > best_spec {
                        false
                    } else if specificity == best_spec {
                        approval_priority(rule.approval) <= approval_priority(best_rule.approval)
                    } else {
                        true
                    }
                }
                None => false,
            };

            if !dominated {
                best = Some((specificity, rule));
            }
        }

        match best {
            Some((_, rule)) => ResolvedResourcePolicy {
                approval: rule.approval,
                input_class: rule.input_class_override.unwrap_or(self.default_input_class),
                output_class: rule.output_class_override.unwrap_or(self.default_output_class),
            },
            None => ResolvedResourcePolicy {
                approval: ToolApproval::Ask,
                input_class: self.default_input_class,
                output_class: self.default_output_class,
            },
        }
    }
}

/// Match a glob-style pattern against a resource address.
///
/// Supports:
/// - `*` — matches everything
/// - `*@domain.com` — matches any user at domain
/// - `user@*` — matches user at any domain
/// - `exact@address.com` — exact match
/// - `*@*.domain.com` — matches subdomains
fn pattern_matches(pattern: &str, address: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let pat = pattern.to_lowercase();
    let addr = address.to_lowercase();

    // Split into segments on '*' and check if the address matches
    // all literal segments in order.
    let parts: Vec<&str> = pat.split('*').collect();

    if parts.len() == 1 {
        // No wildcard — exact match.
        return addr == pat;
    }

    let mut remaining = addr.as_str();

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            // First segment must be a prefix.
            if !remaining.starts_with(part) {
                return false;
            }
            remaining = &remaining[part.len()..];
        } else if i == parts.len() - 1 {
            // Last segment must be a suffix.
            if !remaining.ends_with(part) {
                return false;
            }
            remaining = "";
        } else {
            // Middle segment must appear somewhere.
            match remaining.find(part) {
                Some(idx) => remaining = &remaining[idx + part.len()..],
                None => return false,
            }
        }
    }

    true
}

/// Specificity score: longer non-wildcard content = more specific.
fn pattern_specificity(pattern: &str) -> usize {
    if pattern == "*" {
        return 0;
    }
    pattern.chars().filter(|c| *c != '*').count()
}

fn approval_priority(a: ToolApproval) -> u8 {
    match a {
        ToolApproval::Deny => 2,
        ToolApproval::Ask => 1,
        ToolApproval::Auto => 0,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matches_everything() {
        assert!(pattern_matches("*", "anyone@example.com"));
    }

    #[test]
    fn domain_glob_matches() {
        assert!(pattern_matches("*@outlook.com", "alice@outlook.com"));
        assert!(pattern_matches("*@outlook.com", "BOB@OUTLOOK.COM"));
        assert!(!pattern_matches("*@outlook.com", "alice@gmail.com"));
    }

    #[test]
    fn exact_match() {
        assert!(pattern_matches("boss@gmail.com", "boss@gmail.com"));
        assert!(!pattern_matches("boss@gmail.com", "other@gmail.com"));
    }

    #[test]
    fn subdomain_glob() {
        assert!(pattern_matches("*@*.example.com", "user@mail.example.com"));
        assert!(!pattern_matches("*@*.example.com", "user@example.com"));
    }

    #[test]
    fn resolver_exact_beats_glob() {
        let resolver = ResourceResolver::new(
            vec![
                ResourceRule {
                    pattern: "*@gmail.com".into(),
                    approval: ToolApproval::Ask,
                    input_class_override: None,
                    output_class_override: None,
                },
                ResourceRule {
                    pattern: "boss@gmail.com".into(),
                    approval: ToolApproval::Deny,
                    input_class_override: None,
                    output_class_override: None,
                },
            ],
            DataClass::Internal,
            DataClass::Internal,
        );

        let policy = resolver.resolve("boss@gmail.com");
        assert_eq!(policy.approval, ToolApproval::Deny);

        let policy = resolver.resolve("random@gmail.com");
        assert_eq!(policy.approval, ToolApproval::Ask);
    }

    #[test]
    fn resolver_uses_connector_defaults() {
        let resolver = ResourceResolver::new(
            vec![ResourceRule {
                pattern: "*@outlook.com".into(),
                approval: ToolApproval::Auto,
                input_class_override: Some(DataClass::Confidential),
                output_class_override: None,
            }],
            DataClass::Internal,
            DataClass::Public,
        );

        let policy = resolver.resolve("alice@outlook.com");
        assert_eq!(policy.approval, ToolApproval::Auto);
        assert_eq!(policy.input_class, DataClass::Confidential);
        assert_eq!(policy.output_class, DataClass::Public); // connector default

        let policy = resolver.resolve("unknown@example.com");
        assert_eq!(policy.approval, ToolApproval::Ask); // fallback
        assert_eq!(policy.input_class, DataClass::Internal); // connector default
    }

    #[test]
    fn resolver_stricter_wins_at_equal_specificity() {
        let resolver = ResourceResolver::new(
            vec![
                ResourceRule {
                    pattern: "*@company.com".into(),
                    approval: ToolApproval::Auto,
                    input_class_override: None,
                    output_class_override: None,
                },
                ResourceRule {
                    pattern: "*@company.com".into(),
                    approval: ToolApproval::Ask,
                    input_class_override: None,
                    output_class_override: None,
                },
            ],
            DataClass::Internal,
            DataClass::Internal,
        );

        let policy = resolver.resolve("user@company.com");
        assert_eq!(policy.approval, ToolApproval::Ask);
    }
}
