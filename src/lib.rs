//! A small resource bucket for GitHub, built on panned.
//!
//! Milkpan acquires repository snapshots, single files and release assets,
//! stores them content-addressed, leases them to scopes and reacquires them
//! when their selector moves. Every lease operation is a panned authorization
//! request.
#![warn(missing_docs)]

#[cfg(test)]
mod tests {
    use panned::{Authorizer, Context, Obligation, Request, Scope, StaticRules};

    /// milkpan relies on core static rules carrying obligations (lease settings).
    #[test]
    fn panned_static_rules_carry_obligations() {
        let lease = Obligation::new("lease").with("ttl", "1d");
        let rules = StaticRules::default()
            .allow_with("default", &["acquire"], &["*"], vec![lease.clone()])
            .unwrap();
        let request = Request {
            principal: "user:test".into(),
            scope: Scope::new("default").unwrap(),
            action: "acquire".into(),
            resource: "github:octo/tools".into(),
            context: Context::new(),
        };
        assert_eq!(rules.authorize(&request).unwrap().obligations, [lease]);
    }
}
