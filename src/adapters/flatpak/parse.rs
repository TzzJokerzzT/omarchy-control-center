//! Parsers for flatpak output. Pure functions, no process and no filesystem, so
//! the exact shape of `flatpak list` and `flatpak info` can be pinned by fixtures.
//!
//! `flatpak list --columns=...` prints one row per component with the columns in
//! the requested order, tab separated. `flatpak info` prints a `Name - summary`
//! header followed by an aligned `Key: value` block. A row or field that does not
//! match the shape is reported as such instead of being guessed at.

use crate::domain::InstallScope;

/// One `flatpak list` row, already validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRow {
    /// The ref column: the identifier flatpak itself uses.
    pub reference: String,
    /// The application id flatpak prints for this ref.
    pub application: String,
    pub version: Option<String>,
    pub origin: Option<String>,
    pub scope: InstallScope,
    pub installed_size: Option<u64>,
}

/// Outcome of one `flatpak list` line. A skipped line carries the reason, so the
/// caller can warn about it without retrying or inventing a component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedRow {
    Row(ListRow),
    Skipped { line: String, reason: String },
}

/// Number of columns requested by the adapter.
pub const COLUMN_COUNT: usize = 6;

/// Parses the tab-separated rows of `flatpak list --columns=...`.
pub fn parse_list(text: &str) -> Vec<ParsedRow> {
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let columns: Vec<&str> = line.split('\t').map(str::trim).collect();
        if columns.len() != COLUMN_COUNT {
            rows.push(ParsedRow::Skipped {
                line: line.to_string(),
                reason: format!(
                    "expected {COLUMN_COUNT} tab-separated columns, found {}",
                    columns.len()
                ),
            });
            continue;
        }

        let [reference, application, version, origin, installation, size] = [
            columns[0], columns[1], columns[2], columns[3], columns[4], columns[5],
        ];
        if reference.is_empty() {
            rows.push(ParsedRow::Skipped {
                line: line.to_string(),
                reason: "empty ref column".to_string(),
            });
            continue;
        }
        if application.is_empty() {
            rows.push(ParsedRow::Skipped {
                line: line.to_string(),
                reason: "empty application column".to_string(),
            });
            continue;
        }
        // An installation flatpak does not name is not guessed into user or
        // system: the row is dropped and reported.
        let Some(scope) = parse_scope(installation) else {
            rows.push(ParsedRow::Skipped {
                line: line.to_string(),
                reason: format!("unknown installation {installation:?}"),
            });
            continue;
        };

        rows.push(ParsedRow::Row(ListRow {
            reference: reference.to_string(),
            application: application.to_string(),
            version: optional(version),
            origin: optional(origin),
            scope,
            installed_size: parse_size(size),
        }));
    }
    rows
}

/// `user` and `system` are the only installations flatpak defines.
pub fn parse_scope(value: &str) -> Option<InstallScope> {
    match value.trim() {
        "user" => Some(InstallScope::User),
        "system" => Some(InstallScope::System),
        _ => None,
    }
}

/// Converts a flatpak size (`250.4 MB`, `1.2 GB`) into bytes.
///
/// flatpak prints decimal units, not the binary ones pacman uses, so this parser
/// keeps its own table instead of reusing pacman's.
pub fn parse_size(text: &str) -> Option<u64> {
    let mut parts = text.split_whitespace();
    let number: f64 = parts.next()?.parse().ok()?;
    if number < 0.0 {
        return None;
    }
    let factor: f64 = match parts.next()? {
        "B" | "bytes" => 1.0,
        "kB" => 1_000.0,
        "MB" => 1_000_000.0,
        "GB" => 1_000_000_000.0,
        "TB" => 1e12,
        _ => return None,
    };
    Some((number * factor) as u64)
}

/// A `flatpak info` block: the `Name - summary` header and its aligned fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    pub header: Option<String>,
    pub fields: Vec<(String, String)>,
}

impl Info {
    /// Raw value as printed by flatpak. `None` when the field is absent or empty.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(field, _)| field == key)
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty())
    }

    /// The name half of the `Name - summary` header.
    pub fn header_name(&self) -> Option<&str> {
        let name = match self.header.as_deref()?.split_once(" - ") {
            Some((name, _)) => name.trim(),
            None => self.header.as_deref()?.trim(),
        };
        (!name.is_empty()).then_some(name)
    }

    /// The summary half of the `Name - summary` header.
    pub fn header_summary(&self) -> Option<&str> {
        self.header
            .as_deref()?
            .split_once(" - ")
            .map(|(_, summary)| summary.trim())
            .filter(|summary| !summary.is_empty())
    }
}

