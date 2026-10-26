use jiff::Zoned;
use jiff::civil::Date;

const TODAY: &str = "${today";
const SPANISH_WEEKDAYS: [&str; 7] = [
    "Domingo",
    "Lunes",
    "Martes",
    "Miércoles",
    "Jueves",
    "Viernes",
    "Sábado",
];
const SPANISH_MONTHS: [&str; 12] = [
    "Enero",
    "Febrero",
    "Marzo",
    "Abril",
    "Mayo",
    "Junio",
    "Julio",
    "Agosto",
    "Septiembre",
    "Octubre",
    "Noviembre",
    "Diciembre",
];

/// Section title written in the config, with `${today}` replaced by the local date.
pub fn expand(template: &str) -> String {
    if !template.contains(TODAY) {
        return template.to_string();
    }
    expand_on(template, Zoned::now().date())
}

/// Replaces `${today}` in `template` with `date` in English, e.g. "Sunday, October 25", and
/// `${today:es}` with `date` in Spanish, e.g. "Domingo 25 de Octubre".
pub fn expand_on(template: &str, date: Date) -> String {
    let weekday = SPANISH_WEEKDAYS[usize::from(date.weekday().to_sunday_zero_offset() as u8)];
    let month = SPANISH_MONTHS[usize::from(date.month() as u8) - 1];
    template
        .replace(
            "${today:es}",
            &format!("{weekday} {} de {month}", date.day()),
        )
        .replace("${today}", &date.strftime("%A, %B %-d").to_string())
}
