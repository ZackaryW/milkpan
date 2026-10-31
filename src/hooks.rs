//! Hooks that fulfil a decision's obligations, and hooks that may veto reclaiming a blob.
//!
//! Every obligation of an allow decision must be handled by some after-fetch
//! hook, or the operation fails. The built-in hooks are `verify` (re-hash the
//! blob) and `lease` (lease settings).
use std::collections::BTreeMap;

use panned::{Error, ErrorCode, Obligation};

use crate::{code, spec::Spec, state::LeaseSettings, store::Store};

/// The content an obligation applies to.
pub struct Stored<'a> {
    /// The bucket's store.
    pub store: &'a Store,
    /// The spec being acquired or read.
    pub spec: &'a Spec,
    /// The blob holding its content.
    pub digest: &'a str,
}

/// Fulfils obligations after content is available and before a lease is recorded.
pub trait AfterFetch: Send + Sync {
    /// Whether this hook fulfils obligations named `obligation`.
    fn handles(&self, obligation: &str) -> bool;
    /// Fulfil every obligation this hook handles, at once. `lease` is the
    /// settings of the lease being recorded, or `None` for operations that
    /// record no lease (such as reading).
    fn run(
        &self,
        stored: &Stored,
        obligations: &[Obligation],
        lease: Option<&mut LeaseSettings>,
    ) -> Result<(), Error>;
}

/// Asked before a blob is deleted by reclamation.
pub trait BeforeReclaim: Send + Sync {
    /// `Ok(false)` keeps the blob until the next reclamation.
    fn allow(&self, digest: &str) -> Result<bool, Error>;
}

/// Hand each obligation to the first hook that handles it and run the hooks.
pub(crate) fn fulfil(
    hooks: &[std::sync::Arc<dyn AfterFetch>],
    stored: &Stored,
    obligations: &[Obligation],
    mut lease: Option<&mut LeaseSettings>,
) -> Result<(), Error> {
    let mut assigned: Vec<Vec<Obligation>> = vec![Vec::new(); hooks.len()];
    for obligation in obligations {
        let index = hooks
            .iter()
            .position(|h| h.handles(&obligation.name))
            .ok_or_else(|| {
                Error::new(
                    code("unmet_obligation"),
                    format!("no hook fulfils obligation {:?}", obligation.name),
                )
                .with_detail("obligation", obligation.name.as_str())
                .with_detail("spec", stored.spec.canonical())
            })?;
        assigned[index].push(obligation.clone());
    }
    for (hook, obligations) in hooks.iter().zip(assigned) {
        if obligations.is_empty() {
            continue;
        }
        hook.run(stored, &obligations, lease.as_deref_mut())
            .map_err(|error| match error.code {
                ErrorCode::Tool(ref name) if name.starts_with("milkpan.") => error,
                _ => Error::new(
                    code("unmet_obligation"),
                    format!("obligation hook failed: {}", error.message),
                )
                .with_detail("cause", error.code.as_str())
                .with_detail("spec", stored.spec.canonical()),
            })?;
    }
    Ok(())
}

/// The built-in `verify` hook: re-hash the blob and compare it with its digest.
#[derive(Debug, Clone, Copy, Default)]
pub struct VerifyHook;

impl AfterFetch for VerifyHook {
    fn handles(&self, obligation: &str) -> bool {
        obligation == "verify"
    }

    fn run(&self, stored: &Stored, _: &[Obligation], _: Option<&mut LeaseSettings>) -> Result<(), Error> {
        if stored.store.verify(stored.digest)? {
            Ok(())
        } else {
            Err(Error::new(
                code("unmet_obligation"),
                "stored content does not match its digest",
            )
            .with_detail("obligation", "verify")
            .with_detail("digest", stored.digest))
        }
    }
}

/// The built-in `lease` hook: merge `ttl`, `refresh` and `retain` from every
/// `lease` obligation key by key; two different values for one key conflict.
#[derive(Debug, Clone, Copy, Default)]
pub struct LeaseHook;

impl AfterFetch for LeaseHook {
    fn handles(&self, obligation: &str) -> bool {
        obligation == "lease"
    }

