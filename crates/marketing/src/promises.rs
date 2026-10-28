//! Result Promises in a promotion text (#197, ADR 0037 decision 9): a
//! promise of income, of health, an absolute guarantee or a before and
//! after, for which the ad's Platform rejects the ad and can block the
//! account. The owner is warned, never stopped; only the Pitch refuses
//! them. The patterns live here, each with its source, and match words
//! folded as the owner reads them, accents and case aside.

use std::ops::Range;

use mascate_kernel::folded_word_spans;

/// What a Result Promise promises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromiseKind {
    Income,
    Health,
    Guarantee,
    BeforeAfter,
}

/// The rule a Result Promise goes against, read on 28/10/2026.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromiseSource {
    /// <https://transparency.meta.com/policies/ad-standards/deceptive-content/unrealistic-outcomes>:
    /// getting rich quickly, earning with little or no work, guaranteed
    /// income, and weight loss shown as an unrealistic outcome.
    MetaUnrealisticOutcomes,
    /// <https://transparency.meta.com/policies/ad-standards/>: personal
    /// health and appearance, before-and-after images among them.
    MetaPersonalHealth,
    /// <https://support.google.com/adspolicy/answer/6020955>: claims that
    /// entice the user with an improbable result.
    GoogleUnreliableClaims,
    /// Código Brasileiro de Autorregulamentação Publicitária, article 27:
    /// the ad presents the product truthfully, and the guide for digital
    /// influencers extends it to affiliate posts.
    ConarTruthfulPresentation,
}

impl PromiseSource {
    pub fn name(self) -> &'static str {
        match self {
            PromiseSource::MetaUnrealisticOutcomes => {
                "Padrões de Publicidade da Meta, resultados irreais"
            }
            PromiseSource::MetaPersonalHealth => {
                "Padrões de Publicidade da Meta, saúde e aparência pessoal"
            }
            PromiseSource::GoogleUnreliableClaims => {
                "políticas do Google Ads, afirmações não confiáveis"
            }
            PromiseSource::ConarTruthfulPresentation => {
                "Código do CONAR, artigo 27 (apresentação verdadeira)"
            }
        }
    }
}

/// A phrase of a text that promises a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultPromise {
    pub kind: PromiseKind,
    /// As written in the text.
    pub phrase: String,
    /// Bytes of the text the phrase spans.
    pub at: Range<usize>,
    /// What it promises, as in "promete ganho de dinheiro".
    pub why: &'static str,
    pub source: PromiseSource,
}

impl ResultPromise {
    /// As the owner and the writer read it.
    pub fn describe(&self) -> String {
        format!(
            "“{}” {}; fonte: {}",
            self.phrase,
            self.why,
            self.source.name()
        )
    }
}

/// One word of a pattern, matched against the folded words of the text.
/// A word ending in `*` matches any word starting with the rest.
enum Token {
    One(&'static [&'static str]),
    Maybe(&'static [&'static str]),
    /// Never followed by these tokens, nor consumes them; also met at the
    /// end.
    Not(&'static [Token]),
    /// One or more words of digits, as in `5.000`.
    Number,
    /// One word of anything.
    Any,
    /// Up to this many words of anything.
    Skip(usize),
}

use Token::{Any, Maybe, Not, Number, One, Skip};

struct PromisePattern {
    kind: PromiseKind,
    tokens: &'static [Token],
    why: &'static str,
    source: PromiseSource,
}

const EARN: &[&str] = &["GANH*", "FATUR*", "LUCR*", "RENDA", "RENDIMENTO*"];
const MONEY: &[&str] = &["MIL", "K", "REAIS", "CONTO", "CONTOS"];
const PER: &[&str] = &["POR", "AO", "A", "NO", "CADA", "TODO"];
const PERIOD: &[&str] = &[
    "HORA", "HORAS", "DIA", "DIAS", "SEMANA", "SEMANAS", "MES", "MESES", "ANO", "ANOS", "DIARIO*",
    "MENSA*",
];
const LOSE_WEIGHT: &[&str] = &[
    "PERCA", "PERDER", "PERDEU", "PERDI", "PERDA", "ELIMINE", "ELIMINAR", "ELIMINOU", "ELIMINEI",
    "EMAGREC*", "SEQUE",
];
const WEIGHT: &[&str] = &["KG", "KGS", "KILO", "KILOS", "QUILO", "QUILOS", "QUILINHOS"];