/// Parses one `flatpak info` block. Never fails: what is not printed stays absent.
pub fn parse_info(text: &str) -> Info {
    let mut info = Info::default();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match line.split_once(':') {
            Some((key, value)) if !key.trim().is_empty() => {
                info.fields
                    .push((key.trim().to_string(), value.trim().to_string()));
            }
            _ => {
                // The only non-field line flatpak prints is the leading
                // `Name - summary` header.
                if info.header.is_none() && info.fields.is_empty() {
                    info.header = Some(line.trim().to_string());
                }
            }
        }
    }
    info
}

fn optional(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPS: &str = include_str!("../../../tests/fixtures/flatpak/list-apps.txt");
    const RUNTIMES: &str = include_str!("../../../tests/fixtures/flatpak/list-runtimes.txt");
    const INFO: &str = include_str!("../../../tests/fixtures/flatpak/info.txt");

    fn rows(text: &str) -> Vec<ListRow> {
        parse_list(text)
            .into_iter()
            .map(|row| match row {
                ParsedRow::Row(row) => row,
                ParsedRow::Skipped { reason, .. } => panic!("unexpected skip: {reason}"),
            })
            .collect()
    }

    #[test]
    fn parses_every_column_of_a_list_row() {
        let apps = rows(APPS);
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].reference, "app/org.mozilla.firefox/x86_64/stable");
        assert_eq!(apps[0].application, "org.mozilla.firefox");
        assert_eq!(apps[0].version.as_deref(), Some("129.0.2"));
        assert_eq!(apps[0].origin.as_deref(), Some("flathub"));
        assert_eq!(apps[0].scope, InstallScope::System);
        assert_eq!(apps[0].installed_size, Some(250_400_000));
        assert_eq!(apps[1].scope, InstallScope::User);

        let runtimes = rows(RUNTIMES);
        assert_eq!(
            runtimes[0].reference,
            "runtime/org.freedesktop.Platform/x86_64/24.08"
        );
        assert_eq!(runtimes[0].installed_size, Some(1_200_000_000));
    }

    #[test]
    fn skips_a_row_with_an_installation_it_cannot_name() {
        let text =
            "app/org.example.App/x86_64/stable\torg.example.App\t1.0\tflathub\tportable\t1.0 MB\n";
        let parsed = parse_list(text);
        assert_eq!(parsed.len(), 1);
        match &parsed[0] {
            ParsedRow::Skipped { reason, .. } => {
                assert!(
                    reason.contains("portable"),
                    "reason must name the value: {reason}"
                )
            }
            other => panic!("must be skipped, got {other:?}"),
        }
    }

    #[test]
    fn skips_a_malformed_row_without_dropping_the_good_ones() {
        let text = "app/org.example.App/x86_64/stable\torg.example.App\t1.0\tflathub\tsystem\t1.0 MB\ntoo\tfew\n";
        let parsed = parse_list(text);
        assert_eq!(parsed.len(), 2);
        assert!(matches!(parsed[0], ParsedRow::Row(_)));
        assert!(matches!(parsed[1], ParsedRow::Skipped { .. }));
    }

    #[test]
    fn parses_the_decimal_units_flatpak_prints() {
        assert_eq!(parse_size("250.4 MB"), Some(250_400_000));
        assert_eq!(parse_size("1.2 GB"), Some(1_200_000_000));
        assert_eq!(parse_size("980.5 MB"), Some(980_500_000));
        assert_eq!(parse_size("512 kB"), Some(512_000));
        assert_eq!(parse_size("1024 B"), Some(1024));
        assert_eq!(parse_size("Unknown"), None);
        assert_eq!(parse_size("12 MiB"), None);
    }

    #[test]
    fn parses_the_info_header_and_fields() {
        let info = parse_info(INFO);
        assert_eq!(info.header_name(), Some("Firefox"));
        assert_eq!(
            info.header_summary(),
            Some("Fast, private and secure web browser")
        );
        assert_eq!(
            info.get("Ref"),
            Some("app/org.mozilla.firefox/x86_64/stable")
        );
        assert_eq!(info.get("Installation"), Some("system"));
        assert_eq!(info.get("Installed Size"), Some("250.4 MB"));
        assert_eq!(info.get("Subject"), Some("Web browser"));
        assert_eq!(info.get("Nonexistent"), None);
    }

    #[test]
    fn an_empty_block_is_absent_information_not_a_panic() {
        let info = parse_info("");
        assert_eq!(info.header, None);
        assert_eq!(info.header_name(), None);
        assert_eq!(info.header_summary(), None);
        assert!(parse_list("").is_empty());
    }
}
