use jiff::Zoned;
use jiff::civil::Date;

use crate::i18n::Language;

const TODAY: &str = "${today";

/// Section title written in the config, with `${today}` replaced by the local date.
pub fn expand(template: &str, language: Language) -> String {
    if !template.contains(TODAY) {
        return template.to_string();
    }
    expand_on(template, Zoned::now().date(), language)
}

/// Replaces `${today}` in `template` with `date` in `language`, e.g. "Sunday, October 25" or
/// "Domingo 25 de Octubre"; `${today:en}` and `${today:es}` pick the language.
pub fn expand_on(template: &str, date: Date, language: Language) -> String {
    template
        .replace("${today:en}", &Language::En.long_date(date))
        .replace("${today:es}", &Language::Es.long_date(date))
        .replace("${today}", &language.long_date(date))
}