/// When a product is used, as in "antes e depois do treino".
const USE_TIMES: &[&str] = &[
    "TREINO*",
    "REFEIC*",
    "ALMOCO",
    "JANTAR",
    "CAFE",
    "BANHO",
    "USAR",
    "USO",
    "COMER",
    "DORMIR",
    "LAVAR",
    "APLICAR",
    "APLICACAO",
];

const INCOME: &str = "promete ganho de dinheiro";
const WEIGHT_LOSS: &str = "promete perda de peso";

/// Every Result Promise the app warns of, with its source.
const PROMISES: &[PromisePattern] = &[
    // "ganhe 500 por dia", "fature 10 mil no mês", "lucre R$ 5.000/mês".
    PromisePattern {
        kind: PromiseKind::Income,
        tokens: &[
            One(EARN),
            Skip(4),
            Maybe(&["R"]),
            Number,
            Maybe(MONEY),
            Maybe(&["REAIS"]),
            One(PER),
            One(PERIOD),
        ],
        why: INCOME,
        source: PromiseSource::MetaUnrealisticOutcomes,
    },
    PromisePattern {
        kind: PromiseKind::Income,
        tokens: &[
            One(EARN),
            Skip(4),
            One(&["R"]),
            Number,
            Maybe(MONEY),
            Maybe(&["REAIS"]),
            Maybe(PER),
            One(PERIOD),
        ],
        why: INCOME,
        source: PromiseSource::MetaUnrealisticOutcomes,
    },
    PromisePattern {
        kind: PromiseKind::Income,
        tokens: &[
            One(EARN),
            Skip(4),
            Number,
            One(MONEY),
            Maybe(&["REAIS"]),
            Maybe(PER),
            One(PERIOD),
        ],
        why: INCOME,
        source: PromiseSource::MetaUnrealisticOutcomes,
    },
    // "renda garantida", "lucro extra garantido", "retorno garantido".
    PromisePattern {
        kind: PromiseKind::Income,
        tokens: &[
            One(&[
                "RENDA", "LUCRO", "LUCROS", "DINHEIRO", "SALARIO", "GANHO", "GANHOS", "RETORNO",
            ]),
            Maybe(&["EXTRA", "MENSAL", "FIXA", "FIXO", "CERTO", "CERTA"]),
            One(&["GARANTID*"]),
        ],
        why: "promete ganho garantido",
        source: PromiseSource::MetaUnrealisticOutcomes,
    },
    PromisePattern {
        kind: PromiseKind::Income,
        tokens: &[One(&["DINHEIRO"]), One(&["FACIL", "RAPIDO"])],
        why: "promete ganho fácil",
        source: PromiseSource::MetaUnrealisticOutcomes,
    },
    PromisePattern {
        kind: PromiseKind::Income,
        tokens: &[
            One(&["GANH*", "DINHEIRO", "RENDA"]),
            Skip(2),
            One(&["SEM"]),
            One(&["TRABALHAR", "ESFORCO", "FAZER"]),
        ],
        why: "promete ganho sem trabalho",
        source: PromiseSource::MetaUnrealisticOutcomes,
    },
    PromisePattern {
        kind: PromiseKind::Income,
        tokens: &[
            One(&["FIQUE", "FICAR", "FICA", "FICOU"]),
            One(&["RICO", "RICA", "RICOS", "MILIONARI*"]),
        ],
        why: "promete enriquecer",
        source: PromiseSource::MetaUnrealisticOutcomes,
    },
    PromisePattern {
        kind: PromiseKind::Income,
        tokens: &[One(&["ENRIQUECA", "ENRIQUECER", "ENRIQUECI"])],
        why: "promete enriquecer",
        source: PromiseSource::MetaUnrealisticOutcomes,
    },
    // "perca 10 kg", "emagreci 8 quilos", "elimine até 5kg".
    PromisePattern {
        kind: PromiseKind::Health,
        tokens: &[One(LOSE_WEIGHT), Skip(3), Number, One(WEIGHT)],
        why: WEIGHT_LOSS,
        source: PromiseSource::MetaUnrealisticOutcomes,
    },
    // "emagreça em 7 dias", "seque a barriga em 21 dias".
    PromisePattern {
        kind: PromiseKind::Health,
        tokens: &[
            One(&["EMAGREC*", "SEQUE"]),
            Skip(2),
            One(&["EM"]),
            Number,
            One(PERIOD),
        ],
        why: WEIGHT_LOSS,
        source: PromiseSource::MetaUnrealisticOutcomes,
    },
    PromisePattern {
        kind: PromiseKind::Health,
        tokens: &[One(&["CURE", "CURAR", "CURAM", "CUROU"])],
        why: "promete cura",
        source: PromiseSource::GoogleUnreliableClaims,
    },
    // "cura definitiva", "cura a ansiedade"; never "tempo de cura do verniz".
    PromisePattern {
        kind: PromiseKind::Health,
        tokens: &[
            One(&["CURA"]),
            One(&["PARA", "DEFINITIVA", "NATURAL", "GARANTIDA", "TOTAL"]),
        ],
        why: "promete cura",
        source: PromiseSource::GoogleUnreliableClaims,
    },
    PromisePattern {
        kind: PromiseKind::Health,
        tokens: &[
            One(&["CURA"]),
            One(&["A", "O", "AS", "OS", "SUA", "SEU", "SUAS", "SEUS"]),
            Any,
        ],
        why: "promete cura",
        source: PromiseSource::GoogleUnreliableClaims,
    },
    PromisePattern {
        kind: PromiseKind::Guarantee,
        tokens: &[
            Maybe(&["RESULTADO*", "SUCESSO", "APROVACAO", "EFICACIA"]),
            One(&["100"]),
            One(&["GARANTID*"]),
        ],
        why: "garante resultado",
        source: PromiseSource::ConarTruthfulPresentation,
    },
    PromisePattern {
        kind: PromiseKind::Guarantee,
        tokens: &[
            One(&["RESULTADO*", "SUCESSO", "APROVACAO", "EFICACIA"]),
            One(&["GARANTID*"]),
        ],
        why: "garante resultado",
        source: PromiseSource::ConarTruthfulPresentation,
    },
    // "garantia de resultado"; never "garantia de 7 dias" from the Platform.
    PromisePattern {
        kind: PromiseKind::Guarantee,
        tokens: &[
            One(&["GARANTIA", "GARANTIMOS", "GARANTO"]),
            Maybe(&["DE", "DO", "DOS", "O", "OS", "SEU", "SEUS"]),
            One(&["RESULTADO*", "SUCESSO", "APROVACAO", "EFICACIA"]),
        ],
        why: "garante resultado",
        source: PromiseSource::ConarTruthfulPresentation,
    },
    // "sem risco"; never "sem risco de vazar".
    PromisePattern {
        kind: PromiseKind::Guarantee,
        tokens: &[
            One(&["SEM", "ZERO"]),
            One(&["RISCO", "RISCOS"]),
            Not(&[One(&["DE", "DA", "DO"])]),
        ],
        why: "garante que não há risco",
        source: PromiseSource::ConarTruthfulPresentation,
    },
    PromisePattern {
        kind: PromiseKind::Guarantee,
        tokens: &[One(&["RISCO"]), One(&["ZERO"])],
        why: "garante que não há risco",
        source: PromiseSource::ConarTruthfulPresentation,
    },
    // "antes e depois"; never "antes e depois do treino", a time of use.
    PromisePattern {
        kind: PromiseKind::BeforeAfter,
        tokens: &[
            One(&["ANTES"]),
            Maybe(&["E", "X", "VS", "VERSUS"]),
            One(&["DEPOIS"]),
            Not(&[Maybe(&["DE", "DO", "DA", "DOS", "DAS"]), One(USE_TIMES)]),
        ],
        why: "mostra antes e depois",
        source: PromiseSource::MetaPersonalHealth,
    },
];

