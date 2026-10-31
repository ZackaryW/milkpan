//! Providers resolve specs to versions and fetch their content. Each provider
//! claims one or more spec prefixes; the registry routes specs by prefix.
use std::{io::Write, sync::Arc};

use panned::{Error, ErrorCode};

use crate::{spec::Spec, store::ContentKind};

pub mod github;

/// What a spec's selector resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The exact, immutable version (a commit SHA, a release asset id).
    pub version: String,
    /// How the fetched content is presented.
    pub kind: ContentKind,
}

/// A source of resources.
pub trait Provider: Send + Sync {
    /// The spec prefixes this provider handles, such as `github`.
    fn prefixes(&self) -> Vec<String>;
    /// Validate a parsed spec and put it in canonical form.
    fn canonicalize(&self, spec: Spec) -> Result<Spec, Error> {
        Ok(spec)
    }
    /// A short kind name passed to policy as `source.kind`, such as `snapshot`.
    fn kind(&self, spec: &Spec) -> String;
    /// Resolve the spec's selector to an exact version.
    fn resolve(&self, spec: &Spec) -> Result<Resolved, Error>;
    /// Write the content of a resolved version to `out`.
    fn fetch(&self, spec: &Spec, resolved: &Resolved, out: &mut dyn Write) -> Result<(), Error>;
}

/// Providers by prefix.
#[derive(Clone, Default)]
pub struct Registry {
    providers: Vec<Arc<dyn Provider>>,
}

impl Registry {
    /// Add a provider; a later provider for the same prefix wins.
    pub fn register(&mut self, provider: Arc<dyn Provider>) {
        self.providers.insert(0, provider);
    }

    /// Every registered prefix, sorted.
    pub fn prefixes(&self) -> Vec<String> {
        let mut prefixes: Vec<String> = self.providers.iter().flat_map(|p| p.prefixes()).collect();
        prefixes.sort();
        prefixes.dedup();
        prefixes
    }

    /// The provider for a prefix.
    pub fn get(&self, prefix: &str) -> Result<Arc<dyn Provider>, Error> {
        self.providers
            .iter()
            .find(|p| p.prefixes().iter().any(|q| q == prefix))
            .cloned()
            .ok_or_else(|| {
                let known = self.prefixes();
                Error::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "no provider for prefix {prefix:?}; known prefixes: {}",
                        known.join(", ")
                    ),
                )
                .with_detail("prefix", prefix)
                .with_detail("known", panned::Value::List(known))
            })
    }

    /// Parse a spec string, route it and canonicalize it.
    pub fn spec(&self, text: &str) -> Result<(Spec, Arc<dyn Provider>), Error> {
        let spec = Spec::parse(text)?;
        let provider = self.get(&spec.prefix)?;
        Ok((provider.canonicalize(spec)?, provider))
    }
}
