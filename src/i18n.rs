//! Traducoes por chave. Os textos ficam em `locales/<idioma>.json` (um mapa
//! plano `chave -> texto`, com parametros nomeados como `{n}`), embutidos no
//! executavel. Na interface use a macro [`tr!`]:
//!
//! ```ignore
//! tr!("nav.search")                        // texto simples
//! tr!("common.n_songs", n = list.len())    // com parametros
//! ```
//!
//! Chave que faltar no idioma atual cai no pt-BR e, em ultimo caso, mostra a
//! propria chave (facil de notar). O teste `locales_have_same_keys` garante que
//! todos os idiomas tem as mesmas chaves e os mesmos parametros.
//!
//! Para adicionar um idioma: crie `locales/<codigo>.json`, acrescente uma
//! variante em [`Lang`] e o arquivo em [`Lang::source`].

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    PtBr,
    En,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::PtBr, Lang::En];

    /// Codigo BCP 47, o mesmo do nome do arquivo e das preferencias.
    pub fn code(self) -> &'static str {
        match self {
            Lang::PtBr => "pt-BR",
            Lang::En => "en",
        }
    }

    /// Nome do idioma escrito nele mesmo (para o seletor).
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::PtBr => "Português (Brasil)",
            Lang::En => "English",
        }
    }

    pub fn from_code(code: &str) -> Option<Lang> {
        Lang::ALL.into_iter().find(|l| l.code().eq_ignore_ascii_case(code))
    }

    fn source(self) -> &'static str {
        match self {
            Lang::PtBr => include_str!("../locales/pt-BR.json"),
            Lang::En => include_str!("../locales/en.json"),
        }
    }

    fn index(self) -> usize {
        self as usize
    }

    /// Idioma do Windows: portugues vira pt-BR, qualquer outro vira ingles.
    pub fn system() -> Lang {
        #[cfg(windows)]
        {
            let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
            if id & 0x3ff == 0x16 {
                return Lang::PtBr; // LANG_PORTUGUESE
            }
            Lang::En
        }
        #[cfg(not(windows))]
        {
            Lang::PtBr
        }
    }
}

type Table = HashMap<String, String>;

static CURRENT: AtomicU8 = AtomicU8::new(0);
static TABLES: [OnceLock<Table>; 2] = [OnceLock::new(), OnceLock::new()];

/// Tabela do idioma, lida do JSON embutido na primeira vez que e usada.
fn table(lang: Lang) -> &'static Table {
    TABLES[lang.index()].get_or_init(|| serde_json::from_str(lang.source()).unwrap_or_default())
}

pub fn set_lang(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::En,
        _ => Lang::PtBr,
    }
}

/// Texto da chave no idioma atual.
pub fn t(key: &str) -> &'static str {
    table(lang())
        .get(key)
        .or_else(|| table(Lang::PtBr).get(key))
        .map(String::as_str)
        .unwrap_or_else(|| {
            #[cfg(debug_assertions)]
            eprintln!("i18n: chave sem traducao: {key}");
            Box::leak(key.to_string().into_boxed_str())
        })
}

/// Texto da chave com os parametros `{nome}` substituidos.
pub fn t_args(key: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = t(key).to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

/// `tr!("chave")` ou `tr!("chave", nome = valor, ...)`.
#[macro_export]
macro_rules! tr {
    ($key:expr) => {
        $crate::i18n::t($key)
    };
    ($key:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::t_args($key, &[$((stringify!($name), &$value as &dyn std::fmt::Display)),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn params(s: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut rest = s;
        while let Some(i) = rest.find('{') {
            let Some(j) = rest[i..].find('}') else { break };
            out.insert(rest[i + 1..i + j].to_string());
            rest = &rest[i + j + 1..];
        }
        out
    }

    #[test]
    fn locales_have_same_keys() {
        let base = table(Lang::PtBr);
        assert!(!base.is_empty(), "pt-BR.json nao carregou");
        for lang in Lang::ALL {
            let t = table(lang);
            assert!(!t.is_empty(), "{}.json nao carregou (JSON invalido?)", lang.code());
            for (k, v) in base {
                let other = t.get(k).unwrap_or_else(|| panic!("{} sem a chave {k}", lang.code()));
                assert_eq!(params(v), params(other), "parametros diferentes em {k} ({})", lang.code());
            }
            for k in t.keys() {
                assert!(base.contains_key(k), "{} tem chave extra {k}", lang.code());
            }
        }
    }

    #[test]
    fn formats_args_and_falls_back() {
        set_lang(Lang::En);
        assert_eq!(tr!("common.n_songs", n = 3), "3 songs");
        set_lang(Lang::PtBr);
        assert_eq!(tr!("common.n_songs", n = 3), "3 músicas");
        assert_eq!(t("nao.existe"), "nao.existe");
    }
}