/// Every Result Promise in `text`, in the order they appear; a phrase is
/// told once, by the first pattern that takes it, and never runs from one
/// sentence into the next.
pub fn promises_in(text: &str) -> Vec<ResultPromise> {
    let mut promises = Vec::new();
    for sentence in sentences(text) {
        let words = words_of(&text[sentence.clone()]);
        let folded: Vec<&str> = words.iter().map(|(_, word)| word.as_str()).collect();
        let mut start = 0;
        while start < folded.len() {
            let found = PROMISES.iter().find_map(|pattern| {
                matched(pattern.tokens, &folded, start)
                    .filter(|&end| end > start)
                    .map(|end| (pattern, end))
            });
            let Some((pattern, end)) = found else {
                start += 1;
                continue;
            };
            let at = sentence.start + words[start].0.start..sentence.start + words[end - 1].0.end;
            promises.push(ResultPromise {
                kind: pattern.kind,
                phrase: text[at.clone()].to_owned(),
                at,
                why: pattern.why,
                source: pattern.source,
            });
            start = end;
        }
    }
    promises
}

/// The sentences of `text`: split at a line break, `!`, `?`, `;`, and at a
/// `.` before blank space, never inside a number as in `5.000`.
fn sentences(text: &str) -> Vec<Range<usize>> {
    let mut sentences = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let ends = match c {
            '\n' | '\r' | '!' | '?' | ';' => true,
            '.' => chars.peek().is_none_or(|(_, next)| next.is_whitespace()),
            _ => false,
        };
        if ends {
            sentences.push(start..at);
            start = at + c.len_utf8();
        }
    }
    sentences.push(start..text.len());
    sentences
}

