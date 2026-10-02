//! The language of a conversation, as far as a title needs to know it (MVP slice 6, plan 10
//! section 3.6).
//!
//! A title is written by a model that is only *asked* to use the language of the conversation. A
//! model that drifts (an English thread titled in Chinese) is not caught by asking again, so the
//! core does two things with what the person wrote: it names the language in the instruction
//! ([`Lang::instruction`], last, where a model weighs it most), and it checks the answer
//! ([`script_mismatch`]): a title in a script the person never wrote is not used.
//!
//! The detection is a **census of scripts** (Unicode blocks of the letters) and, for text in the
//! Latin script, a small **stop-word vote** among English, French, German, Spanish, Portuguese and
//! Italian. It is a heuristic for one purpose, and says `None` when it is not sure: short text,
//! a tie, a language it does not know. It is pure and has no I/O.
//!
//! The person's messages are the only evidence: what an agent says, or a model answers, never
//! decides the language. A mixed conversation follows the first message that says something.

use std::collections::BTreeSet;

/// A writing system, as far as the census tells them apart. Digits, punctuation, symbols and
/// emoji belong to none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Script {
    /// Latin letters, with their accents (also Vietnamese and the like).
    Latin,
    /// Han characters (Chinese, and the kanji of Japanese).
    Han,
    /// Hiragana and katakana: Japanese.
    Kana,
    /// Hangul: Korean.
    Hangul,
    /// Cyrillic letters (Russian, Ukrainian, Bulgarian, ...).
    Cyrillic,
    /// Arabic letters (Arabic, Persian, Urdu, ...).
    Arabic,
    /// Hebrew letters.
    Hebrew,
    /// Greek letters.
    Greek,
    /// Devanagari (Hindi, Marathi, ...).
    Devanagari,
    /// Thai.
    Thai,
}

impl Script {
    /// The script of the letter `c`, or `None` for a character that is not a letter of a script
    /// this census knows (and for every digit, symbol and space).
    pub fn of(c: char) -> Option<Script> {
        if !c.is_alphabetic() {
            return None;
        }
        Some(match u32::from(c) {
            0x0041..=0x024F
            | 0x1E00..=0x1EFF
            | 0x2C60..=0x2C7F
            | 0xA720..=0xA7FF
            | 0xFF21..=0xFF3A
            | 0xFF41..=0xFF5A => Script::Latin,
            0x0370..=0x03FF | 0x1F00..=0x1FFF => Script::Greek,
            0x0400..=0x052F | 0x1C80..=0x1C8F | 0x2DE0..=0x2DFF | 0xA640..=0xA69F => {
                Script::Cyrillic
            }
            0x0590..=0x05FF | 0xFB1D..=0xFB4F => Script::Hebrew,
            0x0600..=0x06FF
            | 0x0750..=0x077F
            | 0x08A0..=0x08FF
            | 0xFB50..=0xFDFF
            | 0xFE70..=0xFEFF => Script::Arabic,
            0x0900..=0x097F => Script::Devanagari,
            0x0E00..=0x0E7F => Script::Thai,
            0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF => Script::Hangul,
            0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => Script::Kana,
            0x2E80..=0x2FDF
            | 0x3005..=0x3007
            | 0x3021..=0x3029
            | 0x3038..=0x303B
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x20000..=0x323AF => Script::Han,
            _ => return None,
        })
    }
}

/// How many letters of each script `text` has.
fn census(text: &str) -> [(Script, usize); 10] {
    let mut counts = [
        (Script::Latin, 0),
        (Script::Han, 0),
        (Script::Kana, 0),
        (Script::Hangul, 0),
        (Script::Cyrillic, 0),
        (Script::Arabic, 0),
        (Script::Hebrew, 0),
        (Script::Greek, 0),
        (Script::Devanagari, 0),
        (Script::Thai, 0),
    ];
    for script in text.chars().filter_map(Script::of) {
        if let Some(slot) = counts.iter_mut().find(|(s, _)| *s == script) {
            slot.1 += 1;
        }
    }
    counts
}

