//! Tagging affiliate links through the query string, pure: each Platform's
//! parameter and domains, a link's other parameters and fragment kept as
//! they were, an old marking replaced, foreign links refused.

mod common;

use common::query_value;
use mascate_affiliate::{AffiliatePlatform, LinkTagger, TagRefusal};
use mascate_integrations::{
    AMAZON_LINKS, EDUZZ_LINKS, HOTMART_LINKS, KIWIFY_LINKS, MONETIZZE_LINKS, UrlTagger,
};
use proptest::prelude::*;

const TAGGERS: [UrlTagger; 5] = [
    HOTMART_LINKS,
    KIWIFY_LINKS,
    MONETIZZE_LINKS,
    EDUZZ_LINKS,
    AMAZON_LINKS,
];

#[test]
fn each_platform_tags_through_the_parameter_its_adapter_reads_back() {
    let tagged: Vec<(AffiliatePlatform, &str)> = TAGGERS
        .iter()
        .map(|tagger| (tagger.platform(), tagger.parameter))
        .collect();
    assert_eq!(
        tagged,
        [
            (AffiliatePlatform::Hotmart, "src"),
            (AffiliatePlatform::Kiwify, "sck"),
            (AffiliatePlatform::Monetizze, "src"),
            (AffiliatePlatform::Eduzz, "utm_content"),
            (AffiliatePlatform::Amazon, "tag"),
        ]
    );
}

#[test]
fn the_marking_is_added_after_the_links_own_parameters_and_before_its_fragment() {
    let tag = |link: &str| HOTMART_LINKS.tag(link, "telegram").unwrap();

    assert_eq!(
        tag("https://go.hotmart.com/X12345678Y"),
        "https://go.hotmart.com/X12345678Y?src=telegram"
    );
    assert_eq!(
        tag("https://go.hotmart.com/X12345678Y?"),
        "https://go.hotmart.com/X12345678Y?src=telegram"
    );
    assert_eq!(
        tag("https://go.hotmart.com/X12345678Y?ap=1a2b&utm_source=x%20y"),
        "https://go.hotmart.com/X12345678Y?ap=1a2b&utm_source=x%20y&src=telegram"
    );
    assert_eq!(
        tag("https://pay.hotmart.com/X12345678Y?off=abc#checkout?src=fragment"),
        "https://pay.hotmart.com/X12345678Y?off=abc&src=telegram#checkout?src=fragment"
    );
    assert_eq!(
        tag("  https://hotmart.com/pt-br/marketplace/produtos/curso/X12345678Y  "),
        "https://hotmart.com/pt-br/marketplace/produtos/curso/X12345678Y?src=telegram"
    );
}

#[test]
fn an_old_marking_is_replaced_in_its_place_and_repeats_dropped() {
    assert_eq!(
        HOTMART_LINKS
            .tag(
                "https://go.hotmart.com/X1?a=1&src=antigo&b=2&src=outro&sck=produtor",
                "telegram"
            )
            .unwrap(),
        "https://go.hotmart.com/X1?a=1&src=telegram&b=2&sck=produtor"
    );
    // Only the parameter itself, not one that starts like it.
    assert_eq!(
        HOTMART_LINKS
            .tag("https://go.hotmart.com/X1?srcx=1&src&xsrc=2", "telegram")
            .unwrap(),
        "https://go.hotmart.com/X1?srcx=1&src=telegram&xsrc=2"
    );
    assert_eq!(
        AMAZON_LINKS
            .tag(
                "https://www.amazon.com.br/dp/B0C1234567/ref=x?tag=outro-20&psc=1",
                "mascate-20"
            )
            .unwrap(),
        "https://www.amazon.com.br/dp/B0C1234567/ref=x?tag=mascate-20&psc=1"
    );
}

#[test]
fn links_off_the_platform_are_refused() {
    for (tagger, link) in [
        (HOTMART_LINKS, "https://produtor.example/vendas?src=x"),
        (HOTMART_LINKS, "https://nothotmart.com/X1"),
        (HOTMART_LINKS, "https://hotmart.com.evil.example/X1"),
        (KIWIFY_LINKS, "https://kiwify.com/AbC"),
        (MONETIZZE_LINKS, "https://monetizze.com/r/ABC"),
        (EDUZZ_LINKS, "https://hotmart.com/2001"),
        (AMAZON_LINKS, "https://www.amazon.com/dp/B0C1234567"),
        // A short link redirects with its own tracking id.
        (AMAZON_LINKS, "https://amzn.to/3xYzAbC"),
    ] {
        let refused = tagger.tag(link, "mascate-20");
        assert!(
            matches!(&refused, Err(TagRefusal::ForeignDomain(platform)) if *platform == tagger.platform),
            "{link}: {refused:?}"
        );
    }
}