/// The folded words of `text`, a number glued to a unit split from it, as
/// in `10kg` or `5mil`.
fn words_of(text: &str) -> Vec<(Range<usize>, String)> {
    let mut words = Vec::new();
    for (at, word) in folded_word_spans(text) {
        let digits = word.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && digits < word.len() && !word[digits..].bytes().any(|b| b.is_ascii_digit())
        {
            words.push((at.clone(), word[..digits].to_owned()));
            words.push((at, word[digits..].to_owned()));
        } else {
            words.push((at, word));
        }
    }
    words
}

/// Where `tokens` end when they match `words` from `at`.
fn matched(tokens: &[Token], words: &[&str], at: usize) -> Option<usize> {
    let Some((token, rest)) = tokens.split_first() else {
        return Some(at);
    };
    let here = words.get(at).copied();
    match token {
        One(options) => here
            .filter(|word| is_one_of(word, options))
            .and_then(|_| matched(rest, words, at + 1)),
        Maybe(options) => here
            .filter(|word| is_one_of(word, options))
            .and_then(|_| matched(rest, words, at + 1))
            .or_else(|| matched(rest, words, at)),
        Not(tokens) => {
            if matched(tokens, words, at).is_some() {
                None
            } else {
                matched(rest, words, at)
            }
        }
        Any => here.and_then(|_| matched(rest, words, at + 1)),
        Number => {
            let digits = words[at.min(words.len())..]
                .iter()
                .take_while(|word| word.bytes().all(|b| b.is_ascii_digit()))
                .count();
            (digits > 0)
                .then(|| matched(rest, words, at + digits))
                .flatten()
        }
        Skip(most) => (0..=*most)
            .take_while(|skipped| at + skipped < words.len())
            .find_map(|skipped| matched(rest, words, at + skipped)),
    }
}

fn is_one_of(word: &str, options: &[&str]) -> bool {
    options.iter().any(|option| match option.strip_suffix('*') {
        Some(prefix) => word.starts_with(prefix),
        None => word == *option,
    })
}
