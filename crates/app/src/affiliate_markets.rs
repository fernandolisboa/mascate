//! Where each affiliate Platform shows its products to affiliates, and what
//! to copy from there into Garimpo while the app does not read it (ADR
//! 0039, decision 6). One source, like the credential guides (ADR 0029):
//! the addresses are checked in the slice and in the regression pass.

use mascate_affiliate::AffiliatePlatform;

/// A Platform's affiliate market.
#[derive(Debug, Clone, Copy)]
pub struct Market {
    pub platform: AffiliatePlatform,
    /// Where the owner logs in; the steps say where the market is from
    /// there, since the market's own address needs the login.
    pub url: &'static str,
    pub steps: &'static [&'static str],
}

/// The Platforms with an affiliate market, in the Work Context's order.
pub const MARKET_PLATFORMS: [AffiliatePlatform; 4] = [
    AffiliatePlatform::Hotmart,
    AffiliatePlatform::Kiwify,
    AffiliatePlatform::Monetizze,
    AffiliatePlatform::Eduzz,
];

const COPY: &str = "Abra um produto e copie o endereço da página, o nome e os números que \
                    aparecerem: preço, comissão, temperatura ou vendas, avaliações e o produtor. \
                    Em Cadastrar à mão, cole e digite; o Mascate pontua na hora.";

/// The market of `platform`; `None` on Platforms without one, where the
/// owner promotes any product of the store.
pub fn market(platform: AffiliatePlatform) -> Option<Market> {
    let (url, steps): (&'static str, &'static [&'static str]) = match platform {
        AffiliatePlatform::Hotmart => (
            "https://app.hotmart.com",
            &[
                "Entre na Hotmart com a sua conta e abra Navegue > Mercado de afiliação, ou \
                 Produtos > Afiliar-se a um produto.",
                "Escolha a categoria e ordene pelos mais quentes ou pelos mais recentes. O \
                 filtro Afiliação imediata mostra os que não pedem aprovação do produtor.",
                COPY,
                "Para se afiliar, use Afilie-se agora ou Solicitar afiliação na página do \
                 produto; o pedido fica em Produtos > Meus produtos > Sou Afiliado(a).",
            ],
        ),
        AffiliatePlatform::Kiwify => (
            "https://dashboard.kiwify.com.br",
            &[
                "Entre na Kiwify e abra Marketplace no menu. Só aparecem produtos com pelo \
                 menos R$ 5 mil vendidos.",
                COPY,
                "Para se afiliar, peça a afiliação na página do produto; alguns aprovam na \
                 hora, outros esperam o produtor.",
            ],
        ),
        AffiliatePlatform::Monetizze => (
            "https://app.monetizze.com.br",
            &[
                "Entre na Monetizze e abra a Loja. Filtre por tipo, categoria e faturamento; \
                 o Hall da Fama mostra os mais vendidos do momento.",
                COPY,
                "Para se afiliar, peça a afiliação na página do produto e copie o Hotlink \
                 quando for aprovado.",
            ],
        ),
        AffiliatePlatform::Eduzz => (
            "https://my.eduzz.com",
            &[
                "Entre no MyEduzz e abra a Vitrine (Select) no menu.",
                COPY,
                "Para se afiliar, use Afiliar-se agora na página do produto; o link fica em \
                 Afiliações > Divulgar depois da aprovação.",
            ],
        ),
        AffiliatePlatform::Shopee
        | AffiliatePlatform::Amazon
        | AffiliatePlatform::MercadoLivre
        | AffiliatePlatform::Magalu
        | AffiliatePlatform::Other => return None,
    };
    Some(Market {
        platform,
        url,
        steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_market_platform_has_a_market_and_no_other_does() {
        for platform in AffiliatePlatform::ALL {
            assert_eq!(
                market(platform).is_some(),
                MARKET_PLATFORMS.contains(&platform),
                "{}",
                platform.name()
            );
        }
    }

    #[test]
    fn each_market_opens_a_secure_page_and_says_how_to_affiliate() {
        for platform in MARKET_PLATFORMS {
            let market = market(platform).unwrap();
            assert!(market.url.starts_with("https://"), "{}", market.url);
            assert!(market.steps.contains(&COPY), "{}", platform.name());
            assert!(
                market.steps.iter().any(|step| step.contains("afiliar")),
                "{}",
                platform.name()
            );
        }
    }
}
