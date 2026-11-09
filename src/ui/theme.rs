//! Colour schemes.
//!
//! A theme is a small set of semantic slots, not a list of every widget: `accent` is
//! whatever the interface uses to draw attention, `error` is whatever it uses to say
//! something failed. Six palettes ship with the program, and a user can drop their own
//! into `~/.config/omarchy-control-center/themes/<name>.toml`.
//!
//! Two rules shape this module:
//!
//! * the default theme uses the terminal's own palette and no explicit background, so the
//!   program respects whatever colours the user already chose for their terminal;
//! * a broken user theme is reported with the exact field that is wrong and never stops the
//!   program from starting — it falls back to the default and says so.

use std::path::{Path, PathBuf};

use ratatui::style::Color;
use serde::Deserialize;

/// The colours the interface actually distinguishes between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    /// `None` means "leave the terminal background alone".
    pub background: Option<Color>,
    pub foreground: Option<Color>,
    /// Secondary text: labels, hints, paths.
    pub muted: Color,
    /// What the interface uses to draw attention: titles, the selected pane, the caret.
    pub accent: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    /// Background of the highlighted row. `None` falls back to reverse video, which works
    /// with any terminal palette.
    pub selection: Option<Color>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// Stable identifier: what a configuration file writes, and what the selector shows.
    pub id: String,
    pub name: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub palette: Palette,
    /// Whether this came from the user's own directory.
    pub user_defined: bool,
}

impl Theme {
    /// Whether the theme paints its own background, which is what decides if the screen is
    /// cleared with a solid colour.
    pub fn paints_background(&self) -> bool {
        self.palette.background.is_some()
    }

    /// The base style for the whole frame.
    pub fn base_style(&self) -> ratatui::style::Style {
        let mut style = ratatui::style::Style::new();
        if let Some(background) = self.palette.background {
            style = style.bg(background);
        }
        if let Some(foreground) = self.palette.foreground {
            style = style.fg(foreground);
        }
        style
    }
}

fn rgb(red: u8, green: u8, blue: u8) -> Color {
    Color::Rgb(red, green, blue)
}

/// The default: no background, terminal colours for semantics. This is the theme the
/// program had before themes existed, kept as the default so nothing changes for someone
/// who never opens the selector.
pub fn default_theme() -> Theme {
    Theme {
        id: "default".to_string(),
        name: "Terminal default".to_string(),
        author: None,
        description: Some(
            "Uses the terminal's own palette and leaves its background alone".to_string(),
        ),
        palette: Palette {
            background: None,
            foreground: None,
            muted: Color::DarkGray,
            accent: Color::Cyan,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            selection: None,
        },
        user_defined: false,
    }
}

