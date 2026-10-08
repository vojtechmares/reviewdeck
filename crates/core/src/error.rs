//! The one error type every fallible core function returns.
//!
//! Its `Display` is the sentence the user reads, already phrased for them, so the UI
//! shows it verbatim and never has to know which layer an error came from.

use std::fmt;

#[derive(Debug, Clone)]
pub enum Error {
    /// A non-2xx response or a transport failure (`status` 0). `message` is already
    /// the human sentence from `http::describe`; `body` is what the host sent back,
    /// for the adapters that look for more detail in it.
    Api {
        status: u16,
        url: String,
        message: String,
        body: Option<String>,
    },
    /// Anything else the user should read verbatim.
    Message(String),
    /// Some drafts of a review landed before the submission failed (the
    /// `PartialSubmitError` of providers/submit.ts). `posted` holds the ids of the
    /// drafts that landed, so they can be dropped rather than sent twice.
    PartialSubmit {
        message: String,
        posted: Vec<String>,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Api { message, .. }
            | Error::Message(message)
            | Error::PartialSubmit { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Shorthand for [`Error::Message`].
pub fn msg(s: impl Into<String>) -> Error {
    Error::Message(s.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_the_message_whatever_the_variant() {
        let api = Error::Api {
            status: 404,
            url: "https://example.test/x".into(),
            message: "Not found on example.test (/x).".into(),
            body: Some("{}".into()),
        };
        assert_eq!(api.to_string(), "Not found on example.test (/x).");
        assert_eq!(
            msg("A host is required.").to_string(),
            "A host is required."
        );
        let partial = Error::PartialSubmit {
            message: "Only some comments were posted.".into(),
            posted: vec!["d1".into()],
        };
        assert_eq!(partial.to_string(), "Only some comments were posted.");
    }
}
