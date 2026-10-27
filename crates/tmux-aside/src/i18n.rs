use jiff::civil::Date;
use serde::{Deserialize, Serialize};

use crate::config;

/// Environment variable the daemon sets for plugins to the language code of the sidebar.
pub const LANGUAGE_VAR: &str = "TMUX_ASIDE_LANG";

/// A language the sidebar and the bundled plugins are shown in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    Es,
}

impl Language {
    /// The code written in the config and in `$TMUX_ASIDE_LANG`.
    pub fn code(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Es => "es",
        }
    }

    /// The language of a code or locale such as `es`, `es-CL` or `es_CL.UTF-8`, or `None`
    /// when it is not supported.
    pub fn from_tag(tag: &str) -> Option<Self> {
        let code = tag.split(['_', '-', '.', '@']).next().unwrap_or(tag);
        match code.to_ascii_lowercase().as_str() {
            "en" => Some(Language::En),
            "es" => Some(Language::Es),
            _ => None,
        }
    }

    /// `$TMUX_ASIDE_LANG`, then the locale (`$LC_ALL`, `$LC_MESSAGES`, `$LANG`), then English.
    pub fn from_env() -> Self {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    /// Like [`Language::from_env`], but the `language` of the config file wins over the
    /// locale. For commands the daemon does not start, such as those run from key bindings.
    pub fn detect() -> Self {
        let configured = || {
            let path = config::default_path().ok()?;
            config::load(&path).ok()??.language
        };
        Self::from_var(std::env::var(LANGUAGE_VAR).ok())
            .or_else(configured)
            .unwrap_or_else(Self::from_env)
    }

    /// [`Language::from_env`] reading variables through `var`.
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Self {
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .filter_map(&var)
            .find(|value| !value.is_empty());
        Self::from_var(var(LANGUAGE_VAR))
            .or_else(|| locale.and_then(|locale| Self::from_tag(&locale)))
            .unwrap_or_default()
    }

    fn from_var(value: Option<String>) -> Option<Self> {
        value.and_then(|value| Self::from_tag(&value))
    }

    /// The capitalized name of the weekday of `date`.
    pub fn weekday(self, date: Date) -> &'static str {
        let names = match self {
            Language::En => [
                "Sunday",
                "Monday",
                "Tuesday",
                "Wednesday",
                "Thursday",
                "Friday",
                "Saturday",
            ],
            Language::Es => [
                "Domingo",
                "Lunes",
                "Martes",
                "Miércoles",
                "Jueves",
                "Viernes",
                "Sábado",
            ],
        };
        names[usize::from(date.weekday().to_sunday_zero_offset() as u8)]
    }

    /// The capitalized name of the month of `date`.
    pub fn month(self, date: Date) -> &'static str {
        let names = match self {
            Language::En => [
                "January",
                "February",
                "March",
                "April",
                "May",
                "June",
                "July",
                "August",
                "September",
                "October",
                "November",
                "December",
            ],
            Language::Es => [
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
            ],
        };
        names[usize::from(date.month() as u8) - 1]
    }

    /// `date` written out, e.g. "Tuesday, October 27" or "Martes 27 de Octubre".
    pub fn long_date(self, date: Date) -> String {
        let (weekday, month, day) = (self.weekday(date), self.month(date), date.day());
        match self {
            Language::En => format!("{weekday}, {month} {day}"),
            Language::Es => format!("{weekday} {day} de {month}"),
        }
    }

    /// `date` with the month cut to three letters, e.g. "Tuesday, Oct 27" or "Martes 27 de Oct".
    pub fn short_date(self, date: Date) -> String {
        let (weekday, month, day) = (self.weekday(date), &self.month(date)[..3], date.day());
        match self {
            Language::En => format!("{weekday}, {month} {day}"),
            Language::Es => format!("{weekday} {day} de {month}"),
        }
    }

    pub(crate) fn messages(self) -> &'static Messages {
        match self {
            Language::En => &EN,
            Language::Es => &ES,
        }
    }
}

/// Text the sidebar itself shows, in one language.
pub(crate) struct Messages {
    pub loading: &'static str,
    pub connecting: &'static str,
    pub add_sections: fn(&str) -> String,
    pub create: fn(&str) -> String,
    pub keys: &'static str,
    pub this_row: &'static str,
    pub move_selection: &'static str,
    pub fold_unfold: &'static str,
    pub fold_or_unfold: &'static str,
    pub run_or_show: &'static str,
    pub run_action: &'static str,
    pub close_help: &'static str,
    pub stopped: &'static str,
    pub no_hello: fn(&str) -> String,
    pub timed_out: fn(&str) -> String,
    pub exit_status: fn(i32) -> String,
    pub killed: &'static str,
    pub no_snapshot: &'static str,
    pub not_found: fn(&str) -> String,
    pub failed_to_start: fn(&str) -> String,
    pub wrong_protocol: fn(u32, u32) -> String,
    pub expected_hello: &'static str,
    pub invalid_message: fn(&str) -> String,
}

const EN: Messages = Messages {
    loading: "loading...",
    connecting: "connecting...",
    add_sections: |path| format!("add sections to {path}"),
    create: |path| format!("create {path}"),
    keys: "Keys",
    this_row: "This row",
    move_selection: "move",
    fold_unfold: "fold, unfold",
    fold_or_unfold: "fold or unfold",
    run_or_show: "run or show the row",
    run_action: "run the row's action",
    close_help: "close this help",
    stopped: "stopped",
    no_hello: |timeout| format!("no hello within {timeout}"),
    timed_out: |timeout| format!("timed out after {timeout}"),
    exit_status: |code| format!("exited with status {code}"),
    killed: "exited with a signal",
    no_snapshot: "no snapshot after hello",
    not_found: |name| format!("plugin not found: {name}"),
    failed_to_start: |path| format!("failed to start {path}"),
    wrong_protocol: |got, expected| format!("plugin speaks protocol {got}, expected {expected}"),
    expected_hello: "expected a hello as the first line",
    invalid_message: |preview| format!("invalid message: {preview}"),
};

const ES: Messages = Messages {
    loading: "cargando...",
    connecting: "conectando...",
    add_sections: |path| format!("agrega secciones a {path}"),
    create: |path| format!("crea {path}"),
    keys: "Teclas",
    this_row: "Esta fila",
    move_selection: "moverse",
    fold_unfold: "plegar, desplegar",
    fold_or_unfold: "plegar o desplegar",
    run_or_show: "ejecutar o mostrar la fila",
    run_action: "ejecutar la acción de la fila",
    close_help: "cerrar esta ayuda",
    stopped: "detenido",
    no_hello: |timeout| format!("sin hello en {timeout}"),
    timed_out: |timeout| format!("sin respuesta tras {timeout}"),
    exit_status: |code| format!("terminó con estado {code}"),
    killed: "terminado por una señal",
    no_snapshot: "sin snapshot tras el hello",
    not_found: |name| format!("plugin no encontrado: {name}"),
    failed_to_start: |path| format!("no se pudo iniciar {path}"),
    wrong_protocol: |got, expected| {
        format!("el plugin habla el protocolo {got}, se esperaba el {expected}")
    },
    expected_hello: "se esperaba un hello en la primera línea",
    invalid_message: |preview| format!("mensaje inválido: {preview}"),
};
