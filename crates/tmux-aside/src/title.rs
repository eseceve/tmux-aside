use jiff::Zoned;
use jiff::civil::Date;

use crate::i18n::Language;

const TODAY: &str = "${today";

/// Section title written in the config, with `${today}` replaced by the local date.
pub fn expand(template: &str, language: Language) -> String {
    expand_with(template, language, expand_on)
}

/// Like [`expand`], with the month cut to three letters, for collapsed sections.
pub fn expand_short(template: &str, language: Language) -> String {
    expand_with(template, language, expand_short_on)
}

/// Replaces `${today}` in `template` with `date` in `language`, e.g. "Sunday, October 25" or
/// "Domingo 25 de Octubre"; `${today:en}` and `${today:es}` pick the language.
pub fn expand_on(template: &str, date: Date, language: Language) -> String {
    replace(template, date, language, Language::long_date)
}

/// Like [`expand_on`], with the month cut to three letters: "Sunday, Oct 25".
pub fn expand_short_on(template: &str, date: Date, language: Language) -> String {
    replace(template, date, language, Language::short_date)
}

fn expand_with(
    template: &str,
    language: Language,
    expand: fn(&str, Date, Language) -> String,
) -> String {
    if !template.contains(TODAY) {
        return template.to_string();
    }
    expand(template, Zoned::now().date(), language)
}

fn replace(
    template: &str,
    date: Date,
    language: Language,
    format: fn(Language, Date) -> String,
) -> String {
    template
        .replace("${today:en}", &format(Language::En, date))
        .replace("${today:es}", &format(Language::Es, date))
        .replace("${today}", &format(language, date))
}