/// The scripts `text` has at least one letter of.
pub fn scripts_of(text: &str) -> BTreeSet<Script> {
    census(text)
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(s, _)| s)
        .collect()
}

/// A language a title can be asked for. A closed set (ADR 0004): what the census and the vote can
/// tell, nothing more. `Cyrillic`, `Arabic` and `Devanagari` name a script, not one language,
/// because the script does not say which (Russian or Ukrainian, Arabic or Persian).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    /// English.
    English,
    /// French.
    French,
    /// German.
    German,
    /// Spanish.
    Spanish,
    /// Portuguese.
    Portuguese,
    /// Italian.
    Italian,
    /// Chinese (Han characters and no kana).
    Chinese,
    /// Japanese (kana, with or without kanji).
    Japanese,
    /// Korean.
    Korean,
    /// Some language written in Cyrillic.
    Cyrillic,
    /// Some language written in Arabic script.
    Arabic,
    /// Hebrew.
    Hebrew,
    /// Greek.
    Greek,
    /// Some language written in Devanagari.
    Devanagari,
    /// Thai.
    Thai,
}

impl Lang {
    /// The English name of the language, or of its script for a script that several languages
    /// share.
    pub const fn name(self) -> &'static str {
        match self {
            Lang::English => "English",
            Lang::French => "French",
            Lang::German => "German",
            Lang::Spanish => "Spanish",
            Lang::Portuguese => "Portuguese",
            Lang::Italian => "Italian",
            Lang::Chinese => "Chinese",
            Lang::Japanese => "Japanese",
            Lang::Korean => "Korean",
            Lang::Cyrillic => "Cyrillic script",
            Lang::Arabic => "Arabic script",
            Lang::Hebrew => "Hebrew",
            Lang::Greek => "Greek",
            Lang::Devanagari => "Devanagari script",
            Lang::Thai => "Thai",
        }
    }

    /// The script the language is written in.
    pub const fn script(self) -> Script {
        match self {
            Lang::English
            | Lang::French
            | Lang::German
            | Lang::Spanish
            | Lang::Portuguese
            | Lang::Italian => Script::Latin,
            Lang::Chinese => Script::Han,
            Lang::Japanese => Script::Kana,
            Lang::Korean => Script::Hangul,
            Lang::Cyrillic => Script::Cyrillic,
            Lang::Arabic => Script::Arabic,
            Lang::Hebrew => Script::Hebrew,
            Lang::Greek => Script::Greek,
            Lang::Devanagari => Script::Devanagari,
            Lang::Thai => Script::Thai,
        }
    }

    /// The sentence that names the language to a model: "Write the title in English." For a
    /// script that several languages share it names the script and says to keep the person's
    /// language.
    pub fn instruction(self) -> String {
        self.instruction_for("title")
    }

    /// [`instruction`](Self::instruction) for the text `what` names ("title", "description"):
    /// "Write the description in English."
    pub fn instruction_for(self, what: &str) -> String {
        match self {
            Lang::Cyrillic | Lang::Arabic | Lang::Devanagari => format!(
                "Write the {what} in the language the person wrote in, in {}.",
                self.name()
            ),
            _ => format!("Write the {what} in {}.", self.name()),
        }
    }

    /// The language a configuration names (`tasks.<task>.language`): the lower case English name
    /// of the language, or `cyrillic`, `arabic` and `devanagari` for those scripts. `None` for
    /// any other word. This is the closed set of [`Lang`].
    pub fn from_config_name(name: &str) -> Option<Lang> {
        Some(match name {
            "english" => Lang::English,
            "french" => Lang::French,
            "german" => Lang::German,
            "spanish" => Lang::Spanish,
            "portuguese" => Lang::Portuguese,
            "italian" => Lang::Italian,
            "chinese" => Lang::Chinese,
            "japanese" => Lang::Japanese,
            "korean" => Lang::Korean,
            "cyrillic" => Lang::Cyrillic,
            "arabic" => Lang::Arabic,
            "hebrew" => Lang::Hebrew,
            "greek" => Lang::Greek,
            "devanagari" => Lang::Devanagari,
            "thai" => Lang::Thai,
            _ => return None,
        })
    }
}