/// Every theme that ships with the program, default first.
pub fn builtin_themes() -> Vec<Theme> {
    vec![
        default_theme(),
        Theme {
            id: "zelda".to_string(),
            name: "The Legend of Zelda".to_string(),
            author: Some("Omarchy Control Center".to_string()),
            description: Some(
                "Hyrule at dusk: parchment, forest green and the gold of a Triforce".to_string(),
            ),
            palette: Palette {
                background: Some(rgb(0x11, 0x16, 0x12)),
                foreground: Some(rgb(0xe8, 0xe2, 0xc8)),
                muted: rgb(0x7a, 0x8a, 0x6f),
                accent: rgb(0xe0, 0xb8, 0x3a),
                success: rgb(0x8f, 0xc9, 0x5a),
                warning: rgb(0xd8, 0x9a, 0x3a),
                error: rgb(0xc0, 0x4a, 0x3e),
                selection: Some(rgb(0x25, 0x30, 0x22)),
            },
            user_defined: false,
        },
        Theme {
            id: "cyberpunk".to_string(),
            name: "Cyberpunk".to_string(),
            author: Some("Omarchy Control Center".to_string()),
            description: Some(
                "Night City signage: hot yellow on deep purple, cyan and magenta".to_string(),
            ),
            palette: Palette {
                background: Some(rgb(0x0d, 0x02, 0x21)),
                foreground: Some(rgb(0xe8, 0xe6, 0xff)),
                muted: rgb(0x6b, 0x5f, 0x9c),
                accent: rgb(0xfc, 0xee, 0x0a),
                success: rgb(0x05, 0xd9, 0xf4),
                warning: rgb(0xff, 0x9f, 0x1c),
                error: rgb(0xff, 0x2a, 0x6d),
                selection: Some(rgb(0x2a, 0x10, 0x4a)),
            },
            user_defined: false,
        },
        Theme {
            id: "rei".to_string(),
            name: "Rei Ayanami".to_string(),
            author: Some("Omarchy Control Center".to_string()),
            description: Some(
                "Evangelion Unit-00: pale blue, hospital white and a red that is not warm"
                    .to_string(),
            ),
            palette: Palette {
                background: Some(rgb(0x0e, 0x14, 0x1a)),
                foreground: Some(rgb(0xe6, 0xef, 0xf5)),
                muted: rgb(0x5d, 0x74, 0x85),
                accent: rgb(0x8f, 0xc3, 0xdb),
                success: rgb(0x9f, 0xd8, 0xc8),
                warning: rgb(0xe3, 0xc0, 0x8a),
                error: rgb(0xd0, 0x4b, 0x5a),
                selection: Some(rgb(0x1b, 0x2a, 0x36)),
            },
            user_defined: false,
        },
        Theme {
            id: "monokai".to_string(),
            name: "Monokai".to_string(),
            author: Some("Omarchy Control Center".to_string()),
            description: Some("The editor palette everyone recognises".to_string()),
            palette: Palette {
                background: Some(rgb(0x27, 0x28, 0x22)),
                foreground: Some(rgb(0xf8, 0xf8, 0xf2)),
                muted: rgb(0x75, 0x71, 0x5e),
                accent: rgb(0x66, 0xd9, 0xef),
                success: rgb(0xa6, 0xe2, 0x2e),
                warning: rgb(0xe6, 0xdb, 0x74),
                error: rgb(0xf9, 0x26, 0x72),
                selection: Some(rgb(0x3e, 0x3d, 0x32)),
            },
            user_defined: false,
        },
        Theme {
            id: "matrix".to_string(),
            name: "Matrix".to_string(),
            author: Some("Omarchy Control Center".to_string()),
            description: Some("Green phosphor on black, and nothing else".to_string()),
            palette: Palette {
                background: Some(rgb(0x00, 0x00, 0x00)),
                foreground: Some(rgb(0xb8, 0xff, 0xb8)),
                muted: rgb(0x2f, 0x6b, 0x2f),
                accent: rgb(0x00, 0xff, 0x41),
                success: rgb(0x7d, 0xff, 0x7d),
                warning: rgb(0xc8, 0xff, 0x4f),
                error: rgb(0xff, 0x4f, 0x4f),
                selection: Some(rgb(0x0d, 0x33, 0x0d)),
            },
            user_defined: false,
        },
        Theme {
            id: "cviper".to_string(),
            name: "C. Viper".to_string(),
            author: Some("Omarchy Control Center".to_string()),
            description: Some(
                "Street Fighter 6: charcoal suit, white shirt and an orange that hits".to_string(),
            ),
            palette: Palette {
                background: Some(rgb(0x14, 0x10, 0x0e)),
                foreground: Some(rgb(0xec, 0xe7, 0xe1)),
                muted: rgb(0x6f, 0x62, 0x5a),
                accent: rgb(0xff, 0x6f, 0x3c),
                success: rgb(0x4f, 0xc3, 0x9a),
                warning: rgb(0xff, 0xc2, 0x4a),
                error: rgb(0xe2, 0x3d, 0x3d),
                selection: Some(rgb(0x2c, 0x1e, 0x18)),
            },
            user_defined: false,
        },
    ]
}

/// A theme file on disk.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    name: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    description: Option<String>,
    colors: ThemeColors,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeColors {
    #[serde(default)]
    background: Option<String>,
    #[serde(default)]
    foreground: Option<String>,
    muted: String,
    accent: String,
    success: String,
    warning: String,
    error: String,
    #[serde(default)]
    selection: Option<String>,
}

/// Why a theme file was rejected. Every variant names the file and the field, because a
/// theme that silently does nothing is worse than one that says what is wrong.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ThemeError {
    #[error("{path}: {message}")]
    Read { path: PathBuf, message: String },
    #[error("{path}: {detail}")]
    Parse { path: PathBuf, detail: String },
    #[error("{path}: 'colors.{field}' is not a colour: {value:?} (expected #rrggbb)")]
    BadColor {
        path: PathBuf,
        field: String,
        value: String,
    },
}

