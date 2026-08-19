//! Public sandbox network-update types.

use std::collections::BTreeMap;

/// A single network rule's request transform (e.g. injected headers).
#[derive(Debug, Clone, Default)]
pub struct NetworkRule {
    /// Headers to inject on requests matched by this rule.
    pub transform_headers: BTreeMap<String, String>,
}

/// An atomic update to a sandbox's egress network policy.
///
/// **Replacement semantics:** the update fully replaces the sandbox's policy —
/// any field left empty/`None` is CLEARED on the server, not merged.
#[derive(Debug, Clone, Default)]
pub struct SandboxNetworkUpdate {
    /// Whether the sandbox may reach the public internet.
    pub allow_internet_access: Option<bool>,
    /// Allowed egress destinations (domains/CIDRs).
    pub allow_out: Vec<String>,
    /// Denied egress destinations (domains/CIDRs).
    pub deny_out: Vec<String>,
    /// Per-destination request rules, keyed by destination.
    pub rules: BTreeMap<String, Vec<NetworkRule>>,
}

impl SandboxNetworkUpdate {
    /// Build the `SandboxNetworkUpdateConfig` request body. Field casing matches
    /// the spec: snake_case `allow_internet_access`, camelCase `allowOut`/`denyOut`.
    ///
    /// Empty fields are omitted (matching the JS SDK and the generated type's
    /// `skip_serializing_if`); under the endpoint's atomic-replace semantics an
    /// omitted field is cleared server-side, so omitting an empty list still
    /// clears it.
    pub(crate) fn to_wire_body(&self) -> serde_json::Value {
        let mut body = serde_json::Map::new();
        if let Some(allow) = self.allow_internet_access {
            body.insert(
                "allow_internet_access".to_string(),
                serde_json::Value::Bool(allow),
            );
        }
        if !self.allow_out.is_empty() {
            body.insert("allowOut".to_string(), serde_json::json!(self.allow_out));
        }
        if !self.deny_out.is_empty() {
            body.insert("denyOut".to_string(), serde_json::json!(self.deny_out));
        }
        if !self.rules.is_empty() {
            let rules: serde_json::Map<String, serde_json::Value> = self
                .rules
                .iter()
                .map(|(dest, rules)| {
                    let arr: Vec<serde_json::Value> = rules
                        .iter()
                        .map(|r| {
                            if r.transform_headers.is_empty() {
                                serde_json::json!({})
                            } else {
                                serde_json::json!({ "transform": { "headers": r.transform_headers } })
                            }
                        })
                        .collect();
                    (dest.clone(), serde_json::Value::Array(arr))
                })
                .collect();
            body.insert("rules".to_string(), serde_json::Value::Object(rules));
        }
        serde_json::Value::Object(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_wire_body_uses_correct_casing() {
        let mut update = SandboxNetworkUpdate {
            allow_internet_access: Some(true),
            allow_out: vec!["1.1.1.1".to_string()],
            ..Default::default()
        };
        update
            .rules
            .entry("example.com".to_string())
            .or_default()
            .push(NetworkRule {
                transform_headers: [("X-Test".to_string(), "1".to_string())]
                    .into_iter()
                    .collect(),
            });
        let body = update.to_wire_body();
        // snake_case allow_internet_access, camelCase allowOut/denyOut.
        assert_eq!(body["allow_internet_access"], serde_json::json!(true));
        assert_eq!(body["allowOut"], serde_json::json!(["1.1.1.1"]));
        // Empty deny_out is omitted (matches JS + skip_serializing_if), not sent as [].
        assert!(body.get("denyOut").is_none());
        assert_eq!(
            body["rules"]["example.com"][0]["transform"]["headers"]["X-Test"],
            serde_json::json!("1")
        );
    }

    #[test]
    fn to_wire_body_omits_empty_fields() {
        // A fully-default update serializes to an empty object (all fields omitted).
        let body = SandboxNetworkUpdate::default().to_wire_body();
        assert_eq!(body, serde_json::json!({}));

        // A rule with no transform headers serializes to `{}` (no empty transform).
        let mut update = SandboxNetworkUpdate::default();
        update
            .rules
            .entry("example.com".to_string())
            .or_default()
            .push(NetworkRule::default());
        let body = update.to_wire_body();
        assert_eq!(body["rules"]["example.com"][0], serde_json::json!({}));
        assert!(body.get("allowOut").is_none());
    }
}
