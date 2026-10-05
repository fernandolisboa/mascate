//! Which links open Mercado Livre itself: the app opens links from a
//! Platform's answers, and lets the owner's answers carry links, only when
//! they do.

use mascate_kernel::mercado_livre_link;

#[test]
fn only_https_pages_on_mercado_livres_domains_are_its_links() {
    for link in [
        "https://www.mercadolivre.com.br/syi/core/modify?taskId=1",
        "https://produto.mercadolivre.com.br/MLB-2",
        "https://mercadolivre.com.br",
        "https://Produto.MercadoLivre.com.br/MLB-2",
        "https://api.mercadolibre.com/items/MLB2",
    ] {
        assert!(mercado_livre_link(link), "{link}");
    }
    for link in [
        "http://produto.mercadolivre.com.br/MLB-2",
        "https://mercadolivre.com.br.golpe.com/MLB-2",
        "https://golpemercadolivre.com.br/MLB-2",
        "https://evil.com/?r=https://mercadolivre.com.br",
        "https://evil.com#mercadolivre.com.br",
        "javascript:alert(1)",
        "produto.mercadolivre.com.br/MLB-2",
    ] {
        assert!(!mercado_livre_link(link), "{link}");
    }
}