#[test]
fn what_is_not_a_web_link_is_refused() {
    for link in [
        "",
        "go.hotmart.com/X1",
        "ftp://go.hotmart.com/X1",
        "https://",
        "https:///X1",
        "https://go.hotmart.com/X1 ?src=x",
        "https://go hotmart.com/X1",
        // The part before @ is a user, hiding the real host.
        "https://go.hotmart.com@evil.example/X1",
        "https://go.hotmart.com\\@evil.example/X1",
        "javascript:alert(1)",
    ] {
        assert_eq!(
            HOTMART_LINKS.tag(link, "telegram"),
            Err(TagRefusal::NotALink),
            "{link}"
        );
    }
}

#[test]
fn the_platforms_domains_take_any_subdomain_case_and_port() {
    for (tagger, link) in [
        (HOTMART_LINKS, "HTTPS://GO.HOTMART.COM/X1"),
        (HOTMART_LINKS, "https://hotmart.com:443/X1"),
        (KIWIFY_LINKS, "https://pay.kiwify.com.br/AbC"),
        (KIWIFY_LINKS, "https://kiwify.app/AbC"),
        (MONETIZZE_LINKS, "http://app.monetizze.com.br/r/ABC"),
        (EDUZZ_LINKS, "https://chk.eduzz.com/2001"),
        (AMAZON_LINKS, "https://amazon.com.br/dp/B0C1234567"),
    ] {
        assert!(tagger.tag(link, "mascate-20").is_ok(), "{link}");
    }
}

#[test]
fn an_amazon_tracking_id_ends_in_its_country_code() {
    let link = "https://www.amazon.com.br/dp/B0C1234567";
    for tag in ["mascate", "mascate-21", "-20"] {
        assert_eq!(
            AMAZON_LINKS.tag(link, tag),
            Err(TagRefusal::BadTag(AffiliatePlatform::Amazon)),
            "{tag}"
        );
    }
    assert!(AMAZON_LINKS.tag(link, "mascate-tg-20").is_ok());
    // Other Platforms take any marking.
    assert!(
        HOTMART_LINKS
            .tag("https://go.hotmart.com/X1", "mascate")
            .is_ok()
    );
}

fn any_tagger() -> impl Strategy<Value = UrlTagger> {
    prop::sample::select(TAGGERS.to_vec())
}

fn any_tag() -> impl Strategy<Value = String> {
    "[a-z0-9_]{1,17}".prop_map(|tag| format!("{tag}-20"))
}

/// Query pairs that never use a Platform's parameter, as written in links.
fn any_pairs() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec("[a-z]{1,6}(=[A-Za-z0-9%._~-]{0,8})?", 0..5).prop_map(|pairs| {
        pairs
            .into_iter()
            .filter(|pair| {
                let name = pair.split('=').next().unwrap();
                !["src", "sck", "tag", "utm_content"].contains(&name)
            })
            .collect()
    })
}

fn link_for(tagger: &UrlTagger, path: &str, pairs: &[String], fragment: &Option<String>) -> String {
    let mut link = format!("https://www.{}/{path}", tagger.domains[0]);
    if !pairs.is_empty() {
        link.push('?');
        link.push_str(&pairs.join("&"));
    }
    if let Some(fragment) = fragment {
        link.push('#');
        link.push_str(fragment);
    }
    link
}

proptest! {
    /// The tagged link reads back the marking under the Platform's
    /// parameter, keeps every other parameter in order and the fragment as
    /// they were, and tagging it again changes nothing.
    #[test]
    fn tagging_keeps_everything_else_and_is_idempotent(
        tagger in any_tagger(),
        tag in any_tag(),
        path in "[A-Za-z0-9/_-]{0,20}",
        pairs in any_pairs(),
        fragment in prop::option::of("[A-Za-z0-9=?&/_-]{0,12}"),
        old in prop::option::of(any_tag()),
    ) {
        let mut written = pairs.clone();
        if let Some(old) = &old {
            written.insert(written.len() / 2, format!("{}={old}", tagger.parameter));
        }
        let link = link_for(&tagger, &path, &written, &fragment);

        let tagged = tagger.tag(&link, &tag).unwrap();

        prop_assert_eq!(query_value(&tagged, tagger.parameter), Some(tag.clone()));
        let (before, after) = match tagged.split_once('#') {
            Some((before, after)) => (before, Some(after.to_owned())),
            None => (tagged.as_str(), None),
        };
        prop_assert_eq!(after, fragment);
        let kept: Vec<&str> = before
            .split_once('?')
            .map(|(_, query)| query.split('&').collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter(|pair| pair.split('=').next() != Some(tagger.parameter))
            .collect();
        prop_assert_eq!(kept, pairs.iter().map(String::as_str).collect::<Vec<_>>());
        let start = format!("https://www.{}/{}", tagger.domains[0], path);
        prop_assert!(before.starts_with(&start));
        prop_assert_eq!(tagger.tag(&tagged, &tag).unwrap(), tagged.clone());
    }
}
