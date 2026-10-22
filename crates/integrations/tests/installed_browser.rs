//! The browser engine with the browser installed on this computer: starts
//! it without a window on a fresh profile, reads a local page's table with
//! the panels' script and closes it. Ignored by default, since CI machines
//! may lack a browser; run with `cargo test -p mascate-integrations --test
//! installed_browser -- --ignored`.

use std::sync::Arc;

use mascate_integrations::{BrowserSession, TABLES_SCRIPT, installed_browser};
use serde_json::{Value, json};

#[test]
#[ignore = "needs Edge, Chrome or Chromium installed"]
fn the_installed_browser_opens_a_page_reads_its_tables_and_closes() {
    let program = installed_browser(&|name| std::env::var(name).ok())
        .expect("Edge, Chrome or Chromium installed");
    let folder = tempfile::tempdir().unwrap();
    let page = folder.path().join("relatorio.html");
    std::fs::write(
        &page,
        r#"<!doctype html><meta charset="utf-8"><title>Relatório</title>
        <table><thead><tr><th>Data da venda</th><th>ID da venda</th><th>Comissão</th></tr></thead>
        <tbody><tr><td>18/10/2026</td><td>2000009876543210</td><td>R$ 12,50</td></tr>
        <tr><td></td><td></td><td></td></tr></tbody></table>
        <div role="table"><div role="row"><span role="columnheader">Produto</span></div>
        <div role="row"><span role="cell">Fone   de
        Ouvido</span></div></div>
        <table><tr><td>sem cabeçalho</td></tr></table>"#,
    )
    .unwrap();
    let profile = folder.path().join("perfil");
    let browser = Arc::new(BrowserSession::new(Some(program), profile.clone()));
    let url = format!("file://{}", page.display());

    let value = browser
        .read(&url, TABLES_SCRIPT, &|value: &Value| {
            value["tables"] != json!([])
        })
        .unwrap();

    assert!(value["url"].as_str().unwrap().ends_with("relatorio.html"));
    assert_eq!(
        value["tables"],
        json!([
            {
                "headers": ["Data da venda", "ID da venda", "Comissão"],
                "rows": [["18/10/2026", "2000009876543210", "R$ 12,50"]]
            },
            { "headers": ["Produto"], "rows": [["Fone de Ouvido"]] }
        ])
    );
    assert!(browser.has_profile());
    // A second reading starts the browser again: the first one closed.
    let again = browser
        .read(&url, "document.title", &|_: &Value| true)
        .unwrap();
    assert_eq!(again, json!("Relatório"));
}
