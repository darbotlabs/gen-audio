//! Strip credential-shaped substrings before text is logged or returned.

pub fn redact_secrets(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if let Some(skip) = secret_span(input, index) {
            out.push_str("[redacted]");
            index += skip;
            continue;
        }
        let ch = input[index..].chars().next().unwrap();
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

fn secret_span(input: &str, index: usize) -> Option<usize> {
    let rest = &input[index..];
    const PREFIXES: &[&str] = &[
        "sk-ant-",
        "sk-",
        "ghp_",
        "github_pat_",
        "gho_",
        "AIza",
        "Bearer ",
        "bearer ",
    ];
    for prefix in PREFIXES {
        if rest.starts_with(prefix) {
            let tail = rest[prefix.len()..]
                .chars()
                .take_while(|ch| !ch.is_whitespace() && *ch != '"' && *ch != '\'')
                .count();
            // Count bytes of those chars. Prefixes are ASCII; tokens are ASCII.
            let token_bytes = rest[prefix.len()..]
                .chars()
                .take(tail)
                .map(|ch| ch.len_utf8())
                .sum::<usize>();
            if prefix == &"Bearer " || prefix == &"bearer " || token_bytes >= 6 {
                return Some(prefix.len() + token_bytes);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_common_token_shapes() {
        let raw = "key sk-abc123456789 and ghp_abcdefghijklmnopqrst and Bearer super-secret-value";
        let clean = redact_secrets(raw);
        assert!(!clean.contains("sk-abc"));
        assert!(!clean.contains("ghp_abc"));
        assert!(!clean.contains("super-secret"));
        assert!(clean.contains("[redacted]"));
    }
}
