use std::str::FromStr;

const DEFAULT_PERCENT: u32 = 10;
const MIN_DEFAULT: u16 = 25;
const MAX_DEFAULT: u16 = 50;
const MAX_SANE: u16 = 80;

/// Requested sidebar width: absolute columns or a percentage of the window width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    Columns(u16),
    Percent(u16),
}

impl FromStr for Width {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || format!("invalid width '{s}': expected N or N% (1-100)");
        match s.strip_suffix('%') {
            Some(percent) => match percent.parse::<u16>() {
                Ok(value @ 1..=100) => Ok(Width::Percent(value)),
                _ => Err(invalid()),
            },
            None => s.parse().map(Width::Columns).map_err(|_| invalid()),
        }
    }
}

impl std::fmt::Display for Width {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Width::Columns(columns) => write!(f, "{columns}"),
            Width::Percent(percent) => write!(f, "{percent}%"),
        }
    }
}

/// Resolves the sidebar width in columns for a window of `window_width` columns.
///
/// Without an explicit width it uses 10% of the window clamped to 25-50 columns.
/// An explicit width skips the clamp (minimum 1 column), and any result above
/// 80 columns falls back to the default.
pub fn resolve_width(requested: Option<Width>, window_width: u16) -> u16 {
    let default = percent_of(window_width, DEFAULT_PERCENT).clamp(MIN_DEFAULT, MAX_DEFAULT);
    let explicit = match requested {
        None => return default,
        Some(Width::Columns(columns)) => columns,
        Some(Width::Percent(percent)) => percent_of(window_width, percent.into()),
    }
    .max(1);
    if explicit > MAX_SANE {
        default
    } else {
        explicit
    }
}

fn percent_of(window_width: u16, percent: u32) -> u16 {
    (u32::from(window_width) * percent / 100) as u16
}
