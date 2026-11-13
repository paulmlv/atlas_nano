//! Terminal colors in the chosen theme. Whether a stream gets them is `anstream`'s choice, the one
//! clap makes for its help.

use std::io;

use anstream::{AutoStream, ColorChoice};
use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};

/// How a piece of text is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    /// Secondary detail: shares, dates, separators, column names.
    Dim,
    /// Section titles.
    Heading,
    /// Dollar amounts.
    Money,
    /// Bars.
    Accent,
    /// Something to notice, such as unpriced requests.
    Warning,
    Error,
    /// Good news, such as an installed client or a saved file.
    Done,
}

/// The color theme, chosen in `configure`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Bold and dim with dim bars, in the terminal's own colors.
    #[default]
    Minimal,
    /// Muted blue headings and bars, sage money.
    Soft,
    /// Amber headings and bars, money in the normal text color.
    Warm,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::Minimal, Theme::Soft, Theme::Warm];

    pub fn name(self) -> &'static str {
        match self {
            Theme::Minimal => "minimal",
            Theme::Soft => "soft",
            Theme::Warm => "warm",
        }
    }

    /// The SGR parameters for `tone`. Soft and Warm use the 256-color palette, so they look the same
    /// in every terminal; Minimal uses the terminal's own colors.
    fn code(self, tone: Tone) -> &'static str {
        match (self, tone) {
            (_, Tone::Plain) => "",
            (Theme::Minimal, Tone::Dim) => "2",
            (Theme::Minimal, Tone::Heading) => "1",
            (Theme::Minimal, Tone::Money) => "",
            // Dim rather than bright black, which some palettes, like Solarized Dark, use as the
            // background.
            (Theme::Minimal, Tone::Accent) => "2",
            (Theme::Minimal, Tone::Warning) => "33",
            (Theme::Minimal, Tone::Error) => "1;31",
            (Theme::Minimal, Tone::Done) => "1",
            (Theme::Soft, Tone::Dim) => "38;5;243",
            (Theme::Soft, Tone::Heading) => "1;38;5;110",
            (Theme::Soft, Tone::Money | Tone::Done) => "38;5;108",
            (Theme::Soft, Tone::Accent) => "38;5;67",
            (Theme::Soft, Tone::Warning) => "38;5;179",
            (Theme::Soft, Tone::Error) => "1;38;5;167",
            (Theme::Warm, Tone::Dim) => "38;5;243",
            (Theme::Warm, Tone::Heading) => "1;38;5;215",
            (Theme::Warm, Tone::Money) => "",
            (Theme::Warm, Tone::Accent) => "38;5;173",
            (Theme::Warm, Tone::Warning) => "38;5;210",
            (Theme::Warm, Tone::Error) => "1;38;5;203",
            (Theme::Warm, Tone::Done) => "38;5;215",
        }
    }
}

/// Whether to color one output stream, and in which theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paint {
    on: bool,
    theme: Theme,
}

impl Paint {
    #[cfg(test)]
    pub const OFF: Paint = Paint {
        on: false,
        theme: Theme::Minimal,
    };

    #[cfg(test)]
    pub fn colored(theme: Theme) -> Paint {
        Paint { on: true, theme }
    }

    /// Colors are on for a terminal whose `TERM` supports them, unless `NO_COLOR` is set to a
    /// non-empty value or `CLICOLOR` is `0`; a non-empty `CLICOLOR_FORCE` turns them on anywhere.
    pub fn stdout(theme: Theme) -> Paint {
        Paint {
            on: AutoStream::choice(&io::stdout()) != ColorChoice::Never,
            theme,
        }
    }

    pub fn stderr(theme: Theme) -> Paint {
        Paint {
            on: AutoStream::choice(&io::stderr()) != ColorChoice::Never,
            theme,
        }
    }

    pub fn is_on(self) -> bool {
        self.on
    }

    /// The same stream in another theme.
    pub fn with(self, theme: Theme) -> Paint {
        Paint { theme, ..self }
    }

    /// `text` in `tone`, made bold when `strong`, with its control characters shown as data.
    pub fn styled(self, tone: Tone, strong: bool, text: &str) -> String {
        let text = &printable(text);
        let code = self.code(tone, strong);
        if !self.on || code.is_empty() || text.is_empty() {
            return text.to_string();
        }
        format!("\x1b[{code}m{text}\x1b[0m")
    }

    /// The SGR parameters for `tone` in this theme, made bold when `strong`.
    fn code(self, tone: Tone, strong: bool) -> String {
        let base = self.theme.code(tone);
        if !strong || base.starts_with("1;") || base == "1" {
            base.to_string()
        } else if base.is_empty() || tone == Tone::Dim {
            // Bold and dim cancel out in most terminals, so a strong dim cell is just bold.
            "1".to_string()
        } else {
            format!("1;{base}")
        }
    }

    /// The interactive view's style for `tone`, made bold when `strong`: the colors `styled` uses,
    /// and no style when colors are off.
    pub fn style(self, tone: Tone, strong: bool) -> Style {
        if !self.on {
            return Style::new();
        }
        read_code(&self.code(tone, strong)).expect("every theme code is one the view reads")
    }