/// The instruction when the language is not known: still last, still explicit about whose
/// language it is.
pub const INSTRUCTION_UNKNOWN: &str = "Write the title in the language the person wrote in.";

/// [`INSTRUCTION_UNKNOWN`] for the text `what` names ("title", "description").
pub fn instruction_unknown_for(what: &str) -> String {
    format!("Write the {what} in the language the person wrote in.")
}

/// Words that mark a Latin-script language, for the vote. Only words that are not also common
/// words of the others; a word is lower case, with its accents.
const STOP_WORDS: [(Lang, &[&str]); 6] = [
    (
        Lang::English,
        &[
            "the", "and", "is", "are", "was", "to", "of", "in", "for", "with", "this", "that",
            "it", "on", "you", "please", "can", "how", "what", "my", "your", "not", "be", "have",
            "do", "does", "from", "an", "at", "i'm", "don't", "it's", "we", "should", "would",
            "could", "fix", "add", "make", "why", "when", "there", "about", "into", "just", "need",
        ],
    ),
    (
        Lang::French,
        &[
            "le", "la", "les", "des", "du", "est", "sont", "et", "un", "une", "pour", "avec",
            "dans", "que", "qui", "pas", "ce", "cette", "ces", "je", "tu", "nous", "vous", "il",
            "elle", "sur", "au", "aux", "ne", "mon", "ma", "mes", "comment", "pourquoi", "où",
            "être", "j'ai", "c'est", "d'un", "d'une", "peux", "peut", "veux", "merci", "bonjour",
            "s'il", "très", "aussi",
        ],
    ),
    (
        Lang::German,
        &[
            "der", "die", "das", "und", "ist", "nicht", "ein", "eine", "zu", "mit", "für", "auf",
            "ich", "du", "wir", "sie", "es", "den", "dem", "von", "im", "wie", "was", "warum",
            "auch", "bitte", "kann", "können", "mein", "meine", "über", "aus", "bei", "noch",
            "nur", "wird", "sind", "habe", "hallo", "danke",
        ],
    ),
    (
        Lang::Spanish,
        &[
            "el", "los", "las", "una", "es", "son", "para", "con", "en", "que", "por", "se", "mi",
            "mis", "como", "cómo", "qué", "porque", "puedes", "puedo", "quiero", "esto", "esta",
            "este", "del", "al", "pero", "muy", "también", "hola", "gracias", "tengo", "está",
            "están", "hay", "necesito",
        ],
    ),
    (
        Lang::Portuguese,
        &[
            "os", "as", "uma", "é", "são", "para", "com", "em", "que", "por", "não", "se", "meu",
            "minha", "como", "porque", "você", "posso", "quero", "isso", "esta", "este", "do",
            "da", "dos", "das", "na", "mas", "muito", "também", "olá", "obrigado", "tenho", "está",
            "estão", "preciso", "ao",
        ],
    ),
    (
        Lang::Italian,
        &[
            "il", "lo", "gli", "le", "un", "una", "è", "sono", "per", "con", "in", "che", "non",
            "mi", "mio", "mia", "come", "perché", "puoi", "posso", "voglio", "questo", "questa",
            "del", "della", "dei", "delle", "al", "ma", "molto", "anche", "ciao", "grazie", "ho",
            "ha", "devo", "bisogno",
        ],
    ),
];

/// The words of `text`, lower case, with an apostrophe kept inside a word.
fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !(c.is_alphabetic() || c == '\'' || c == '\u{2019}'))
        .filter(|w| !w.is_empty())
        .map(|w| w.replace('\u{2019}', "'").trim_matches('\'').to_lowercase())
        .filter(|w| !w.is_empty())
}

