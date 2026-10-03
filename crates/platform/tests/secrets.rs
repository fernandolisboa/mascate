use std::collections::BTreeMap;
use std::sync::Arc;

use mascate_platform::testing::MemorySecretStore;
use mascate_platform::{Build, Environment, Secret, SecretStore, secret_store_for};

fn environment(variables: &[(&str, &str)]) -> Environment {
    let variables: BTreeMap<String, String> = variables
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    Box::new(move |name| variables.get(name).cloned())
}

fn system_with(name: &str, value: &str) -> Arc<MemorySecretStore> {
    let system = Arc::new(MemorySecretStore::default());
    system.write(name, &Secret::new(value)).unwrap();
    system
}

#[test]
fn a_development_build_reads_the_environment_before_the_system_store() {
    let system = system_with("ANTHROPIC_API_KEY", "from-store");
    let store = secret_store_for(
        Build::Development,
        system,
        environment(&[("ANTHROPIC_API_KEY", "from-env")]),
    );

    let secret = store.read("ANTHROPIC_API_KEY").unwrap();

    assert_eq!(secret, Some(Secret::new("from-env")));
}

#[test]
fn a_release_build_never_reads_the_environment() {
    let system = system_with("ANTHROPIC_API_KEY", "from-store");
    let store = secret_store_for(
        Build::Release,
        system,
        environment(&[("ANTHROPIC_API_KEY", "from-env"), ("ML_CLIENT_ID", "id")]),
    );

    assert_eq!(
        store.read("ANTHROPIC_API_KEY").unwrap(),
        Some(Secret::new("from-store"))
    );
    assert_eq!(store.read("ML_CLIENT_ID").unwrap(), None);
}

#[test]
fn without_a_variable_a_development_build_reads_the_system_store() {
    let system = system_with("ML_CLIENT_ID", "from-store");
    let store = secret_store_for(
        Build::Development,
        system,
        environment(&[("ML_CLIENT_SECRET", "other")]),
    );

    assert_eq!(
        store.read("ML_CLIENT_ID").unwrap(),
        Some(Secret::new("from-store"))
    );
}

#[test]
fn a_blank_variable_counts_as_unset_and_values_are_trimmed() {
    let system = system_with("ML_CLIENT_ID", "from-store");
    let store = secret_store_for(
        Build::Development,
        system,
        environment(&[("ML_CLIENT_ID", "  "), ("ML_CLIENT_SECRET", " s3cret\n")]),
    );

    assert_eq!(
        store.read("ML_CLIENT_ID").unwrap(),
        Some(Secret::new("from-store"))
    );
    assert_eq!(
        store.read("ML_CLIENT_SECRET").unwrap(),
        Some(Secret::new("s3cret"))
    );
}

#[test]
fn in_a_development_build_writes_and_removals_reach_the_system_store() {
    let system = Arc::new(MemorySecretStore::default());
    let store = secret_store_for(
        Build::Development,
        system.clone(),
        environment(&[("ML_CLIENT_ID", "from-env")]),
    );

    store.write("ML_CLIENT_ID", &Secret::new("pasted")).unwrap();
    assert_eq!(
        system.read("ML_CLIENT_ID").unwrap(),
        Some(Secret::new("pasted"))
    );
    assert_eq!(
        store.read("ML_CLIENT_ID").unwrap(),
        Some(Secret::new("from-env"))
    );

    store.remove("ML_CLIENT_ID").unwrap();
    assert_eq!(system.read("ML_CLIENT_ID").unwrap(), None);
}

#[test]
fn a_secret_never_shows_its_value_when_debug_printed() {
    let secret = Secret::new("s3cret-value");

    let printed = format!("{secret:?} {:?}", Some(secret.clone()));

    assert!(!printed.contains("s3cret-value"), "{printed}");
    assert_eq!(secret.expose(), "s3cret-value");
}
