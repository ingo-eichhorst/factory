//! One entry point, six level routers (#193 phase 6, slice S3).
//!
//! `Engine::handle` stays the single entry point and `access.rs` the single
//! authorisation check; this is only what `handle` calls after both. A request
//! is routed by `Request::level`, and each level's arms live in their own file.
//! The routers are still `impl Engine`: the services that will replace them come
//! later (S5 onwards), and the level-ownership guard watches what each file reaches.
use crate::engine::*;
use factory_core::protocol::RouteLevel;

mod l1;
mod l2;
mod l3;
mod l4;
mod l5;
mod l6;
mod own;

/// `Request::level` and a router's arms disagree: a bug in the table or the
/// router, never something a caller can cause. `router_tests` pins them together.
pub(super) fn misrouted(level: RouteLevel) -> FactoryError {
    FactoryError::Other(anyhow::anyhow!(
        "a request routed to {level:?} has no arm there: Request::level and the router disagree"
    ))
}

impl Engine {
    /// The request dispatcher. Each level's router is boxed so that no one
    /// future carries every arm's state (see `request_future_stays_bounded`).
    pub(crate) async fn dispatch_request(
        self: &Arc<Self>,
        caller: &crate::access::Caller,
        req: Request,
    ) -> Result<Payload> {
        match req.level() {
            RouteLevel::L1 => Box::pin(self.route_l1(caller, req)).await,
            RouteLevel::L2 => Box::pin(self.route_l2(caller, req)).await,
            RouteLevel::L3 => Box::pin(self.route_l3(caller, req)).await,
            RouteLevel::L4 => Box::pin(self.route_l4(caller, req)).await,
            RouteLevel::L5 => Box::pin(self.route_l5(caller, req)).await,
            RouteLevel::L6 => Box::pin(self.route_l6(caller, req)).await,
            RouteLevel::Router => Box::pin(self.route_own(caller, req)).await,
        }
    }
}

#[cfg(test)]
mod tests {
    //! `Request::level` and the routers' arms must agree, request by request.
    //! A request filed under L4 with no arm in `l4.rs` would answer
    //! "no arm there" at runtime; this finds it at test time instead.
    use std::collections::{BTreeMap, BTreeSet};

    const PROTOCOL: &str = include_str!("../../../factory-interfaces/src/protocol.rs");
    const FILES: [(&str, &str); 7] = [
        ("L1", include_str!("l1.rs")),
        ("L2", include_str!("l2.rs")),
        ("L3", include_str!("l3.rs")),
        ("L4", include_str!("l4.rs")),
        ("L5", include_str!("l5.rs")),
        ("L6", include_str!("l6.rs")),
        ("Router", include_str!("own.rs")),
    ];

    /// variant -> level, read from `Request::level`'s own table.
    fn table() -> BTreeMap<String, String> {
        let start = PROTOCOL.find("pub fn level(&self)").expect("Request::level");
        let mut level = String::new();
        let mut out = BTreeMap::new();
        for line in PROTOCOL[start..].lines().skip(1) {
            if let Some(name) = line.trim().strip_prefix("// ") {
                level = name.to_string();
            } else if line.starts_with("        }") {
                break;
            }
            let mut rest = line;
            while let Some(at) = rest.find("Request::") {
                rest = &rest[at + "Request::".len()..];
                let name: String = rest.chars().take_while(|c| c.is_alphanumeric()).collect();
                out.insert(name, level.clone());
            }
        }
        out
    }

    /// The variants a router file has an arm for: `Request::X` at the arm indent.
    fn arms(source: &str) -> BTreeSet<String> {
        source
            .lines()
            .filter_map(|line| line.strip_prefix("            Request::"))
            .map(|rest| rest.chars().take_while(|c| c.is_alphanumeric()).collect())
            .collect()
    }

    #[test]
    fn every_request_has_exactly_one_arm_and_it_is_in_its_levels_router() {
        let table = table();
        assert_eq!(table.len(), 141, "the routing table covers every Request variant");
        let mut seen = BTreeSet::new();
        for (level, source) in FILES {
            let expected: BTreeSet<String> = table
                .iter()
                .filter(|(_, l)| l.as_str() == level)
                .map(|(name, _)| name.clone())
                .collect();
            let actual = arms(source);
            assert_eq!(actual, expected, "{level}: router arms and Request::level disagree");
            for name in actual {
                assert!(seen.insert(name.clone()), "{name} has arms in two routers");
            }
        }
        assert_eq!(seen.len(), 141);
    }
}