    pub fn tone(self, tone: Tone, text: &str) -> String {
        self.styled(tone, false, text)
    }

    pub fn bold(self, text: &str) -> String {
        self.styled(Tone::Plain, true, text)
    }

    pub fn dim(self, text: &str) -> String {
        self.tone(Tone::Dim, text)
    }

    /// The ` · ` between items on one line.
    pub fn dot(self) -> String {
        format!(" {} ", self.dim("·"))
    }

    /// A few report pieces in this theme, to preview it.
    pub fn sample(self) -> String {
        format!(
            "{}  {}  {}  {}",
            self.tone(Tone::Heading, "By day"),
            self.tone(Tone::Money, "$12.34"),
            self.tone(Tone::Accent, "██████████"),
            self.dim("50%")
        )
    }
}

/// Text from a transcript or folder name, with control characters, such as terminal escape codes,
/// and text-direction marks replaced so they show as data and cannot act on the terminal or reorder
/// what it shows.
pub(crate) fn printable(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || reorders(c) {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect()
}

/// Read SGR parameters into a style; None for a parameter the themes do not use.
fn read_code(code: &str) -> Option<Style> {
    let mut style = Style::new();
    let mut parts = code.split(';').filter(|part| !part.is_empty());
    while let Some(part) = parts.next() {
        style = match part {
            "1" => style.add_modifier(Modifier::BOLD),
            "2" => style.add_modifier(Modifier::DIM),
            "31" => style.fg(Color::Red),
            "33" => style.fg(Color::Yellow),
            "38" if parts.next() == Some("5") => {
                style.fg(Color::Indexed(parts.next()?.parse().ok()?))
            }
            _ => return None,
        };
    }
    Some(style)
}

/// The marks that change the direction text shows in: embeddings, overrides, and isolates.
fn reorders(c: char) -> bool {
    matches!(
        c,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

/// The width of `text` on screen, leaving out color codes.
pub fn width(text: &str) -> usize {
    let mut count = 0;
    let mut in_code = false;
    for character in text.chars() {
        if in_code {
            in_code = character != 'm';
        } else if character == '\x1b' {
            in_code = true;
        } else {
            count += 1;
        }
    }
    count
}

#[cfg(test)]
pub fn strip(text: &str) -> String {
    regex::Regex::new("\x1b\\[[0-9;]*m")
        .expect("the pattern is valid")
        .replace_all(text, "")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_leaves_text_unchanged_and_on_wraps_it_in_the_theme() {
        assert_eq!(Paint::OFF.tone(Tone::Heading, "Totals"), "Totals");
        let soft = Paint::colored(Theme::Soft);
        assert_eq!(soft.tone(Tone::Money, "$1"), "\x1b[38;5;108m$1\x1b[0m");
        assert_eq!(
            soft.styled(Tone::Money, true, "$1"),
            "\x1b[1;38;5;108m$1\x1b[0m"
        );
        assert_eq!(soft.styled(Tone::Dim, true, "4%"), "\x1b[1m4%\x1b[0m");
        // Warm shows money in the normal text color, bold on a group's row.
        let warm = Paint::colored(Theme::Warm);
        assert_eq!(warm.tone(Tone::Money, "$1"), "$1");
        assert_eq!(warm.styled(Tone::Money, true, "$1"), "\x1b[1m$1\x1b[0m");
        assert_eq!(width(&soft.styled(Tone::Money, true, "$1.00")), 5);
    }

    #[test]
    fn the_view_reads_every_theme_color() {
        let tones = [
            Tone::Plain,
            Tone::Dim,
            Tone::Heading,
            Tone::Money,
            Tone::Accent,
            Tone::Warning,
            Tone::Error,
            Tone::Done,
        ];
        for theme in Theme::ALL {
            for tone in tones {
                for strong in [false, true] {
                    let code = Paint::colored(theme).code(tone, strong);
                    assert!(read_code(&code).is_some(), "{theme:?} {tone:?}: {code}");
                }
            }
        }
        let soft = Paint::colored(Theme::Soft);
        assert_eq!(
            soft.style(Tone::Money, false),
            Style::new().fg(Color::Indexed(108))
        );
        assert_eq!(
            soft.style(Tone::Heading, false),
            Style::new()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Indexed(110))
        );
        let minimal = Paint::colored(Theme::Minimal);
        assert_eq!(
            minimal.style(Tone::Warning, false),
            Style::new().fg(Color::Yellow)
        );
        assert_eq!(minimal.style(Tone::Plain, false), Style::new());
        assert_eq!(Paint::OFF.style(Tone::Heading, true), Style::new());
    }

    #[test]
    fn shows_control_characters_as_data() {
        let soft = Paint::colored(Theme::Soft);
        assert_eq!(Paint::OFF.dim("a\u{1b}[2Jb"), "a\u{fffd}[2Jb");
        assert_eq!(Paint::OFF.dim("file\u{202e}txt.exe"), "file\u{fffd}txt.exe");
        assert_eq!(soft.tone(Tone::Plain, "a\u{1b}[2Jb"), "a\u{fffd}[2Jb");
        assert_eq!(
            soft.tone(Tone::Money, "\u{7}"),
            "\x1b[38;5;108m\u{fffd}\x1b[0m"
        );
    }
}
