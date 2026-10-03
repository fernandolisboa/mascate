use std::sync::Arc;

use mascate_integrations::{Connection, ConnectionError, ConnectionState, Connections};
use mascate_platform::testing::MemorySecretStore;
use mascate_platform::{Secret, SecretStore};

fn connections() -> (Connections, Arc<MemorySecretStore>) {
    let store = Arc::new(MemorySecretStore::default());
    (Connections::new(store.clone()), store)
}

#[test]
fn nothing_is_configured_on_a_new_install_and_shopee_awaits_approval() {
    let (connections, _) = connections();

    assert_eq!(
        connections.state(Connection::MercadoLivre),
        ConnectionState::NotConfigured
    );
    assert_eq!(
        connections.state(Connection::ShopeeAffiliates),
        ConnectionState::AwaitingApproval
    );
    assert_eq!(
        connections.state(Connection::Anthropic),
        ConnectionState::NotConfigured
    );
}

#[test]
fn saving_a_key_puts_it_in_the_secret_store_and_connects() {
    let (connections, store) = connections();

    connections
        .save(
            Connection::Anthropic,
            &[("ANTHROPIC_API_KEY", "sk-ant-123")],
        )
        .unwrap();

    assert_eq!(
        store.read("ANTHROPIC_API_KEY").unwrap(),
        Some(Secret::new("sk-ant-123"))
    );
    assert_eq!(
        connections.state(Connection::Anthropic),
        ConnectionState::Connected
    );
}

#[test]
fn pasted_values_are_trimmed() {
    let (connections, store) = connections();

    connections
        .save(
            Connection::ShopeeAffiliates,
            &[
                ("SHOPEE_AFFILIATE_APP_ID", "  1234 \n"),
                ("SHOPEE_AFFILIATE_SECRET", "\tabc "),
            ],
        )
        .unwrap();

    assert_eq!(
        store.read("SHOPEE_AFFILIATE_APP_ID").unwrap(),
        Some(Secret::new("1234"))
    );
    assert_eq!(
        store.read("SHOPEE_AFFILIATE_SECRET").unwrap(),
        Some(Secret::new("abc"))
    );
    assert_eq!(
        connections.state(Connection::ShopeeAffiliates),
        ConnectionState::Connected
    );
}

#[test]
fn a_blank_value_keeps_the_stored_one() {
    let (connections, store) = connections();
    let shopee = Connection::ShopeeAffiliates;
    connections
        .save(
            shopee,
            &[
                ("SHOPEE_AFFILIATE_APP_ID", "1234"),
                ("SHOPEE_AFFILIATE_SECRET", "old"),
            ],
        )
        .unwrap();

    connections
        .save(
            shopee,
            &[
                ("SHOPEE_AFFILIATE_APP_ID", "   "),
                ("SHOPEE_AFFILIATE_SECRET", "new"),
            ],
        )
        .unwrap();

    assert_eq!(
        store.read("SHOPEE_AFFILIATE_APP_ID").unwrap(),
        Some(Secret::new("1234"))
    );
    assert_eq!(
        store.read("SHOPEE_AFFILIATE_SECRET").unwrap(),
        Some(Secret::new("new"))
    );
}

#[test]
fn a_partly_filled_connection_is_not_configured_even_for_shopee() {
    let (connections, _) = connections();

    connections
        .save(
            Connection::ShopeeAffiliates,
            &[("SHOPEE_AFFILIATE_APP_ID", "1234")],
        )
        .unwrap();

    assert_eq!(
        connections.state(Connection::ShopeeAffiliates),
        ConnectionState::NotConfigured
    );
}

#[test]
fn mercado_livre_needs_the_seller_login_on_top_of_the_app_keys() {
    let (connections, store) = connections();

    connections
        .save(
            Connection::MercadoLivre,
            &[("ML_CLIENT_ID", "app"), ("ML_CLIENT_SECRET", "secret")],
        )
        .unwrap();
    assert_eq!(
        connections.state(Connection::MercadoLivre),
        ConnectionState::NotConfigured
    );

    store
        .write("ML_REFRESH_TOKEN", &Secret::new("TG-1"))
        .unwrap();
    assert_eq!(
        connections.state(Connection::MercadoLivre),
        ConnectionState::Connected
    );
}