/// The Latin-script language of `text`, when one wins the vote: the language with the most
/// distinct stop words, at least two, and strictly more than the next. Words that two languages
/// share count for both, so a tie says `None`; one word alone is not a vote (the Dutch `de` is
/// German's too), so a language the lists do not know says `None` rather than a neighbour.
fn latin_language(text: &str) -> Option<Lang> {
    let seen: BTreeSet<String> = words(text).collect();
    let mut scores: Vec<(Lang, usize)> = STOP_WORDS
        .iter()
        .map(|(lang, list)| (*lang, list.iter().filter(|w| seen.contains(**w)).count()))
        .collect();
    scores.sort_by(|a, b| b.1.cmp(&a.1));
    let (best, top) = scores.first().copied()?;
    let second = scores.get(1).map_or(0, |s| s.1);
    (top >= 2 && top > second).then_some(best)
}

/// The language of one message, when it can be told.
fn language_of(text: &str) -> Option<Lang> {
    let counts = census(text);
    let count = |s: Script| counts.iter().find(|(c, _)| *c == s).map_or(0, |(_, n)| *n);
    // Japanese mixes kana with kanji; any kana at all says so (Han alone is Chinese)
    if count(Script::Kana) > 0 && count(Script::Kana) * 8 >= count(Script::Han) {
        return Some(Lang::Japanese);
    }
    // a character of a CJK script says about as much as a word of a Latin one, so it weighs more
    // when the two are mixed (a Chinese sentence that names `npm run build`)
    let weight = |script: Script, n: usize| match script {
        Script::Han | Script::Kana | Script::Hangul => n * 3,
        _ => n,
    };
    let (script, _) = counts
        .iter()
        .copied()
        .map(|(script, n)| (script, weight(script, n)))
        .max_by_key(|(_, n)| *n)
        .filter(|(_, n)| *n > 0)?;
    match script {
        Script::Latin => latin_language(text),
        Script::Han => Some(Lang::Chinese),
        Script::Kana => Some(Lang::Japanese),
        Script::Hangul => Some(Lang::Korean),
        Script::Cyrillic => Some(Lang::Cyrillic),
        Script::Arabic => Some(Lang::Arabic),
        Script::Hebrew => Some(Lang::Hebrew),
        Script::Greek => Some(Lang::Greek),
        Script::Devanagari => Some(Lang::Devanagari),
        Script::Thai => Some(Lang::Thai),
    }
}

/// The language the person writes in, from their messages in order: the language of the first
/// message that has one. `None` when no message says (too short, a tie, no letters, a language
/// this module does not know). A conversation that starts in one language and goes on in another
/// follows the first.
pub fn detect(person: &[&str]) -> Option<Lang> {
    person.iter().find_map(|text| language_of(text))
}

/// A title in a script the person did not write in, which the core declines
/// ([`script_mismatch`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptMismatch {
    /// The script of the title that none of the person's messages has.
    pub script: Script,
}

/// Whether `title` is in a script the person's messages never use: the answer a model gives when
/// it drifts into another language. `Err` names the first such script.
///
/// The Latin script is always allowed: a product name, a file name or an identifier is Latin in
/// every language ("Node.js", "render.mjs"). So is a script the person did use, and a title of
/// no letters. When the person's messages have **no letters at all** there is nothing to check
/// against, and the title is allowed.
///
/// # Errors
/// [`ScriptMismatch`] when the title has letters of a script other than Latin that no message of
/// the person has.
pub fn script_mismatch(person: &[&str], title: &str) -> Result<(), ScriptMismatch> {
    let mut written = BTreeSet::new();
    for text in person {
        written.extend(scripts_of(text));
    }
    if written.is_empty() {
        return Ok(());
    }
    // Japanese is written with kanji and kana, and kanji may stand alone in a short title
    if written.contains(&Script::Kana) {
        written.insert(Script::Han);
    }
    match scripts_of(title)
        .into_iter()
        .find(|s| *s != Script::Latin && !written.contains(s))
    {
        Some(script) => Err(ScriptMismatch { script }),
        None => Ok(()),
    }
}

