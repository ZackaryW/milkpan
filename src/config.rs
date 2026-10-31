//! `milkpan.toml`: the policy as panned `[[rule]]` tables, whose obligations
//! carry lease settings. Without a file, a built-in rule allows every lease
//! operation in scope `default`; any configured rule replaces it.
use std::path::Path;

use panned::{Error, ErrorCode, StaticRules, core::Rule};
use serde::Deserialize;

/// The lease operations milkpan authorizes.
pub const ACTIONS: [&str; 4] = ["acquire", "reacquire", "read", "release"];

/// The configuration file name under the bucket home.
pub const FILE: &str = "milkpan.toml";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    rule: Vec<Rule>,
}

/// The policy used when no rule is configured.
pub fn builtin() -> StaticRules {
    StaticRules::default()
        .allow("default", &ACTIONS, &["*"])
        .expect("valid built-in rule")
}

/// Load the policy from `<home>/milkpan.toml`, or the built-in policy without one.
pub fn load(home: &Path) -> Result<StaticRules, Error> {
    let path = home.join(FILE);
    let invalid = |message: String| {
        Error::new(ErrorCode::InvalidRequest, message).with_detail("path", path.display().to_string())
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(builtin()),
        Err(e) => return Err(invalid(format!("cannot read {}: {e}", path.display()))),
    };
    let file: ConfigFile =
        toml::from_str(&text).map_err(|e| invalid(format!("invalid {}: {e}", path.display())))?;
    for (index, rule) in file.rule.iter().enumerate() {
        if rule.actions.is_empty() || rule.resources.is_empty() {
            return Err(invalid(format!(
                "rule {index} needs at least one action and one resource"
            )));
        }
    }
    Ok(if file.rule.is_empty() {
        builtin()
    } else {
        StaticRules::new(file.rule)
    })
}

#[cfg(test)]
mod tests {
    use panned::ScopeList;

    use super::*;

    #[test]
    fn no_file_uses_the_builtin_rule() {
        let home = tempfile::tempdir().unwrap();
        let rules = load(home.path()).unwrap();
        assert_eq!(rules, builtin());
        assert_eq!(rules.scopes().unwrap(), [panned::Scope::new("default").unwrap()]);
    }

    #[test]
    fn configured_rules_replace_the_builtin_rule() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join(FILE),
            r#"
[[rule]]
scope = "ci"
effect = "allow"
actions = ["acquire"]
resources = ["github:octo/*"]
obligations = [{ name = "lease", params = { ttl = "1d" } }]
"#,
        )
        .unwrap();
        let rules = load(home.path()).unwrap();
        assert_eq!(rules.rules.len(), 1);
        assert_eq!(rules.rules[0].obligations[0].params["ttl"], "1d");
    }

    #[test]
    fn broken_files_name_themselves() {
        let home = tempfile::tempdir().unwrap();
        for text in ["[[rule]\n", "[[rule]]\nscope = \"ci\"\n", "unknown = 1\n"] {
            std::fs::write(home.path().join(FILE), text).unwrap();
            let error = load(home.path()).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidRequest, "{text}");
            assert!(error.details["path"].to_string().contains(FILE));
        }
    }
}
