//! Parsers for pacman output. Pure functions, no process and no filesystem, so the
//! exact shape of `pacman -Qi` can be pinned by fixtures.
//!
//! `pacman -Qi` prints `Field            : value` records separated by blank lines,
//! and wraps long values onto continuation lines indented under the field. A parser
//! that ignored those continuation lines would silently lose dependencies.

use std::collections::BTreeSet;

/// One `pacman -Qi` record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QiRecord {
    fields: Vec<(String, String)>,
}

impl QiRecord {
    /// Raw value as printed by pacman. `None` when the field is absent or "None".
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(field, _)| field == key)
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty() && *value != "None")
    }

    /// Whitespace-separated values, the shape pacman uses for dependency lists.
    pub fn list(&self, key: &str) -> Vec<String> {
        self.get(key)
            .map(|value| {
                value
                    .split_whitespace()
                    .map(|item| item.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Parses every record in a `pacman -Qi` dump.
pub fn parse_qi(text: &str) -> Vec<QiRecord> {
    let mut records = Vec::new();
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut current: Option<String> = None;

    for line in text.lines() {
        if line.trim().is_empty() {
            if !fields.is_empty() {
                records.push(QiRecord {
                    fields: std::mem::take(&mut fields),
                });
                current = None;
            }
            continue;
        }

        let is_continuation = line.starts_with(' ') || line.starts_with('\t');
        if is_continuation {
            if let Some(key) = &current
                && let Some((_, value)) = fields.iter_mut().rev().find(|(field, _)| field == key)
            {
                value.push(' ');
                value.push_str(line.trim());
            }
            continue;
        }

        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        let value = value.trim().to_string();
        if key.is_empty() {
            continue;
        }
        current = Some(key.clone());
        fields.push((key, value));
    }

    if !fields.is_empty() {
        records.push(QiRecord { fields });
    }
    records
}

/// Parses `pacman -Q` lines into `(name, version)`.
pub fn parse_q(text: &str) -> Vec<(String, String)> {
    let mut packages = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(name), Some(version)) = (parts.next(), parts.next()) {
            packages.push((name.to_string(), version.to_string()));
        }
    }
    packages
}

/// Parses the first column of a name list (`pacman -Qm`, `pacman -Qdtq`).
pub fn parse_name_list(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(|name| name.to_string())
        .collect()
}

/// Converts a pacman size (`5.08 MiB`, `1024.00 B`) into bytes.
pub fn parse_size(text: &str) -> Option<u64> {
    let mut parts = text.split_whitespace();
    let number: f64 = parts.next()?.parse().ok()?;
    let unit = parts.next().unwrap_or("B");
    let factor: f64 = match unit {
        "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0_f64.powi(4),
        _ => return None,
    };
    if number < 0.0 {
        return None;
    }
    Some((number * factor) as u64)
}

/// Reads the `Repository` field out of `pacman -Si` output.
pub fn parse_repository(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| {
            line.strip_prefix("Repository")
                .and_then(|rest| rest.split_once(':'))
        })
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty() && value != "None")
}

#[cfg(test)]
mod tests {
    use super::*;

    const QI: &str = include_str!("../../../tests/fixtures/pacman/qi.txt");
    const Q: &str = include_str!("../../../tests/fixtures/pacman/q.txt");
    const QM: &str = include_str!("../../../tests/fixtures/pacman/qm.txt");
    const QDTQ: &str = include_str!("../../../tests/fixtures/pacman/qdtq.txt");
    const SI: &str = include_str!("../../../tests/fixtures/pacman/si.txt");

    fn record<'a>(records: &'a [QiRecord], name: &str) -> &'a QiRecord {
        records
            .iter()
            .find(|record| record.get("Name") == Some(name))
            .unwrap_or_else(|| panic!("no record for {name}"))
    }

    #[test]
    fn parses_every_record_and_its_fields() {
        let records = parse_qi(QI);
        assert_eq!(records.len(), 4);

        let btop = record(&records, "btop");
        assert_eq!(btop.get("Version"), Some("1.4.0-1"));
        assert_eq!(btop.get("Description"), Some("A monitor of resources"));
        assert_eq!(btop.get("Install Reason"), Some("Explicitly installed"));
    }

    #[test]
    fn keeps_wrapped_continuation_lines_of_a_field() {
        let records = parse_qi(QI);
        let libfoo = record(&records, "libfoo");
        let optional = libfoo.list("Optional Deps");
        assert_eq!(
            optional,
            vec![
                "gdb:",
                "debugging",
                "symbols",
                "[installed]",
                "lldb:",
                "alternative",
                "debugger"
            ],
            "continuation lines belong to the same field: {optional:?}"
        );
        assert_eq!(
            libfoo.get("Optional Deps"),
            Some("gdb: debugging symbols [installed] lldb: alternative debugger")
        );
    }

    #[test]
    fn treats_none_as_absent_instead_of_as_a_value() {
        let records = parse_qi(QI);
        let btop = record(&records, "btop");
        assert_eq!(btop.get("Groups"), None);
        assert!(btop.list("Groups").is_empty());
        assert_eq!(btop.get("Required By"), None);
    }

    #[test]
    fn splits_dependency_lists_on_whitespace() {
        let records = parse_qi(QI);
        let btop = record(&records, "btop");
        assert_eq!(
            btop.list("Depends On"),
            vec!["gcc-libs", "glibc", "libstdc++.so=6-64"]
        );
        let gcc = record(&records, "gcc-libs");
        assert_eq!(gcc.list("Required By"), vec!["btop"]);
    }

    #[test]
    fn parses_sizes_in_every_unit_pacman_prints() {
        assert_eq!(parse_size("1024.00 B"), Some(1024));
        assert_eq!(parse_size("5.08 MiB"), Some(5_326_766));
        assert_eq!(parse_size("72.15 MiB"), Some(75_654_758));
        assert_eq!(parse_size("1.00 KiB"), Some(1024));
        assert_eq!(parse_size("not-a-size"), None);
    }

    #[test]
    fn parses_package_and_name_lists() {
        assert_eq!(parse_q(Q).len(), 4);
        assert_eq!(parse_q(Q)[0], ("btop".to_string(), "1.4.0-1".to_string()));
        assert_eq!(
            parse_name_list(QM).into_iter().collect::<Vec<_>>(),
            vec!["libfoo".to_string(), "yay".to_string()]
        );
        assert_eq!(
            parse_name_list(QDTQ).into_iter().collect::<Vec<_>>(),
            vec!["libfoo".to_string()]
        );
        assert!(parse_name_list("").is_empty());
    }

    #[test]
    fn reads_the_repository_from_si_output() {
        assert_eq!(parse_repository(SI), Some("extra".to_string()));
        assert_eq!(parse_repository("Name : btop\n"), None);
        assert_eq!(parse_repository("Repository      : None\n"), None);
    }

    #[test]
    fn ignores_empty_input_without_panicking() {
        assert!(parse_qi("").is_empty());
        assert!(parse_q("").is_empty());
    }
}
