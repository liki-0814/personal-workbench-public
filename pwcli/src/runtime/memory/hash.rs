// Content fingerprint for memory dedup (normalize + SHA-256 hex).
use sha2::{Digest, Sha256};

pub fn normalize_for_hash(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for ch in text.trim().chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

pub fn fact_content_hash(summary: &str, content: &str) -> String {
    let payload = format!(
        "{}|{}",
        normalize_for_hash(summary),
        normalize_for_hash(content)
    );
    let digest = Sha256::digest(payload.as_bytes());
    format!("{:x}", digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_whitespace() {
        assert_eq!(normalize_for_hash("  hello   world  "), "hello world");
    }

    #[test]
    fn hash_stable_for_equivalent_text() {
        let a = fact_content_hash("sum", "hello   world");
        let b = fact_content_hash("sum", "hello world");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_differs_for_different_content() {
        let a = fact_content_hash("a", "one");
        let b = fact_content_hash("a", "two");
        assert_ne!(a, b);
    }
}