#[test]
fn the_login_token_cannot_be_pasted() {
    let (connections, store) = connections();

    let saved = connections.save(
        Connection::MercadoLivre,
        &[("ML_CLIENT_ID", "app"), ("ML_REFRESH_TOKEN", "pasted")],
    );

    assert!(matches!(saved, Err(ConnectionError::NotPasted { .. })));
    assert!(
        store.names().is_empty(),
        "nothing is written when one name is wrong"
    );
}

#[test]
fn a_credential_of_another_connection_is_refused() {
    let (connections, store) = connections();

    let saved = connections.save(Connection::Anthropic, &[("ML_CLIENT_ID", "app")]);

    assert_eq!(
        saved,
        Err(ConnectionError::NotPasted {
            connection: Connection::Anthropic,
            name: "ML_CLIENT_ID".into(),
        })
    );
    assert!(store.names().is_empty());
}

#[test]
fn removing_a_connection_deletes_its_secrets_and_only_its_own() {
    let (connections, store) = connections();
    connections
        .save(
            Connection::MercadoLivre,
            &[("ML_CLIENT_ID", "app"), ("ML_CLIENT_SECRET", "secret")],
        )
        .unwrap();
    store
        .write("ML_REFRESH_TOKEN", &Secret::new("TG-1"))
        .unwrap();
    connections
        .save(
            Connection::Anthropic,
            &[("ANTHROPIC_API_KEY", "sk-ant-123")],
        )
        .unwrap();

    connections.remove(Connection::MercadoLivre).unwrap();

    assert_eq!(store.names(), ["ANTHROPIC_API_KEY"]);
    assert_eq!(
        connections.state(Connection::MercadoLivre),
        ConnectionState::NotConfigured
    );
    assert_eq!(
        connections.state(Connection::Anthropic),
        ConnectionState::Connected
    );
}

#[test]
fn removing_shopee_takes_it_back_to_awaiting_approval() {
    let (connections, _) = connections();
    let shopee = Connection::ShopeeAffiliates;
    connections
        .save(
            shopee,
            &[
                ("SHOPEE_AFFILIATE_APP_ID", "1234"),
                ("SHOPEE_AFFILIATE_SECRET", "abc"),
            ],
        )
        .unwrap();

    connections.remove(shopee).unwrap();

    assert_eq!(connections.state(shopee), ConnectionState::AwaitingApproval);
}

#[test]
fn removing_a_connection_that_was_never_saved_succeeds() {
    let (connections, _) = connections();

    assert_eq!(connections.remove(Connection::Anthropic), Ok(()));
}

#[test]
fn a_missing_or_locked_secret_store_shows_as_failed_and_refuses_saves() {
    let connections = Connections::new(Arc::new(MemorySecretStore::unavailable()));

    for connection in Connection::ALL {
        assert!(
            matches!(
                connections.state(connection),
                ConnectionState::Failed { .. }
            ),
            "{connection:?}"
        );
    }
    assert!(matches!(
        connections.save(Connection::Anthropic, &[("ANTHROPIC_API_KEY", "k")]),
        Err(ConnectionError::Store(_))
    ));
    assert!(matches!(
        connections.remove(Connection::Anthropic),
        Err(ConnectionError::Store(_))
    ));
}

#[test]
fn every_credential_name_is_unique_and_shopee_alone_waits_for_approval() {
    let mut names: Vec<_> = Connection::ALL
        .iter()
        .flat_map(|connection| connection.credentials())
        .map(|credential| credential.name)
        .collect();
    let count = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), count);

    let (connections, _) = connections();
    let awaiting: Vec<_> = Connection::ALL
        .into_iter()
        .filter(|&c| connections.state(c) == ConnectionState::AwaitingApproval)
        .collect();
    assert_eq!(awaiting, [Connection::ShopeeAffiliates]);
}

#[test]
fn has_credential_tells_which_keys_are_stored() {
    let (connections, _) = connections();
    connections
        .save(Connection::MercadoLivre, &[("ML_CLIENT_ID", "app")])
        .unwrap();

    assert_eq!(connections.has_credential("ML_CLIENT_ID"), Ok(true));
    assert_eq!(connections.has_credential("ML_CLIENT_SECRET"), Ok(false));
}
