//! Result Promises through the Marketing module's public interface: each
//! pattern warns on an example and stays quiet on a close one, names the
//! phrase as written and why, and folds accents and case.

use mascate_marketing::{PromiseKind, PromiseSource, ResultPromise, promises_in};
use proptest::prelude::*;

/// A text that promises, the phrase the owner is shown, what it promises,
/// and a close text that promises nothing.
struct Case {
    promising: &'static str,
    phrase: &'static str,
    kind: PromiseKind,
    source: PromiseSource,
    close: &'static str,
}

const CASES: &[Case] = &[
    Case {
        promising: "Ganhe R$ 500 por dia com o método",
        phrase: "Ganhe R$ 500 por dia",
        kind: PromiseKind::Income,
        source: PromiseSource::MetaUnrealisticOutcomes,
        close: "Ganhe R$ 50 de desconto na primeira compra",
    },
    Case {
        promising: "Fature 10 mil no mês trabalhando de casa",
        phrase: "Fature 10 mil no mês",
        kind: PromiseKind::Income,
        source: PromiseSource::MetaUnrealisticOutcomes,
        close: "Fature 10 pedidos e ganhe um brinde",
    },
    Case {
        promising: "Alunos lucram R$5.000/mês",
        phrase: "lucram R$5.000/mês",
        kind: PromiseKind::Income,
        source: PromiseSource::MetaUnrealisticOutcomes,
        close: "Assine por R$ 29 por mês",
    },
    Case {
        promising: "Comece a ganhar de 300 a 500 reais por dia",
        phrase: "ganhar de 300 a 500 reais por dia",
        kind: PromiseKind::Income,
        source: PromiseSource::MetaUnrealisticOutcomes,
        close: "Ganhe 2 dias a mais de acesso",
    },
    Case {
        promising: "Renda extra garantida para quem seguir o passo a passo",
        phrase: "Renda extra garantida",
        kind: PromiseKind::Income,
        source: PromiseSource::MetaUnrealisticOutcomes,
        close: "Uma renda extra para quem gosta de cozinhar",
    },
    Case {
        promising: "Dinheiro fácil todo dia",
        phrase: "Dinheiro fácil",
        kind: PromiseKind::Income,
        source: PromiseSource::MetaUnrealisticOutcomes,
        close: "Dinheiro de volta se não gostar",
    },
    Case {
        promising: "Ganhe dinheiro sem trabalhar",
        phrase: "Ganhe dinheiro sem trabalhar",
        kind: PromiseKind::Income,
        source: PromiseSource::MetaUnrealisticOutcomes,
        close: "Ganhe tempo sem perder qualidade",
    },
    Case {
        promising: "O método para você ficar rico em um ano",
        phrase: "ficar rico",
        kind: PromiseKind::Income,
        source: PromiseSource::MetaUnrealisticOutcomes,
        close: "Um bolo rico em chocolate",
    },
    Case {
        promising: "Enriqueça com o digital",
        phrase: "Enriqueça",
        kind: PromiseKind::Income,
        source: PromiseSource::MetaUnrealisticOutcomes,
        close: "Cereal enriquecido com ferro",
    },
    Case {
        promising: "Perca 10 kg sem academia",
        phrase: "Perca 10 kg",
        kind: PromiseKind::Health,
        source: PromiseSource::MetaUnrealisticOutcomes,
        close: "Pacote de 10 kg de ração",
    },
    Case {
        promising: "Eu emagreci 8kg com este cardápio",
        phrase: "emagreci 8kg",
        kind: PromiseKind::Health,
        source: PromiseSource::MetaUnrealisticOutcomes,
        close: "Cardápio para emagrecer com acompanhamento",
    },
    Case {
        promising: "Seque a barriga em 21 dias",
        phrase: "Seque a barriga em 21 dias",
        kind: PromiseKind::Health,
        source: PromiseSource::MetaUnrealisticOutcomes,
        close: "Seque o cabelo em 5 minutos",
    },
    Case {
        promising: "O chá que cura a ansiedade",
        phrase: "cura a ansiedade",
        kind: PromiseKind::Health,
        source: PromiseSource::GoogleUnreliableClaims,
        close: "Tempo de cura do verniz: 24 horas",
    },
    Case {
        promising: "Óleo natural que cure dores nas costas",
        phrase: "cure",
        kind: PromiseKind::Health,
        source: PromiseSource::GoogleUnreliableClaims,
        close: "Curso de massagem para as costas",
    },
    Case {
        promising: "Aprovação 100% garantida",
        phrase: "Aprovação 100% garantida",
        kind: PromiseKind::Guarantee,
        source: PromiseSource::ConarTruthfulPresentation,
        close: "Aprovado por 100 alunos",
    },
    Case {
        promising: "Resultado garantido já na primeira semana",
        phrase: "Resultado garantido",
        kind: PromiseKind::Guarantee,
        source: PromiseSource::ConarTruthfulPresentation,
        close: "Garantia de 7 dias da Hotmart",
    },
    Case {
        promising: "Garantimos o resultado",
        phrase: "Garantimos o resultado",
        kind: PromiseKind::Guarantee,
        source: PromiseSource::ConarTruthfulPresentation,
        close: "Garantimos a entrega no prazo",
    },
    Case {
        promising: "Um investimento sem risco",
        phrase: "sem risco",
        kind: PromiseKind::Guarantee,
        source: PromiseSource::ConarTruthfulPresentation,
        close: "Garrafa térmica sem risco de vazar",
    },
    Case {
        promising: "Risco zero para você",
        phrase: "Risco zero",
        kind: PromiseKind::Guarantee,
        source: PromiseSource::ConarTruthfulPresentation,
        close: "Zero açúcar e zero lactose",
    },
    Case {
        promising: "Veja o antes e depois das alunas",
        phrase: "antes e depois",
        kind: PromiseKind::BeforeAfter,
        source: PromiseSource::MetaPersonalHealth,
        close: "Tome antes e depois do treino",
    },
    Case {
        promising: "ANTES X DEPOIS em 30 dias",
        phrase: "ANTES X DEPOIS",
        kind: PromiseKind::BeforeAfter,
        source: PromiseSource::MetaPersonalHealth,
        close: "Leia antes de usar e guarde depois",
    },
    Case {
        promising: "Fotos de antes e depois de 30 dias",
        phrase: "antes e depois",
        kind: PromiseKind::BeforeAfter,
        source: PromiseSource::MetaPersonalHealth,
        close: "Use antes e depois das refeições",
    },
];