/// Reads one theme file. The id is the file stem, so the file name is the identifier a
/// configuration writes.
pub fn load_theme_file(path: &Path) -> Result<Theme, ThemeError> {
    let text = std::fs::read_to_string(path).map_err(|error| ThemeError::Read {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let file: ThemeFile = toml::from_str(&text).map_err(|error| ThemeError::Parse {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;

    let field = |name: &str, value: &str| -> Result<Color, ThemeError> {
        parse_hex(value).ok_or_else(|| ThemeError::BadColor {
            path: path.to_path_buf(),
            field: name.to_string(),
            value: value.to_string(),
        })
    };
    let optional = |name: &str, value: &Option<String>| -> Result<Option<Color>, ThemeError> {
        match value {
            Some(value) => field(name, value).map(Some),
            None => Ok(None),
        }
    };

    let colors = &file.colors;
    let palette = Palette {
        background: optional("background", &colors.background)?,
        foreground: optional("foreground", &colors.foreground)?,
        muted: field("muted", &colors.muted)?,
        accent: field("accent", &colors.accent)?,
        success: field("success", &colors.success)?,
        warning: field("warning", &colors.warning)?,
        error: field("error", &colors.error)?,
        selection: optional("selection", &colors.selection)?,
    };

    let id = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unnamed".to_string());

    Ok(Theme {
        id,
        name: file.name,
        author: file.author,
        description: file.description,
        palette,
        user_defined: true,
    })
}

/// `#rrggbb` or `#rgb`.
pub fn parse_hex(value: &str) -> Option<Color> {
    let hex = value.trim().strip_prefix('#')?;
    let (red, green, blue) = match hex.len() {
        6 => (
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
        ),
        3 => {
            let digit = |index: usize| {
                let value = u8::from_str_radix(&hex[index..index + 1], 16).ok()?;
                Some(value * 17)
            };
            (digit(0)?, digit(1)?, digit(2)?)
        }
        _ => return None,
    };
    Some(Color::Rgb(red, green, blue))
}

/// What the selector needs: the themes that exist and the problems found on disk.
#[derive(Debug, Clone, Default)]
pub struct ThemeCatalogue {
    pub themes: Vec<Theme>,
    pub problems: Vec<ThemeError>,
}

impl ThemeCatalogue {
    pub fn find(&self, id: &str) -> Option<&Theme> {
        self.themes.iter().find(|theme| theme.id == id)
    }
}

/// The directory a user drops their own themes into.
pub fn user_theme_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("themes")
}

/// Built-in themes plus every valid file in the user's theme directory. A file that fails to
/// load becomes a reported problem; it never removes the built-ins or stops the program.
pub fn catalogue(config_dir: &Path) -> ThemeCatalogue {
    let mut catalogue = ThemeCatalogue {
        themes: builtin_themes(),
        problems: Vec::new(),
    };
    let directory = user_theme_dir(config_dir);

    let Ok(entries) = std::fs::read_dir(&directory) else {
        return catalogue;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("toml"))
        .collect();
    files.sort();

    for file in files {
        match load_theme_file(&file) {
            Ok(theme) => {
                // A user theme may not shadow a built-in: the selector would show two rows
                // with the same identifier and the configuration would be ambiguous.
                if catalogue.find(&theme.id).is_some() {
                    catalogue.problems.push(ThemeError::Parse {
                        path: file.clone(),
                        detail: format!("the id '{}' is already taken; rename the file", theme.id),
                    });
                    continue;
                }
                catalogue.themes.push(theme);
            }
            Err(error) => catalogue.problems.push(error),
        }
    }
    catalogue
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("occ-theme-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("themes")).expect("themes dir");
        dir
    }

    #[test]
    fn the_six_requested_themes_exist_with_distinct_palettes() {
        let themes = builtin_themes();
        let ids: Vec<&str> = themes.iter().map(|theme| theme.id.as_str()).collect();
        for expected in [
            "default",
            "zelda",
            "cyberpunk",
            "rei",
            "monokai",
            "matrix",
            "cviper",
        ] {
            assert!(ids.contains(&expected), "{expected} is missing: {ids:?}");
        }
        assert_eq!(themes.len(), 7, "six themes plus the default");

        // Every theme must be visually distinct: no two share an accent.
        let mut accents: Vec<Color> = themes.iter().map(|theme| theme.palette.accent).collect();
        let total = accents.len();
        accents.sort_by_key(|color| format!("{color:?}"));
        accents.dedup();
        assert_eq!(accents.len(), total, "two themes share an accent colour");
    }

    #[test]
    fn the_default_theme_leaves_the_terminal_background_alone() {
        let theme = default_theme();
        assert!(!theme.paints_background());
        assert!(theme.palette.background.is_none());
        assert_eq!(theme.base_style(), ratatui::style::Style::new());
    }

    #[test]
    fn a_themed_palette_paints_its_background() {
        let matrix = &builtin_themes()[5];
        assert_eq!(matrix.id, "matrix");
        assert!(matrix.paints_background());
        let style = matrix.base_style();
        assert_eq!(style.bg, Some(Color::Rgb(0x00, 0x00, 0x00)));
    }

    #[test]
    fn parses_six_and_three_digit_hex() {
        assert_eq!(parse_hex("#00ff41"), Some(Color::Rgb(0, 255, 65)));
        assert_eq!(parse_hex("00ff41"), None, "the # is required");
        assert_eq!(parse_hex("#0f0"), Some(Color::Rgb(0, 255, 0)));
        assert_eq!(parse_hex("#gggggg"), None);
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex(""), None);
    }

    #[test]
    fn loads_a_user_theme_and_takes_its_id_from_the_file_name() {
        let dir = sandbox("load");
        std::fs::write(
            dir.join("themes/sunset.toml"),
            r##"
name = "Sunset"
author = "someone"
description = "warm"
[colors]
background = "#201010"
foreground = "#ffeee0"
muted = "#806058"
accent = "#ff8a5c"
success = "#8fd18f"
warning = "#ffc857"
error = "#ff5c5c"
selection = "#3a1f1a"
"##,
        )
        .expect("write");

        let catalogue = catalogue(&dir);
        assert!(catalogue.problems.is_empty(), "{:?}", catalogue.problems);
        let theme = catalogue.find("sunset").expect("the user theme is loaded");
        assert_eq!(theme.name, "Sunset");
        assert_eq!(theme.author.as_deref(), Some("someone"));
        assert!(theme.user_defined);
        assert_eq!(theme.palette.accent, Color::Rgb(0xff, 0x8a, 0x5c));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_theme_with_a_bad_colour_is_reported_and_the_builtins_still_load() {
        let dir = sandbox("badcolor");
        std::fs::write(
            dir.join("themes/broken.toml"),
            "name = \"Broken\"\n[colors]\nmuted = \"#333\"\naccent = \"not-a-colour\"\n\
             success = \"#0f0\"\nwarning = \"#ff0\"\nerror = \"#f00\"\n",
        )
        .expect("write");

        let catalogue = catalogue(&dir);
        assert_eq!(catalogue.problems.len(), 1);
        let problem = catalogue.problems[0].to_string();
        assert!(problem.contains("accent"), "{problem}");
        assert!(problem.contains("not-a-colour"), "{problem}");
        assert_eq!(catalogue.themes.len(), 7, "the built-ins are untouched");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_theme_missing_a_required_colour_says_which_one() {
        let dir = sandbox("missing");
        std::fs::write(
            dir.join("themes/partial.toml"),
            "name = \"Partial\"\n[colors]\naccent = \"#0ff\"\n",
        )
        .expect("write");

        let catalogue = catalogue(&dir);
        let problem = catalogue.problems[0].to_string();
        assert!(problem.contains("muted"), "{problem}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_key_is_rejected_instead_of_ignored() {
        let dir = sandbox("unknown");
        std::fs::write(
            dir.join("themes/typo.toml"),
            "name = \"Typo\"\naccentt = \"#0ff\"\n[colors]\nmuted = \"#333\"\naccent = \"#0ff\"\n\
             success = \"#0f0\"\nwarning = \"#ff0\"\nerror = \"#f00\"\n",
        )
        .expect("write");
        let catalogue = catalogue(&dir);
        assert_eq!(catalogue.problems.len(), 1);
        assert!(catalogue.problems[0].to_string().contains("accentt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_user_theme_may_not_shadow_a_built_in() {
        let dir = sandbox("shadow");
        std::fs::write(
            dir.join("themes/matrix.toml"),
            "name = \"Fake Matrix\"\n[colors]\nmuted = \"#333\"\naccent = \"#0ff\"\n\
             success = \"#0f0\"\nwarning = \"#ff0\"\nerror = \"#f00\"\n",
        )
        .expect("write");

        let catalogue = catalogue(&dir);
        assert_eq!(catalogue.problems.len(), 1);
        assert!(catalogue.problems[0].to_string().contains("already taken"));
        assert_eq!(catalogue.find("matrix").expect("builtin").name, "Matrix");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_that_does_not_exist_is_not_an_error() {
        let catalogue = catalogue(Path::new("/nonexistent/occ-themes"));
        assert_eq!(catalogue.themes.len(), 7);
        assert!(catalogue.problems.is_empty());
    }
}
