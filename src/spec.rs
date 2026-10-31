//! Specs: what to fetch, as `prefix:source[:path][@selector][#asset]`.
//!
//! The grammar is shared by every provider; each provider then validates and
//! canonicalizes the parts it understands. The canonical string is the spec's
//! identity and its panned resource name.
use std::fmt;

use panned::{Error, ErrorCode};
use serde::{Deserialize, Serialize};

/// A parsed spec.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Spec {
    /// The provider prefix, such as `github` or `github-release`.
    pub prefix: String,
    /// The source within the provider, such as `owner/repo`.
    pub source: String,
    /// A path inside the source, for single-file specs.
    pub path: Option<String>,
    /// The selector, such as a branch, tag or commit; `None` means the provider's default.
    pub selector: Option<String>,
    /// An asset name or pattern, for release specs.
    pub asset: Option<String>,
}

impl Spec {
    /// Parse the shared grammar. Providers validate the parts afterwards.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let invalid = |why: &str| {
            Error::new(ErrorCode::InvalidRequest, format!("invalid spec {text:?}: {why}"))
                .with_detail("spec", text)
        };
        let (prefix, rest) = text
            .split_once(':')
            .ok_or_else(|| invalid("missing provider prefix"))?;
        let (rest, asset) = match rest.rsplit_once('#') {
            Some((rest, asset)) => (rest, Some(asset)),
            None => (rest, None),
        };
        let (rest, selector) = match rest.rsplit_once('@') {
            Some((rest, selector)) => (rest, Some(selector)),
            None => (rest, None),
        };
        let (source, path) = match rest.split_once(':') {
            Some((source, path)) => (source, Some(path)),
            None => (rest, None),
        };
        let part = |value: Option<&str>, name: &str| match value {
            Some("") => Err(invalid(&format!("empty {name}"))),
            Some(value) if value.chars().any(char::is_whitespace) => {
                Err(invalid(&format!("whitespace in {name}")))
            }
            other => Ok(other.map(str::to_string)),
        };
        if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(invalid("prefix must be letters, digits or -"));
        }
        if source.is_empty() {
            return Err(invalid("missing source"));
        }
        Ok(Self {
            prefix: prefix.to_ascii_lowercase(),
            source: part(Some(source), "source")?.unwrap_or_default(),
            path: part(path, "path")?.map(|p| p.trim_matches('/').to_string()),
            selector: part(selector, "selector")?,
            asset: part(asset, "asset")?,
        })
    }

    /// The canonical string form.
    pub fn canonical(&self) -> String {
        let mut text = format!("{}:{}", self.prefix, self.source);
        if let Some(path) = &self.path {
            text += &format!(":{path}");
        }
        if let Some(selector) = &self.selector {
            text += &format!("@{selector}");
        }
        if let Some(asset) = &self.asset {
            text += &format!("#{asset}");
        }
        text
    }
}

impl fmt::Display for Spec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

impl TryFrom<String> for Spec {
    type Error = Error;
    fn try_from(value: String) -> Result<Self, Error> {
        Self::parse(&value)
    }
}

impl From<Spec> for String {
    fn from(value: Spec) -> Self {
        value.canonical()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_and_round_trip() {
        let spec = Spec::parse("github:octo/tools:docs/guide.md@main").unwrap();
        assert_eq!(
            (
                spec.source.as_str(),
                spec.path.as_deref(),
                spec.selector.as_deref()
            ),
            ("octo/tools", Some("docs/guide.md"), Some("main"))
        );
        let release = Spec::parse("github-release:octo/tools@v1.0#tools-*.tar.gz").unwrap();
        assert_eq!(release.asset.as_deref(), Some("tools-*.tar.gz"));
        for text in [
            "github:octo/tools",
            "github:octo/tools@main",
            "github-release:o/r@latest#a",
        ] {
            assert_eq!(Spec::parse(text).unwrap().canonical(), text);
        }
        assert_eq!(Spec::parse("GitHub:o/r").unwrap().prefix, "github");
    }

    #[test]
    fn malformed_specs_are_invalid_requests() {
        for text in [
            "octo/tools",
            ":octo/tools",
            "github:",
            "github:@main",
            "github:o/r@",
            "github:o/r#",
            "git hub:o/r",
        ] {
            let error = Spec::parse(text).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidRequest, "{text}");
        }
    }
}
