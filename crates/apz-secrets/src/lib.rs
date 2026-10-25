//! The secret shapes AgentPlaza recognises in text, held in one place.
//!
//! The hub's laws flag a tool call whose input carries a secret (`apid`'s `laws::detect_flags`), and the
//! harness's hooks refuse or flag an edit or a command that writes one (`agent-harness`'s `hooks`). Both
//! use [`has_secret`], so the two agree on what a secret looks like.
//!
//! A secret is recognised by its well-known shape, never by a word: the token prefix must start a token (it may
//! not sit inside a longer word), and a run of token characters of at least the shape's length must follow it.
//! The words "token" and "password" are everywhere in ordinary code and are not secrets.

/// The token prefixes and the least length of the run of token characters after each.
const SHAPES: [(&str, usize); 9] = [
    ("sk-ant-", 20),
    ("sk-proj-", 20),
    ("sk-", 32),
    ("ghp_", 30),
    ("github_pat_", 30),
    ("xoxb-", 20),
    ("xoxp-", 20),
    ("AIza", 35),
    ("AKIA", 16),
];

/// The header of a private key block.
const PRIVATE_KEY: &str = "PRIVATE KEY-----";

/// Whether `text` carries a secret by its well-known shape: provider API keys and tokens, cloud access keys,
/// chat-platform tokens and private-key blocks.
pub fn has_secret(text: &str) -> bool {
    shape(text).is_some()
}

/// The shape `text` carries a secret of, named as the detector names it (`PRIVATE KEY` or the token prefix), or
/// `None` when it carries none. A finding names the shape and never the value.
pub fn shape(text: &str) -> Option<&'static str> {
    if text.contains(PRIVATE_KEY) {
        return Some("PRIVATE KEY");
    }
    let token_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    for (prefix, min) in SHAPES {
        let mut from = 0;
        while let Some(at) = text[from..].find(prefix) {
            let start = from + at;
            // The prefix must start a token, not sit inside a longer word.
            let boundary = text[..start]
                .chars()
                .next_back()
                .is_none_or(|c| !token_char(c));
            let run = text[start + prefix.len()..]
                .chars()
                .take_while(|c| token_char(*c))
                .count();
            if boundary && run >= min {
                return Some(prefix);
            }
            from = start + prefix.len();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_shaped_token_is_a_secret_and_a_word_is_not() {
        let key = format!("sk-ant-{}", "a1".repeat(12));
        assert!(has_secret(&format!("export K={key}")));
        assert_eq!(shape(&key), Some("sk-ant-"));
        assert!(!has_secret("the token and the password are config keys"));
        assert!(!has_secret("sk-ant-short"), "too short to be a key");
    }

    #[test]
    fn the_prefix_must_start_a_token() {
        let key = format!("sk-ant-{}", "a1".repeat(12));
        assert!(!has_secret(&format!("my_{key}")), "inside a longer word");
        assert!(has_secret(&format!("\"{key}\"")));
    }

    #[test]
    fn a_private_key_block_is_a_secret() {
        assert_eq!(
            shape("-----BEGIN RSA PRIVATE KEY-----\nMII"),
            Some("PRIVATE KEY")
        );
    }
}
