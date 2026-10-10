//! How to get the keys of each Connection (#65): numbered steps on the
//! Platform, each with the exact page to open. One source of truth (ADR
//! 0029): the guide screen in Conexões shows them, and the phase 1 section
//! of `docs/credentials.html` is generated from them.

use mascate_integrations::Connection;

/// A page on the Platform that a step opens in the system browser.
#[derive(Debug, Clone, Copy)]
pub struct Link {
    pub label: &'static str,
    pub url: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct Step {
    pub text: &'static str,
    pub link: Option<Link>,
}

/// The steps up to having the keys copied; pasting them is the last step,
/// worded by whoever shows the guide.
#[derive(Debug, Clone, Copy)]
pub struct Guide {
    pub steps: &'static [Step],
    /// What is still missing after the keys are saved.
    pub then: Option<&'static str>,
}

const fn step(text: &'static str) -> Step {
    Step { text, link: None }
}

const fn step_at(text: &'static str, label: &'static str, url: &'static str) -> Step {
    Step {
        text,
        link: Some(Link { label, url }),
    }
}

pub fn guide(connection: Connection) -> Guide {
    match connection {
        Connection::MercadoLivre => {
            const GUIDE: Guide = Guide {
                steps: &[
                    step_at(
                        "Entre no DevCenter do Mercado Livre com a mesma conta em que você vende.",
                        "Abrir o DevCenter",
                        "https://developers.mercadolivre.com.br/devcenter",
                    ),
                    step(
                        "Crie uma aplicação nova: nome, nome curto, descrição de até 150 \
                     caracteres e um logotipo.",
                    ),
                    // The redirect URI is required and must be https; the address the
                    // Conectar button will use is decided with the seller login (#10).
                    step(
                        "Em URI de redirect, o Mercado Livre exige um endereço https. Enquanto o \
                     botão Conectar não chega, cadastre https://github.com/fernandolisboa; o \
                     app vai mostrar o endereço definitivo, e você troca na mesma aplicação.",
                    ),
                    step(
                        "Nos escopos, marque leitura, escrita e acesso offline: com o acesso \
                     offline o app renova o login sozinho. Os tópicos de notificação podem \
                     ficar vazios, porque o app busca as mudanças por conta própria.",
                    ),
                    step(
                        "Salve e abra a aplicação na lista do DevCenter. Copie o App ID, que é o \
                     Client ID, e a Secret Key, que é o Client Secret.",
                    ),
                ],
                then: Some(
                    "Depois de salvar, falta entrar na sua conta de vendedor pelo botão Conectar, \
                 que chega numa próxima versão.",
                ),
            };
            GUIDE
        }
        Connection::ShopeeAffiliates => {
            const GUIDE: Guide = Guide {
                steps: &[
                    step_at(
                        "Entre no portal de afiliados da Shopee com a sua conta de afiliado. Quem \
                     ainda não é afiliado se cadastra no mesmo portal.",
                        "Abrir o portal de afiliados",
                        "https://affiliate.shopee.com.br/",
                    ),
                    step_at(
                        "Abra a área Open API e peça acesso. A Shopee aprova à mão e pode levar \
                     semanas; até lá, a descoberta usa o cadastro manual de ofertas.",
                        "Abrir a Open API",
                        "https://affiliate.shopee.com.br/open_api",
                    ),
                    step(
                        "Com o acesso aprovado, a mesma página mostra o AppID, um número longo, e \
                     o Secret. Copie os dois. O PID de afiliado não é o AppID: se a página só \
                     mostra o PID, o acesso ainda não foi liberado.",
                    ),
                ],
                then: None,
            };
            GUIDE
        }
        Connection::Anthropic => {
            const GUIDE: Guide = Guide {
                steps: &[
                    step_at(
                        "Entre no Claude Console, o antigo console da Anthropic, ou crie uma conta.",
                        "Abrir o Claude Console",
                        "https://platform.claude.com/",
                    ),
                    step_at(
                        "Em Settings, Billing, clique em Buy credits e escolha um valor. A API \
                     cobra por uso e, sem saldo, o texto com IA falha. Os créditos valem por \
                     um ano.",
                        "Abrir Billing",
                        "https://platform.claude.com/settings/billing",
                    ),
                    step_at(
                        "Em Settings, API keys, clique em Create key, dê um nome como Mascate, \
                     escolha a validade e deixe Linked account na sua conta.",
                        "Abrir API keys",
                        "https://platform.claude.com/settings/keys",
                    ),
                    step(
                        "Copie a chave, que começa com sk-ant-. O Console só mostra a chave \
                     inteira nessa hora; se perder, crie outra.",
                    ),
                ],
                then: None,
            };
            GUIDE
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::connections::{connection_name, connection_purpose};

    const START: &str = "<!-- phase-1-guides:start";
    const END: &str = "<!-- phase-1-guides:end -->";

    fn credentials_page() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/credentials.html")
    }

    /// The hosts each Platform documents its setup on.
    fn official_host(connection: Connection) -> &'static str {
        match connection {
            Connection::MercadoLivre => "developers.mercadolivre.com.br",
            Connection::ShopeeAffiliates => "affiliate.shopee.com.br",
            Connection::Anthropic => "platform.claude.com",
        }
    }

    fn escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    /// The phase 1 cards of `docs/credentials.html`, between the markers.
    fn phase_one_cards() -> String {
        let mut html = String::new();
        for connection in Connection::ALL {
            let guide = guide(connection);
            let (pill, class) = if connection.awaits_approval_without_credentials() {
                ("pedir acesso", "pill wait")
            } else {
                ("criar agora", "pill")
            };
            let name = escape(connection_name(connection));
            writeln!(html, "    <article class=\"cred\">").unwrap();
            writeln!(
                html,
                "      <header><h3>{name}</h3><span class=\"{class}\">{pill}</span></header>"
            )
            .unwrap();
            writeln!(
                html,
                "      <p class=\"note\">{}</p>",
                escape(connection_purpose(connection))
            )
            .unwrap();
            writeln!(html, "      <ol class=\"steps\">").unwrap();
            for step in guide.steps {
                let link = step.link.map_or(String::new(), |link| {
                    format!(
                        " <a href=\"{}\">{}</a>",
                        escape(link.url),
                        escape(link.label)
                    )
                });
                writeln!(html, "        <li>{}{link}</li>", escape(step.text)).unwrap();
            }
            writeln!(
                html,
                "        <li>No app, abra Configurações → Conexões → {name} → Como conseguir as \
                 chaves, cole e clique em Salvar.</li>"
            )
            .unwrap();
            writeln!(html, "      </ol>").unwrap();
            if let Some(then) = guide.then {
                writeln!(html, "      <p>{}</p>", escape(then)).unwrap();
            }
            let variables: String = connection
                .pasted_credentials()
                .map(|credential| {
                    format!(
                        "<button class=\"var\" type=\"button\">{}</button>",
                        credential.name
                    )
                })
                .collect();
            writeln!(
                html,
                "      <dl><dt>Variáveis</dt><dd class=\"vars\">{variables}</dd></dl>"
            )
            .unwrap();
            writeln!(html, "    </article>").unwrap();
        }
        html
    }

    /// The page with its phase 1 cards replaced by `cards`. Line endings are
    /// normalized: Windows checks the page out with CRLF.
    fn with_cards(page: &str, cards: &str) -> String {
        let page = page.replace("\r\n", "\n");
        let start = page.find(START).expect("the page has the start marker");
        let body = start + page[start..].find('\n').expect("the marker ends its line") + 1;
        let end = page.find(END).expect("the page has the end marker");
        let indent = page[..end].rfind('\n').map_or(end, |line| line + 1);
        format!("{}{cards}{}", &page[..body], &page[indent..])
    }

    #[test]
    fn every_connection_has_steps_with_official_https_links() {
        for connection in Connection::ALL {
            let guide = guide(connection);
            assert!(!guide.steps.is_empty(), "{connection:?} has no steps");
            assert!(
                guide.steps.iter().any(|step| step.link.is_some()),
                "{connection:?} never says where to go"
            );
            for link in guide.steps.iter().filter_map(|step| step.link) {
                let host = official_host(connection);
                assert!(
                    link.url.starts_with(&format!("https://{host}/")),
                    "{} is not on {host}",
                    link.url
                );
            }
        }
    }

    #[test]
    fn the_credentials_page_shows_the_same_steps_as_the_app() {
        let page = std::fs::read_to_string(credentials_page()).unwrap();
        let expected = with_cards(&page, &phase_one_cards());
        assert!(
            page.replace("\r\n", "\n") == expected,
            "docs/credentials.html is out of date: run `cargo test -p mascate -- --ignored \
             write_credentials_page` and commit it"
        );
    }

    #[test]
    fn the_page_lists_every_pasted_credential_once() {
        let cards = phase_one_cards();
        for credential in Connection::ALL
            .into_iter()
            .flat_map(Connection::pasted_credentials)
        {
            let chip = format!(">{}<", credential.name);
            assert_eq!(cards.matches(&chip).count(), 1, "{}", credential.name);
        }
    }

    /// Run after changing a guide: `cargo test -p mascate -- --ignored
    /// write_credentials_page`, then commit the page.
    #[test]
    #[ignore = "rewrites docs/credentials.html"]
    fn write_credentials_page() {
        let page = std::fs::read_to_string(credentials_page()).unwrap();
        std::fs::write(credentials_page(), with_cards(&page, &phase_one_cards())).unwrap();
    }
}
