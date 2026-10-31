//! The README's configuration example is a valid policy that behaves as described.
use std::sync::Arc;

use milkpan::{BucketBuilder, config};
use panned::{Authorizer, Context, Request, Scope};

fn readme_toml() -> String {
    let readme = include_str!("../README.md");
    let start = readme.find("```toml\n").expect("README has a toml example") + "```toml\n".len();
    readme[start..].split("```").next().unwrap().to_string()
}

#[test]
fn readme_configuration_loads_and_decides_as_documented() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(config::FILE), readme_toml()).unwrap();
    let rules = Arc::new(config::load(home.path()).unwrap());
    let decide = |action: &str, resource: &str| {
        rules.authorize(&Request {
            principal: "user:test".into(),
            scope: Scope::new("ci").unwrap(),
            action: action.into(),
            resource: resource.into(),
            context: Context::new(),
        })
    };
    let fast = decide("acquire", "github:octo/fast-cli").unwrap();
    let names: Vec<_> = fast.obligations.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["lease", "verify"]);
    assert!(!decide("acquire", "github:octo/archived-old").unwrap().allowed());
    assert!(!decide("acquire", "github:other/repo").unwrap().allowed());
    assert!(BucketBuilder::new(home.path()).build().is_ok());
}
