//! Module boundaries (ADR 0002): a module crate may depend only on the crates
//! listed for it. Adding an entry is an architecture decision, not a fix for a
//! failing build.

use std::collections::{BTreeMap, BTreeSet};

use cargo_metadata::{DependencyKind, MetadataCommand};

/// Workspace crates each crate may depend on. `None` means any: only the app,
/// which composes every module.
fn allowed() -> BTreeMap<&'static str, Option<&'static [&'static str]>> {
    const SHARED: &[&str] = &["mascate-kernel", "mascate-platform"];
    BTreeMap::from([
        ("mascate", None),
        ("mascate-architecture", Some(&[][..])),
        ("mascate-kernel", Some(&[][..])),
        ("mascate-platform", Some(&["mascate-kernel"][..])),
        ("mascate-catalog", Some(SHARED)),
        // Receiving and selling move stock through Inventory (ADR 0012).
        (
            "mascate-commerce",
            Some(&["mascate-kernel", "mascate-platform", "mascate-inventory"][..]),
        ),
        ("mascate-inventory", Some(SHARED)),
        ("mascate-affiliate", Some(SHARED)),
        ("mascate-marketing", Some(SHARED)),
        // Adapters implement the ports the modules define (ADR 0013): demand
        // for the catalog, the Sales Channel for commerce.
        (
            "mascate-integrations",
            Some(
                &[
                    "mascate-kernel",
                    "mascate-platform",
                    "mascate-catalog",
                    "mascate-commerce",
                ][..],
            ),
        ),
        ("mascate-finance", Some(SHARED)),
    ])
}

/// Every workspace crate with the other workspace crates it depends on,
/// counting normal, build and dev dependencies.
fn workspace_dependencies() -> BTreeMap<String, BTreeSet<String>> {
    let metadata = MetadataCommand::new()
        .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
        .no_deps()
        .exec()
        .expect("cargo metadata runs");
    let members: BTreeSet<String> = metadata
        .workspace_packages()
        .iter()
        .map(|package| package.name.to_string())
        .collect();
    metadata
        .workspace_packages()
        .into_iter()
        .map(|package| {
            let internal = package
                .dependencies
                .iter()
                .filter(|dependency| {
                    matches!(
                        dependency.kind,
                        DependencyKind::Normal
                            | DependencyKind::Build
                            | DependencyKind::Development
                    )
                })
                .map(|dependency| dependency.name.clone())
                .filter(|name| members.contains(name) && *name != package.name.as_str())
                .collect();
            (package.name.to_string(), internal)
        })
        .collect()
}

#[test]
fn every_crate_has_a_rule() {
    let rules = allowed();
    let missing: Vec<_> = workspace_dependencies()
        .into_keys()
        .filter(|name| !rules.contains_key(name.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "add these crates to the allowed list: {missing:?}"
    );
}

#[test]
fn crates_depend_only_on_what_their_module_allows() {
    let rules = allowed();
    let violations: Vec<String> = workspace_dependencies()
        .into_iter()
        .filter_map(|(name, dependencies)| {
            let allowed: BTreeSet<&str> = rules
                .get(name.as_str())?
                .as_ref()?
                .iter()
                .copied()
                .collect();
            let forbidden: Vec<_> = dependencies
                .iter()
                .filter(|dependency| !allowed.contains(dependency.as_str()))
                .collect();
            (!forbidden.is_empty()).then(|| format!("{name} -> {forbidden:?}"))
        })
        .collect();
    assert!(
        violations.is_empty(),
        "forbidden dependencies: {violations:#?}"
    );
}

#[test]
fn nothing_depends_on_the_app() {
    let dependents: Vec<_> = workspace_dependencies()
        .into_iter()
        .filter(|(_, dependencies)| dependencies.contains("mascate"))
        .map(|(name, _)| name)
        .collect();
    assert!(
        dependents.is_empty(),
        "only the app composes modules: {dependents:?}"
    );
}
