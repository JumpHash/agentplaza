//! The gateway's error type. No variant ever carries a credential: provider
//! error bodies are scrubbed of the request's secret before they are kept.

/// Why a gateway call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GatewayError {
    #[error("unknown provider `{0}`")]
    UnknownProvider(String),
    /// The owner turned the provider off: it keeps its settings but takes no calls.
    #[error("the provider `{0}` is disabled")]
    Disabled(String),
    #[error("bad model name `{0}` (expected `<provider>/<model>`)")]
    BadModelName(String),
    /// The credential could not be obtained (the message names its source,
    /// never its value).
    #[error("credentials: {0}")]
    Auth(String),
    /// The secret vault refused the credential: `status` is what the caller should answer with, and
    /// `message` says why (sealed, tampered, or held only by other nodes). It names no value.
    #[error("{message}")]
    Vault { status: u16, message: String },
    /// The provider's configuration is incomplete (no region, no project, …).
    #[error("provider configuration: {0}")]
    Config(String),
    /// The call asks for something the provider's wire format cannot carry (an image, for one): refused before any
    /// request is sent. The message names the provider and the thing.
    #[error("{0}")]
    Unsupported(String),
    /// The provider answered with an error status.
    #[error("HTTP {status}: {body}")]
    Http {
        status: u16,
        body: String,
        retryable: bool,
    },
    /// The provider reported an error inside a stream.
    #[error("provider error: {message}")]
    Provider { message: String, retryable: bool },
    /// The connection failed or broke.
    #[error("transport: {0}")]
    Transport(String),
    /// The provider's answer could not be understood.
    #[error("decode: {0}")]
    Decode(String),
}

/// Why a secret source gave no credential: the vault is sealed, its file was changed outside the API, or
/// the secret is held only by other nodes; `Missing` is the plain "no such secret". The gateway turns the
/// first three into a [`GatewayError::Vault`], and `Missing` into the generic "not set" error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretReason {
    Sealed,
    Tampered(String),
    OutOfScope,
    Missing,
}

impl SecretReason {
    /// The error a call fails with for this refusal: its HTTP status, and the message the owner sees.
    /// `None` for [`SecretReason::Missing`], which the caller words itself.
    pub fn error(&self) -> Option<GatewayError> {
        let (status, message) = match self {
            SecretReason::Sealed => (503, "vault sealed".to_string()),
            SecretReason::Tampered(reason) => (503, format!("vault tampered: {reason}")),
            SecretReason::OutOfScope => (403, "that secret is not scoped to this node".to_string()),
            SecretReason::Missing => return None,
        };
        Some(GatewayError::Vault { status, message })
    }
}

impl GatewayError {
    /// Whether trying the same call again later may succeed.
    pub fn is_retryable(&self) -> bool {
        match self {
            GatewayError::Http { retryable, .. } | GatewayError::Provider { retryable, .. } => {
                *retryable
            }
            GatewayError::Transport(_) => true,
            _ => false,
        }
    }

    pub(crate) fn decode(what: impl std::fmt::Display) -> Self {
        GatewayError::Decode(what.to_string())
    }
}

/// Whether an HTTP status is worth retrying.
pub(crate) fn retryable_status(status: u16) -> bool {
    matches!(status, 408 | 409 | 429) || status >= 500
}

/// `text` cut to at most `max` bytes on a char boundary, with `…` if cut.
pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// `text` with every occurrence of `secret` replaced.
pub(crate) fn redact(text: &str, secret: Option<&str>) -> String {
    match secret {
        Some(s) if s.len() >= 4 => text.replace(s, "[redacted]"),
        _ => text.to_string(),
    }
}
