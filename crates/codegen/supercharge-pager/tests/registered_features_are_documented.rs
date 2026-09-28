//! `FEATURES` is the source of truth and the operator tables are hand-maintained mirrors with no compile-time check of their own.
//! This test is that check.

use supercharge_shell::agent::config::FEATURES;

#[test]
fn every_registered_feature_reaches_the_operator() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let enterprise_path = root.join("docs/internal/25-enterprise.md");
    let env_path = root.join("docs/internal/22-environment-variables.md");
    let (Ok(enterprise), Ok(env_vars)) = (
        std::fs::read_to_string(&enterprise_path),
        std::fs::read_to_string(&env_path),
    ) else {
        // These operator notes are not part of the public checkout.
        return;
    };
    for spec in FEATURES {
        assert!(
            enterprise.contains(&format!("`{}`", spec.key)),
            "{} has no row in the 25-enterprise.md pinning table",
            spec.key,
        );
        assert!(
            env_vars.contains(&format!("`{}`", spec.env)),
            "{} is undocumented in 22-environment-variables.md",
            spec.env,
        );
    }
}