#[test]
fn each_pattern_warns_on_a_promise_and_not_on_a_close_text() {
    for case in CASES {
        let promises = promises_in(case.promising);
        assert_eq!(promises.len(), 1, "{}: {promises:?}", case.promising);
        let promise = &promises[0];
        assert_eq!(promise.phrase, case.phrase, "{}", case.promising);
        assert_eq!(&case.promising[promise.at.clone()], case.phrase);
        assert_eq!(promise.kind, case.kind, "{}", case.promising);
        assert_eq!(promise.source, case.source, "{}", case.promising);

        assert_eq!(promises_in(case.close), [], "{}", case.close);
    }
}

#[test]
fn a_promise_says_the_phrase_what_it_promises_and_its_source() {
    let promises = promises_in("Ganhe R$ 500 por dia");

    assert_eq!(
        promises[0].describe(),
        "“Ganhe R$ 500 por dia” promete ganho de dinheiro; fonte: Padrões de Publicidade da \
         Meta, resultados irreais"
    );
}

#[test]
fn accents_and_case_aside_a_promise_is_the_same() {
    for text in [
        "Fature 10 mil no mês",
        "FATURE 10 MIL NO MES",
        "fature 10 mil no mes",
    ] {
        let promises = promises_in(text);
        assert_eq!(promises.len(), 1, "{text}");
        assert_eq!(promises[0].phrase, text);
    }
}

#[test]
fn every_promise_in_a_text_is_told_in_order_and_each_phrase_once() {
    let text = "Renda garantida de R$ 300 por dia! Perca 5 kg. Veja o antes e depois.";

    let promises = promises_in(text);

    let shown: Vec<(&str, PromiseKind)> = promises
        .iter()
        .map(|promise: &ResultPromise| (promise.phrase.as_str(), promise.kind))
        .collect();
    assert_eq!(
        shown,
        [
            ("Renda garantida de R$ 300 por dia", PromiseKind::Income),
            ("Perca 5 kg", PromiseKind::Health),
            ("antes e depois", PromiseKind::BeforeAfter),
        ]
    );
}

#[test]
fn an_offer_post_without_a_promise_has_none() {
    let text = "Curso de Fotografia Completo\n\
                Fotos de revista com o celular que você já tem no bolso\n\
                De R$ 199,90 por R$ 149,90 (25% off)\n\
                Garantia de 7 dias e acesso por 1 ano\n\
                https://go.hotmart.com/X12345678Y?src=telegram\n\n\
                #publi · link de afiliado";

    assert_eq!(promises_in(text), []);
}

proptest! {
    #[test]
    fn each_phrase_is_the_text_it_spans(text in "\\PC{0,80}") {
        for promise in promises_in(&text) {
            prop_assert_eq!(&text[promise.at.clone()], promise.phrase.as_str());
        }
    }

    #[test]
    fn a_promise_is_found_whatever_honest_text_surrounds_it(
        before in "[a-z ]{0,30}",
        after in "[a-z ]{0,30}",
        case in 0..CASES.len(),
    ) {
        let case = &CASES[case];
        let text = format!("{before}. {} .{after}", case.phrase);

        let promises = promises_in(&text);

        prop_assert!(
            promises.iter().any(|promise| promise.phrase == case.phrase && promise.kind == case.kind),
            "{:?}: {:?}", text, promises
        );
    }
}