/// Whether `text` is in a script that `lang` is not written in: what a fixed language of a task
/// checks the model's answer against (a deployment whose titles are always English declines a Han
/// title even in a Chinese conversation). `Err` names the first such script.
///
/// The Latin script is always allowed, as in [`script_mismatch`] (a product name is Latin in every
/// language), and so are the kanji of Japanese.
///
/// # Errors
/// [`ScriptMismatch`] when `text` has letters of a script other than Latin that `lang` does not
/// use.
pub fn script_mismatch_fixed(lang: Lang, text: &str) -> Result<(), ScriptMismatch> {
    let mut allowed = BTreeSet::from([Script::Latin, lang.script()]);
    if lang == Lang::Japanese {
        allowed.insert(Script::Han);
    }
    match scripts_of(text).into_iter().find(|s| !allowed.contains(s)) {
        Some(script) => Err(ScriptMismatch { script }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_language_checks_the_answer_against_its_own_script() {
        assert_eq!(
            script_mismatch_fixed(Lang::English, "Fix the build"),
            Ok(())
        );
        assert_eq!(
            script_mismatch_fixed(Lang::English, "修复构建"),
            Err(ScriptMismatch {
                script: Script::Han
            })
        );
        assert_eq!(
            script_mismatch_fixed(Lang::Chinese, "修复 Node.js 构建"),
            Ok(())
        );
        assert_eq!(
            script_mismatch_fixed(Lang::Japanese, "ビルドを直す 修正"),
            Ok(())
        );
        assert_eq!(
            script_mismatch_fixed(Lang::Korean, "ビルド"),
            Err(ScriptMismatch {
                script: Script::Kana
            })
        );
        assert_eq!(script_mismatch_fixed(Lang::French, "12 + 3"), Ok(()));
    }

    #[test]
    fn a_language_is_read_from_its_configuration_name() {
        for lang in [
            Lang::English,
            Lang::French,
            Lang::German,
            Lang::Spanish,
            Lang::Portuguese,
            Lang::Italian,
            Lang::Chinese,
            Lang::Japanese,
            Lang::Korean,
            Lang::Cyrillic,
            Lang::Arabic,
            Lang::Hebrew,
            Lang::Greek,
            Lang::Devanagari,
            Lang::Thai,
        ] {
            let name = lang.name().to_lowercase();
            let key = name.split(' ').next().unwrap_or_default();
            assert_eq!(Lang::from_config_name(key), Some(lang), "{key}");
        }
        assert_eq!(Lang::from_config_name("klingon"), None);
        assert_eq!(Lang::from_config_name("English"), None);
    }

    #[test]
    fn the_instruction_names_what_is_written() {
        assert_eq!(
            Lang::French.instruction_for("description"),
            "Write the description in French."
        );
        assert_eq!(Lang::French.instruction(), "Write the title in French.");
        assert!(
            Lang::Cyrillic
                .instruction_for("description")
                .starts_with("Write the description in the language the person wrote in")
        );
        assert_eq!(
            instruction_unknown_for("description"),
            "Write the description in the language the person wrote in."
        );
        assert_eq!(instruction_unknown_for("title"), INSTRUCTION_UNKNOWN);
    }

    #[test]
    fn scripts_are_told_apart() {
        assert_eq!(Script::of('a'), Some(Script::Latin));
        assert_eq!(Script::of('é'), Some(Script::Latin));
        assert_eq!(Script::of('ß'), Some(Script::Latin));
        assert_eq!(Script::of('图'), Some(Script::Han));
        assert_eq!(Script::of('ひ'), Some(Script::Kana));
        assert_eq!(Script::of('カ'), Some(Script::Kana));
        assert_eq!(Script::of('한'), Some(Script::Hangul));
        assert_eq!(Script::of('ж'), Some(Script::Cyrillic));
        assert_eq!(Script::of('ع'), Some(Script::Arabic));
        assert_eq!(Script::of('ש'), Some(Script::Hebrew));
        assert_eq!(Script::of('λ'), Some(Script::Greek));
        assert_eq!(Script::of('ह'), Some(Script::Devanagari));
        assert_eq!(Script::of('ก'), Some(Script::Thai));
        for c in ['1', ' ', '.', '-', '!', '🎉', '€', '、', '。'] {
            assert_eq!(Script::of(c), None, "{c:?}");
        }
    }

    #[test]
    fn a_message_is_in_the_language_its_letters_and_words_say() {
        let cases = [
            (
                "Please fix the login page and the redirect",
                Some(Lang::English),
            ),
            (
                "How do I export the drawing to a file?",
                Some(Lang::English),
            ),
            (
                "Peux-tu corriger la page de connexion et la redirection ?",
                Some(Lang::French),
            ),
            (
                "Wie kann ich die Zeichnung in eine Datei exportieren?",
                Some(Lang::German),
            ),
            (
                "¿Cómo puedo exportar el dibujo a un archivo?",
                Some(Lang::Spanish),
            ),
            (
                "Como posso exportar o desenho para um arquivo? Não encontro",
                Some(Lang::Portuguese),
            ),
            (
                "Come posso esportare il disegno? Non lo trovo e ho bisogno di aiuto",
                Some(Lang::Italian),
            ),
            ("请修复登录页面的重定向问题", Some(Lang::Chinese)),
            (
                "ログインページのリダイレクトを直してください",
                Some(Lang::Japanese),
            ),
            ("漢字だけでなくひらがなも", Some(Lang::Japanese)),
            ("로그인 페이지 리디렉션을 고쳐 주세요", Some(Lang::Korean)),
            (
                "Почини перенаправление на странице входа",
                Some(Lang::Cyrillic),
            ),
            ("أصلح إعادة التوجيه في صفحة الدخول", Some(Lang::Arabic)),
            ("תקן את ההפניה בדף ההתחברות", Some(Lang::Hebrew)),
            ("Διόρθωσε την ανακατεύθυνση", Some(Lang::Greek)),
            ("लॉगिन पेज का रीडायरेक्ट ठीक करो", Some(Lang::Devanagari)),
            ("แก้การเปลี่ยนเส้นทางหน้าเข้าสู่ระบบ", Some(Lang::Thai)),
        ];
        for (text, want) in cases {
            assert_eq!(language_of(text), want, "{text}");
        }
    }

    #[test]
    fn a_message_that_does_not_say_is_none() {
        for text in [
            "",
            "   ",
            "123 456",
            "🎉🎉",
            "hi",
            "ok",
            "ci",
            "npm run render",
            "[mock:title] hi",
        ] {
            assert_eq!(language_of(text), None, "{text:?}");
        }
        // a tie between two languages that share the words
        assert_eq!(language_of("para que"), None);
        // one stop word is not a vote: Dutch shares `de` with German, and the lists know no Dutch
        assert_eq!(language_of("Hoe kan ik de database exporteren?"), None);
        assert_eq!(language_of("the"), None);
    }

    #[test]
    fn code_and_names_in_a_message_do_not_change_its_language() {
        assert_eq!(
            language_of("Please run `npm run render` and tell me what the 图 looks like"),
            Some(Lang::English)
        );
        assert_eq!(
            language_of("请运行 npm run build 然后告诉我结果"),
            Some(Lang::Chinese)
        );
    }

    #[test]
    fn a_conversation_follows_the_first_message_that_says_something() {
        assert_eq!(
            detect(&["Please fix the login page", "Peux-tu corriger la page ?"]),
            Some(Lang::English)
        );
        assert_eq!(
            detect(&["请修复登录页面", "Please fix the login page"]),
            Some(Lang::Chinese)
        );
        // the first says nothing, the second does
        assert_eq!(
            detect(&["hi", "Peux-tu corriger la page de connexion ?"]),
            Some(Lang::French)
        );
        assert_eq!(detect(&["hi", "ok"]), None);
        assert_eq!(detect(&[]), None);
    }

    #[test]
    fn the_instruction_names_the_language() {
        assert_eq!(Lang::English.instruction(), "Write the title in English.");
        assert_eq!(Lang::Chinese.instruction(), "Write the title in Chinese.");
        assert_eq!(
            Lang::Cyrillic.instruction(),
            "Write the title in the language the person wrote in, in Cyrillic script."
        );
        for lang in [
            Lang::English,
            Lang::French,
            Lang::German,
            Lang::Spanish,
            Lang::Portuguese,
            Lang::Italian,
            Lang::Chinese,
            Lang::Japanese,
            Lang::Korean,
            Lang::Cyrillic,
            Lang::Arabic,
            Lang::Hebrew,
            Lang::Greek,
            Lang::Devanagari,
            Lang::Thai,
        ] {
            assert!(lang.instruction().ends_with('.'));
            assert!(lang.instruction().contains(lang.name()));
        }
    }

    #[test]
    fn a_title_in_a_script_the_person_did_not_write_is_a_mismatch() {
        let english = ["Please render the drawing with node and export it to a file"];
        assert_eq!(
            script_mismatch(&english, "Node.js 绘图导出"),
            Err(ScriptMismatch {
                script: Script::Han
            })
        );
        assert_eq!(
            script_mismatch(&english, "绘图导出"),
            Err(ScriptMismatch {
                script: Script::Han
            })
        );
        assert_eq!(
            script_mismatch(&english, "Экспорт рисунка"),
            Err(ScriptMismatch {
                script: Script::Cyrillic
            })
        );
        assert_eq!(
            script_mismatch(&english, "ログイン"),
            Err(ScriptMismatch {
                script: Script::Kana
            })
        );
        assert_eq!(script_mismatch(&english, "Export the drawing"), Ok(()));
        assert_eq!(script_mismatch(&english, "Node.js render.mjs"), Ok(()));
        assert_eq!(script_mismatch(&english, "Exportar el dibujo"), Ok(()));
        assert_eq!(script_mismatch(&english, "Café über naïve"), Ok(()));
    }

    #[test]
    fn a_title_in_the_script_the_person_wrote_is_fine() {
        let chinese = ["请把图导出成文件"];
        assert_eq!(script_mismatch(&chinese, "绘图导出"), Ok(()));
        assert_eq!(script_mismatch(&chinese, "Node.js 绘图导出"), Ok(()));
        // the latin script is always allowed: names, files, commands
        assert_eq!(script_mismatch(&chinese, "Node.js"), Ok(()));
        // a person that wrote some of another script wrote it
        let mixed = ["Please fix the 图 export"];
        assert_eq!(script_mismatch(&mixed, "修复图导出"), Ok(()));
        let russian = ["Почини вход"];
        assert_eq!(script_mismatch(&russian, "Исправить вход"), Ok(()));
        assert_eq!(
            script_mismatch(&russian, "修复登录"),
            Err(ScriptMismatch {
                script: Script::Han
            })
        );
    }

    #[test]
    fn japanese_may_stand_on_kanji_or_kana_alone() {
        let japanese = ["ログインページのリダイレクトを直してください"];
        assert_eq!(script_mismatch(&japanese, "修正"), Ok(()));
        assert_eq!(script_mismatch(&japanese, "ログイン修正"), Ok(()));
        // but Chinese does not stand on kana
        let chinese = ["请修复登录页面"];
        assert_eq!(
            script_mismatch(&chinese, "ログイン"),
            Err(ScriptMismatch {
                script: Script::Kana
            })
        );
    }

    #[test]
    fn with_nothing_written_there_is_nothing_to_check() {
        assert_eq!(script_mismatch(&[], "绘图导出"), Ok(()));
        assert_eq!(script_mismatch(&["🎉", "123"], "绘图导出"), Ok(()));
        // a title of no letters
        assert_eq!(script_mismatch(&["Fix the build"], "123 - 4"), Ok(()));
    }

    #[test]
    fn every_stop_word_is_lower_case_and_belongs_to_its_list() {
        for (lang, list) in STOP_WORDS {
            for w in list {
                assert_eq!(*w, w.to_lowercase(), "{lang:?} {w}");
                assert!(!w.is_empty());
            }
        }
    }
}
