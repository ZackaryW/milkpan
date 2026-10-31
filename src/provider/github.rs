//! The built-in GitHub provider: repository snapshots, single files and
//! release assets at exact versions.
use panned::{Error, ErrorCode};

use crate::spec::Spec;

/// Validate a GitHub spec and fold the case-insensitive parts (owner and repository).
pub fn canonicalize(mut spec: Spec) -> Result<Spec, Error> {
    let invalid = |why: &str| {
        Error::new(
            ErrorCode::InvalidRequest,
            format!("invalid GitHub spec {spec}: {why}"),
        )
        .with_detail("spec", spec.canonical())
    };
    let parts: Vec<&str> = spec.source.split('/').collect();
    if parts.len() != 2 || parts.iter().any(|p| p.is_empty()) {
        return Err(invalid("source must be owner/repo"));
    }
    match spec.prefix.as_str() {
        "github" if spec.asset.is_some() => return Err(invalid("only github-release specs take #asset")),
        "github-release" if spec.path.is_some() => return Err(invalid("release specs take no path")),
        "github-release" if spec.selector.is_none() || spec.asset.is_none() => {
            return Err(invalid("release specs need @tag (or @latest) and #asset"));
        }
        "github" | "github-release" => {}
        _ => return Err(invalid("not a GitHub prefix")),
    }
    spec.source = spec.source.to_ascii_lowercase();
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical(text: &str) -> Result<String, Error> {
        canonicalize(Spec::parse(text)?).map(|s| s.canonical())
    }

    #[test]
    fn owner_and_repo_fold_case_but_paths_and_refs_do_not() {
        assert_eq!(
            canonical("github:Octo/Tools@Main").unwrap(),
            "github:octo/tools@Main"
        );
        assert_eq!(
            canonical("GITHUB:octo/tools:Docs/A.md").unwrap(),
            "github:octo/tools:Docs/A.md"
        );
        assert_eq!(
            canonical("github-release:Octo/Tools@latest#T-*").unwrap(),
            "github-release:octo/tools@latest#T-*"
        );
    }

    #[test]
    fn invalid_github_specs() {
        for text in [
            "github:octo",
            "github:octo/tools/x",
            "github:o/r#asset",
            "github-release:o/r#a",
            "github-release:o/r@v1",
            "github-release:o/r:p@v1#a",
        ] {
            assert_eq!(
                canonical(text).unwrap_err().code,
                ErrorCode::InvalidRequest,
                "{text}"
            );
        }
    }
}