    fn run(
        &self,
        _: &Stored,
        obligations: &[Obligation],
        lease: Option<&mut LeaseSettings>,
    ) -> Result<(), Error> {
        let mut merged: BTreeMap<&str, &str> = BTreeMap::new();
        for obligation in obligations {
            for (key, value) in &obligation.params {
                if let Some(previous) = merged.insert(key, value)
                    && previous != value
                {
                    return Err(Error::new(
                        code("conflicting_obligation"),
                        format!("lease {key} is set to both {previous:?} and {value:?}"),
                    )
                    .with_detail("parameter", key.as_str())
                    .with_detail(
                        "values",
                        panned::Value::List(vec![previous.into(), value.clone()]),
                    ));
                }
            }
        }
        let invalid = |key: &str, value: &str| {
            Error::new(code("unmet_obligation"), format!("invalid lease {key} {value:?}"))
                .with_detail("obligation", "lease")
                .with_detail("parameter", key)
        };
        let mut settings = LeaseSettings::default();
        for (key, value) in merged {
            match key {
                "ttl" => settings.ttl = parse_duration(value).ok_or_else(|| invalid(key, value))?,
                "refresh" => settings.refresh = parse_duration(value).ok_or_else(|| invalid(key, value))?,
                "retain" => settings.retain = value.parse().map_err(|_| invalid(key, value))?,
                _ => return Err(invalid(key, value)),
            }
        }
        if let Some(lease) = lease {
            *lease = settings;
        }
        Ok(())
    }
}

/// Parse `30s`, `10m`, `1h` or `7d` into seconds.
pub fn parse_duration(text: &str) -> Option<u64> {
    let unit = match text.chars().last()? {
        's' => 1,
        'm' => 60,
        'h' => 3_600,
        'd' => 86_400,
        _ => return None,
    };
    text[..text.len() - 1].parse::<u64>().ok()?.checked_mul(unit)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn run(obligations: &[Obligation]) -> Result<LeaseSettings, Error> {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path());
        let spec = Spec::parse("github:o/r").unwrap();
        let stored = Stored {
            store: &store,
            spec: &spec,
            digest: "00",
        };
        let hooks: Vec<Arc<dyn AfterFetch>> = vec![Arc::new(LeaseHook)];
        let mut settings = LeaseSettings::default();
        fulfil(&hooks, &stored, obligations, Some(&mut settings)).map(|()| settings)
    }

    fn lease(params: &[(&str, &str)]) -> Obligation {
        params
            .iter()
            .fold(Obligation::new("lease"), |o, (k, v)| o.with(*k, *v))
    }

    #[test]
    fn lease_settings_default_merge_and_conflict() {
        assert_eq!(run(&[]).unwrap(), LeaseSettings::default());
        let merged = run(&[
            lease(&[("ttl", "1d")]),
            lease(&[("refresh", "10m"), ("ttl", "1d")]),
        ])
        .unwrap();
        assert_eq!(
            merged,
            LeaseSettings {
                ttl: 86_400,
                refresh: 600,
                retain: 0
            }
        );
        let conflict = run(&[lease(&[("ttl", "1d")]), lease(&[("ttl", "7d")])]).unwrap_err();
        assert_eq!(conflict.code.as_str(), "milkpan.conflicting_obligation");
        assert!(
            conflict.message.contains("ttl")
                && conflict.message.contains("1d")
                && conflict.message.contains("7d")
        );
        for bad in [("ttl", "soon"), ("retain", "-1"), ("color", "red")] {
            let error = run(&[lease(&[bad])]).unwrap_err();
            assert_eq!(error.code.as_str(), "milkpan.unmet_obligation", "{bad:?}");
        }
    }

    #[test]
    fn unhandled_obligations_fail() {
        let error = run(&[Obligation::new("scan")]).unwrap_err();
        assert_eq!(error.code.as_str(), "milkpan.unmet_obligation");
        assert!(error.message.contains("scan"));
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30s"), Some(30));
        assert_eq!(parse_duration("2h"), Some(7_200));
        for bad in ["", "h", "1x", "1.5h", "-1d"] {
            assert_eq!(parse_duration(bad), None, "{bad}");
        }
    }
}
