//! How to get the keys of each Connection (#65): numbered steps on the
//! Platform, each with the exact page to open. One source of truth (ADR
//! 0029): the guide screen in Conexões shows them, and each phase's section
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
                     semanas; até lá, a descoberta usa o cadastro manual de ofertas, as \
                     comissões da Shopee entram à mão em Afiliação e os links curtos da Shopee \
                     esperam.",
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
        Connection::Hotmart => {
            const GUIDE: Guide = Guide {
                steps: &[
                    step_at(
                        "Entre na Hotmart com a sua conta e abra Ferramentas, Credenciais \
                     Developers.",
                        "Abrir Credenciais Developers",
                        "https://app-vlc.hotmart.com/tools/credentials",
                    ),
                    step(
                        "Clique em Criar credencial e dê um nome como Mascate. Deixe o tipo \
                     sandbox desmarcado: a credencial de sandbox só enxerga vendas de teste.",
                    ),
                    step(
                        "Confirme. A Hotmart mostra o Client ID, o Client Secret e o Basic; copie \
                     os três antes de fechar. Se perder algum, apague a credencial e crie outra.",
                    ),
                ],
                then: None,
            };
            GUIDE
        }
        Connection::Monetizze => {
            const GUIDE: Guide = Guide {
                steps: &[
                    step_at(
                        "Entre na Monetizze com a sua conta de afiliado e abra Ferramentas, API \
                     no menu.",
                        "Abrir a Monetizze",
                        "https://app.monetizze.com.br/",
                    ),
                    step(
                        "Crie uma chave de acesso e dê um nome como Mascate. Se a Monetizze \
                     pedir as permissões, marque a consulta de transações: o app só lê as \
                     vendas.",
                    ),
                    step(
                        "Copie a chave. Tenha à mão também o e-mail com que você entra na \
                     Monetizze: quando uma venda tem mais de um afiliado, é por ele que o app \
                     acha a sua comissão.",
                    ),
                ],
                then: None,
            };
            GUIDE
        }
        Connection::Kiwify => {
            const GUIDE: Guide = Guide {
                steps: &[
                    step_at(
                        "Entre na Kiwify com a sua conta e abra Apps, API no menu do painel.",
                        "Abrir o painel da Kiwify",
                        "https://dashboard.kiwify.com.br/",
                    ),
                    step(
                        "Clique em Criar API Key e dê um nome como Mascate. Se a Kiwify pedir \
                     os escopos, marque vendas e financeiro: o app só lê as vendas e os saques.",
                    ),
                    step(
                        "Copie o Client ID, o Client Secret e o Account ID que a Kiwify mostra \
                     na mesma tela. Se perder o Client Secret, apague a chave e crie outra.",
                    ),
                ],
                then: None,
            };
            GUIDE
        }
        Connection::Amazon => {
            const GUIDE: Guide = Guide {
                steps: &[
                    // Only the Professional plan reaches the Selling Partner API.
                    step(
                        "No Seller Central da Amazon.com.br, confira em Configurações › \
                     Informações da conta que a conta está no plano Profissional: o plano \
                     Individual não usa a API. Entre como usuário principal da conta, o único \
                     que autoriza apps.",
                    ),
                    step_at(
                        "Abra Apps e serviços › Desenvolver apps e preencha o perfil de \
                     desenvolvedor como desenvolvedor privado, para uso só na própria conta. \
                     Marque os papéis Product Listing, Pricing e Inventory and Order \
                     Tracking; peça também Direct-to-Consumer Shipping, o papel restrito que \
                     libera o endereço do comprador para os pedidos e passa por análise da \
                     Amazon.",
                        "Abrir Desenvolver apps",
                        "https://sellercentral.amazon.com.br/sellingpartner/developerconsole",
                    ),
                    step_at(
                        "Com o perfil aprovado, clique em Adicionar novo cliente de app, dê um \
                     nome como Mascate, escolha SP API e os mesmos papéis. Em Credenciais LWA, \
                     clique em Visualizar e copie o Client ID e o Client Secret.",
                        "Ver os papéis da API",
                        "https://developer-docs.amazon.com/sp-api/docs/roles-in-the-selling-partner-api",
                    ),
                    step_at(
                        "Na linha do app, abra Autorizar e clique em Autorizar app: a Amazon \
                     mostra o refresh token uma vez só. Copie inteiro; se perder, autorize de \
                     novo e cole o novo.",
                        "Como autorizar o próprio app",
                        "https://developer-docs.amazon.com/sp-api/docs/self-authorization",
                    ),
                    step_at(
                        "Em Configurações › Informações da conta › Seu token de comerciante, \
                     copie o ID de vendedor (o Merchant Token, como A1B2C3D4E5F6G7).",
                        "Abrir o token de comerciante",
                        "https://sellercentral.amazon.com.br/sw/AccountInfo/MerchantToken/step/MerchantToken",
                    ),
                ],
                then: Some(
                    "O Client Secret vence a cada 180 dias: a Amazon avisa antes, e o app lembra \
                 em Lembretes. Gere um novo na mesma tela de credenciais e cole aqui só o \
                 Client Secret; o resto continua valendo.",
                ),
            };
            GUIDE
        }
        Connection::ShopeeSeller => {
            const GUIDE: Guide = Guide {
                steps: &[
                    // Fiscal, never a block: the app reminds it and goes on.
                    step_at(
                        "A Shopee pede CNPJ para registrar um app de vendedor na Open Platform: o \
                     cadastro de vendedor individual não abre no Brasil. Sem CNPJ, os passos \
                     abaixo esperam, e o app lembra disso em Lembretes.",
                        "Abrir a Open Platform",
                        "https://open.shopee.com/",
                    ),
                    step_at(
                        "Entre no console da Open Platform, cadastre-se como desenvolvedor do \
                     tipo Seller In House System e crie um app para a sua loja.",
                        "Abrir o console",
                        "https://open.shopee.com/console",
                    ),
                    // The app has no address of its own yet (#10): the owner
                    // copies the one the browser lands on.
                    step(
                        "No app, em Redirect URL Domain, cadastre https://github.com/fernandolisboa: \
                     depois do login, a Shopee manda o navegador para lá com o código que você \
                     cola no Mascate.",
                    ),
                    step(
                        "Peça a publicação do app (Go Live). Aprovado, copie o Live Partner ID, um \
                     número, e a Live Partner Key. As chaves de teste só valem no sandbox da \
                     Shopee.",
                    ),
                ],
                then: Some(
                    "Depois de salvar, clique em Entrar na Shopee, autorize o app com a conta da \
                 loja e cole o endereço em que o navegador parou. A autorização da loja vale \
                 até um ano; quando vencer, entre de novo do mesmo jeito.",
                ),
            };
            GUIDE
        }
        Connection::Magalu => {
            const GUIDE: Guide = Guide {
                steps: &[
                    // Fiscal, never a block: the app reminds it and goes on.
                    step_at(
                        "A Magalu vende só por loja de pessoa jurídica: o cadastro do Parceiro \
                     Magalu pede CNPJ. Sem ele, os passos abaixo esperam, e o app lembra disso \
                     em Lembretes.",
                        "Abrir o portal de desenvolvedores",
                        "https://developers.magalu.com/docs/first-steps/create-an-application/create-application",
                    ),
                    step_at(
                        "Baixe o ID Magalu CLI para o seu sistema, renomeie o arquivo para idm e, \
                     no Windows, mova para C:\\Windows\\System32. No terminal, rode idm login, \
                     entre com a conta da loja e autorize Criar client e Atualizar client.",
                        "Baixar o idm",
                        "https://github.com/luizalabs/id-magalu-cli/releases",
                    ),
                    // The app has no address of its own yet (#10): the owner
                    // copies the one the browser lands on. The scopes are
                    // `MAGALU_SCOPES`, checked by a test.
                    step(
                        "Crie o cliente do app com: idm client create --name \"Mascate\" \
                     --description \"Gestão da loja\" --terms-of-use \
                     \"https://github.com/fernandolisboa/mascate\" --privacy-term \
                     \"https://github.com/fernandolisboa/mascate\" --redirect-uris \
                     \"https://github.com/fernandolisboa\" --audience \"https://api.magalu.com \
                     https://services.magalu.com\" --scopes \"open:portfolio-skus-seller:read \
                     open:portfolio-prices-seller:read open:portfolio-prices-seller:write \
                     open:portfolio-stocks-seller:read open:portfolio-stocks-seller:write \
                     open:order-order-seller:read open:order-delivery-seller:read \
                     open:order-financial-report-seller:read\" \
                     --access-token-exp 7200",
                    ),
                    step(
                        "Copie o Client ID e o Client Secret que o idm mostra: o Client Secret \
                     não aparece de novo; se perder, crie outro cliente.",
                    ),
                ],
                then: Some(
                    "Depois de salvar, clique em Entrar na Magalu, entre com o ID Magalu, escolha \
                 a loja, autorize o app e cole o endereço em que o navegador parou. O app \
                 renova o login sozinho; se a Magalu deixar de aceitar, entre de novo do mesmo \
                 jeito.",
                ),
            };
            GUIDE
        }
        Connection::Pinterest => {
            const GUIDE: Guide = Guide {
                steps: &[
                    step_at(
                        "A API do Pinterest pede uma conta comercial, que é de graça: se a sua \
                     for pessoal, converta em Configurações › Configurações da conta. Depois, \
                     entre no portal de desenvolvedores com ela e abra My apps.",
                        "Abrir o portal do Pinterest",
                        "https://developers.pinterest.com/apps/",
                    ),
                    step(
                        "Clique em Connect app, dê o nome Mascate, uma descrição curta e o site \
                     https://github.com/fernandolisboa/mascate, e peça o acesso Trial. O \
                     Pinterest analisa o pedido e avisa por e-mail.",
                    ),
                    // The app has no address of its own (#10): the browser
                    // stops at localhost and the owner copies the address.
                    step(
                        "Com o acesso aprovado, abra o app em My apps e, em Redirect URIs, \
                     cadastre http://localhost/ exatamente assim, com a barra no fim.",
                    ),
                    step("Copie o App ID e o App secret do app (o secret aparece em Show)."),
                ],
                then: Some(
                    "Depois de salvar, clique em Entrar no Pinterest, autorize o app e cole o \
                 endereço em que o navegador parou: http://localhost/?code=…, numa página que \
                 não abre, e é assim mesmo. O app renova o login sozinho. Com o acesso Trial, \
                 os Pins aparecem só para você; para todos verem, peça o acesso Standard em My \
                 apps, com um vídeo do app criando um Pin.",
                ),
            };
            GUIDE
        }
        Connection::Eduzz => {
            const GUIDE: Guide = Guide {
                steps: &[
                    step_at(
                        "Entre no console de desenvolvedor da Eduzz com a sua conta e abra Meus \
                     Aplicativos.",
                        "Abrir o console da Eduzz",
                        "https://console.eduzz.com/application/apps",
                    ),
                    step(
                        "Clique em Novo Aplicativo, dê um nome como Mascate e marque os escopos \
                     myeduzz_financial_read e myeduzz_sales_read: o app só lê o extrato, as \
                     vendas e as transferências. Clique em Criar Aplicativo; o Client ID e o \
                     Client Secret que aparecem não são usados.",
                    ),
                    step(
                        "Na lista de aplicativos, na linha do Mascate, use Copiar access token. \
                     É o seu token pessoal e não expira; se gerar outro ou mudar os escopos, \
                     cole o novo aqui.",
                    ),
                ],
                then: None,
            };
            GUIDE
        }
        Connection::Telegram => {
            const GUIDE: Guide = Guide {
                steps: &[
                    step_at(
                        "No Telegram, abra o BotFather, o bot oficial que cria bots.",
                        "Abrir o BotFather",
                        "https://t.me/BotFather",
                    ),
                    step(
                        "Mande /newbot, escolha um nome (como Mascate Ofertas) e um nome de \
                     usuário que termine em bot. O BotFather responde com o token, um texto \
                     como 123456789:AAH…; copie-o inteiro. Quem tem o token posta pelo bot: \
                     não mande para ninguém.",
                    ),
                    step(
                        "No seu canal, abra Administradores, Adicionar administrador, busque o \
                     bot pelo nome de usuário e deixe ligada a permissão Publicar mensagens. \
                     Repita em cada canal onde o app vai postar.",
                    ),
                ],
                then: Some(
                    "Depois de salvar, informe o @nome do canal (ou o id -100… de um canal \
                 privado) em Divulgação e use Conferir canal: o app confirma que o bot é \
                 administrador e pode publicar.",
                ),
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

    /// The phases with generated cards on the page.
    const PHASES: [u8; 3] = [1, 2, 3];

    fn credentials_page() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/credentials.html")
    }

    /// The hosts each Platform documents its setup on.
    fn official_hosts(connection: Connection) -> &'static [&'static str] {
        match connection {
            Connection::MercadoLivre => &["developers.mercadolivre.com.br"],
            Connection::Amazon => &["sellercentral.amazon.com.br", "developer-docs.amazon.com"],
            Connection::ShopeeAffiliates => &["affiliate.shopee.com.br"],
            Connection::ShopeeSeller => &["open.shopee.com"],
            Connection::Magalu => &["developers.magalu.com", "github.com/luizalabs"],
            Connection::Anthropic => &["platform.claude.com"],
            Connection::Hotmart => &["app-vlc.hotmart.com"],
            Connection::Monetizze => &["app.monetizze.com.br"],
            Connection::Kiwify => &["dashboard.kiwify.com.br"],
            Connection::Eduzz => &["console.eduzz.com"],
            Connection::Telegram => &["t.me"],
            Connection::Pinterest => &["developers.pinterest.com"],
        }
    }

    /// The phase of the plan a Connection arrived with; its card goes in
    /// that phase's section of the page.
    fn phase(connection: Connection) -> u8 {
        match connection {
            Connection::MercadoLivre | Connection::ShopeeAffiliates | Connection::Anthropic => 1,
            Connection::Hotmart
            | Connection::Monetizze
            | Connection::Kiwify
            | Connection::Eduzz
            | Connection::Telegram => 2,
            Connection::Amazon
            | Connection::ShopeeSeller
            | Connection::Magalu
            | Connection::Pinterest => 3,
        }
    }

    fn escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    /// The cards of `phase` in `docs/credentials.html`, between its markers.
    fn cards(phase: u8) -> String {
        let mut html = String::new();
        for connection in Connection::ALL
            .into_iter()
            .filter(|&connection| self::phase(connection) == phase)
        {
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

    /// The page with the cards of `phase` replaced by `cards`. Line endings
    /// are normalized: Windows checks the page out with CRLF.
    fn with_cards(page: &str, phase: u8, cards: &str) -> String {
        let page = page.replace("\r\n", "\n");
        let start_marker = format!("<!-- phase-{phase}-guides:start");
        let end_marker = format!("<!-- phase-{phase}-guides:end -->");
        let start = page
            .find(&start_marker)
            .expect("the page has the start marker");
        let body = start + page[start..].find('\n').expect("the marker ends its line") + 1;
        let end = page.find(&end_marker).expect("the page has the end marker");
        let indent = page[..end].rfind('\n').map_or(end, |line| line + 1);
        format!("{}{cards}{}", &page[..body], &page[indent..])
    }

    /// The page with every phase's cards generated again.
    fn with_every_phase(page: &str) -> String {
        PHASES.into_iter().fold(page.to_owned(), |page, phase| {
            with_cards(&page, phase, &cards(phase))
        })
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
                let hosts = official_hosts(connection);
                assert!(
                    hosts
                        .iter()
                        .any(|host| link.url.starts_with(&format!("https://{host}/"))),
                    "{} is not on {hosts:?}",
                    link.url
                );
            }
        }
    }

    #[test]
    fn the_magalu_client_asks_for_the_scopes_the_login_asks_for() {
        let command = guide(Connection::Magalu)
            .steps
            .iter()
            .find(|step| step.text.contains("idm client create"))
            .expect("a step creates the client")
            .text;
        assert!(
            command.contains(&format!(
                "--scopes \"{}\"",
                mascate_integrations::MAGALU_SCOPES
            )),
            "{command}"
        );
    }

    #[test]
    fn the_credentials_page_shows_the_same_steps_as_the_app() {
        let page = std::fs::read_to_string(credentials_page()).unwrap();
        let expected = with_every_phase(&page);
        assert!(
            page.replace("\r\n", "\n") == expected,
            "docs/credentials.html is out of date: run `cargo test -p mascate -- --ignored \
             write_credentials_page` and commit it"
        );
    }

    #[test]
    fn the_page_lists_every_pasted_credential_once() {
        let cards: String = PHASES.into_iter().map(cards).collect();
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
        std::fs::write(credentials_page(), with_every_phase(&page)).unwrap();
    }
}
