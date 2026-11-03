//! Transcription languages (plan 5.6-1): one table for the code Whisper takes, the name the
//! screens show, the hallucination list and the summary's token estimate. The screens read it from
//! `transcribe_languages` instead of keeping their own copy. Its order is the order lists are
//! stored and shown in.

use serde::Serialize;

#[derive(Serialize, Debug)]
pub struct Lang {
    pub code: &'static str,
    pub name: &'static str,
    /// Phrases Whisper makes up on silence and noise in this language (`filter`), one per line.
    #[serde(skip)]
    pub hallucinations: &'static str,
    /// Summary-model tokens per character of transcript (plan 5.6-17): Hangul is about one each,
    /// Latin script a few characters per token.
    #[serde(skip)]
    pub tokens_per_char: f64,
}

/// Token factors: the summary model's tokenizer counted 0.69 (ko), 0.23 (en) and 0.29 (vi) tokens
/// a character over the fixtures (`tests/fixtures/README.md`); the table keeps about a third more,
/// the safe side, as one token a Korean character does. `id` is not measured yet.
pub const LANGS: [Lang; 4] = [
    Lang {
        code: "ko",
        name: "한국어",
        hallucinations: include_str!("../resources/hallucinations.ko.txt"),
        tokens_per_char: 1.0,
    },
    Lang {
        code: "en",
        name: "영어",
        hallucinations: include_str!("../resources/hallucinations.en.txt"),
        tokens_per_char: 0.3,
    },
    Lang {
        code: "vi",
        name: "베트남어",
        hallucinations: include_str!("../resources/hallucinations.vi.txt"),
        tokens_per_char: 0.4,
    },
    Lang {
        code: "id",
        name: "인도네시아어",
        hallucinations: include_str!("../resources/hallucinations.id.txt"),
        tokens_per_char: 0.3,
    },
];

pub fn get(code: &str) -> Option<&'static Lang> {
    LANGS.iter().find(|l| l.code == code)
}

/// `meta.languages` and `transcribe_languages` before anyone picks.
pub fn default_list() -> Vec<String> {
    vec!["ko".into()]
}

/// Plan 5.7-7, at every trust boundary: one or more known codes, none twice. Comes back in table
/// order, as lists are stored.
pub fn parse(languages: &[String]) -> Result<Vec<String>, String> {
    if languages.is_empty() {
        return Err("전사 언어를 하나 이상 골라 주세요".into());
    }
    for (i, code) in languages.iter().enumerate() {
        if get(code).is_none() {
            return Err(format!("알 수 없는 전사 언어입니다: {code}"));
        }
        if languages[..i].contains(code) {
            return Err(format!("전사 언어가 두 번 들어 있습니다: {code}"));
        }
    }
    Ok(LANGS
        .iter()
        .filter(|l| languages.iter().any(|c| c == l.code))
        .map(|l| l.code.to_string())
        .collect())
}

/// The screens' and the prompt's way of naming a list: "한국어·베트남어".
pub fn names(languages: &[String]) -> String {
    languages
        .iter()
        .filter_map(|c| get(c))
        .map(|l| l.name)
        .collect::<Vec<_>>()
        .join("·")
}

/// Only Korean: the 1.0 behaviour everywhere (prompt, dates, no `lang` on segments).
pub fn korean_only(languages: &[String]) -> bool {
    languages == ["ko"]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|c| c.to_string()).collect()
    }

    #[test]
    fn the_table_is_in_its_order_with_korean_names() {
        let codes: Vec<_> = LANGS.iter().map(|l| l.code).collect();
        assert_eq!(codes, ["ko", "en", "vi", "id"]);
        // Every code is one Whisper knows.
        assert!(codes.iter().all(|c| whisper_rs::get_lang_id(c).is_some()));
        assert_eq!(names(&list(&["ko", "vi"])), "한국어·베트남어");
        let json = serde_json::to_string(&LANGS[0]).unwrap();
        assert_eq!(json, r#"{"code":"ko","name":"한국어"}"#);
    }

    #[test]
    fn lists_are_checked_and_stored_in_table_order() {
        assert_eq!(parse(&list(&["vi", "ko"])).unwrap(), ["ko", "vi"]);
        assert_eq!(parse(&list(&["id", "en"])).unwrap(), ["en", "id"]);
        assert!(parse(&[]).is_err());
        assert!(parse(&list(&["ko", "ko"])).is_err());
        assert!(parse(&list(&["ja"])).is_err());
        assert!(parse(&list(&["KO"])).is_err());
        assert!(korean_only(&default_list()) && !korean_only(&list(&["ko", "en"])));
    }
}
