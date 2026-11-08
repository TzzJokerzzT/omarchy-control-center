//! Parsers for `systemctl` output. Pure functions: no process, no filesystem, so the
//! exact column layouts can be pinned by fixtures.
//!
//! `systemctl list-units --no-legend` prints `UNIT LOAD ACTIVE SUB DESCRIPTION`, padded
//! with spaces, and prefixes a unit that failed with a marker such as `●`. The
//! description is free text and may contain spaces, so it must be taken as a remainder
//! rather than as a field.

use std::collections::BTreeMap;

/// One row of `systemctl list-units --type=service --all`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitRow {
    pub load: String,
    pub active: String,
    pub sub: String,
    pub description: String,
}

/// Splits a line into at most `count` whitespace-separated fields and returns the rest
/// untouched, with only its leading padding removed. A naive `split_whitespace` would
/// rewrite the description's own spacing.
fn split_fields(line: &str, count: usize) -> (Vec<&str>, &str) {
    let mut fields = Vec::with_capacity(count);
    let mut cursor = 0;
    while fields.len() < count {
        let rest = &line[cursor..];
        let trimmed = rest.trim_start();
        cursor += rest.len() - trimmed.len();
        if trimmed.is_empty() {
            return (fields, "");
        }
        let token_len = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        fields.push(&trimmed[..token_len]);
        cursor += token_len;
    }
    (fields, line[cursor..].trim_start())
}

/// Drops the status marker systemd puts in front of a failed or not-found unit.
fn without_marker(line: &str) -> &str {
    line.trim_start()
        .trim_start_matches(['●', '×', '*', '·'])
        .trim_start()
}

fn is_header(name: &str) -> bool {
    matches!(name, "UNIT" | "UNIT FILE")
}

/// `systemctl list-units` rows, keyed by unit name.
pub fn parse_units(text: &str) -> BTreeMap<String, UnitRow> {
    let mut rows = BTreeMap::new();
    for line in text.lines() {
        let line = without_marker(line);
        if line.trim().is_empty() {
            continue;
        }
        let (fields, description) = split_fields(line, 4);
        let [name, load, active, sub] = fields[..] else {
            continue;
        };
        if is_header(name) {
            continue;
        }
        rows.insert(
            name.to_string(),
            UnitRow {
                load: load.to_string(),
                active: active.to_string(),
                sub: sub.to_string(),
                description: description.to_string(),
            },
        );
    }
    rows
}

/// `systemctl list-unit-files` rows: unit name to the state column (`enabled`,
/// `disabled`, `static`, `masked`, …). The optional `PRESET` column is ignored because
/// it describes what a preset would do, not what this machine does.
pub fn parse_unit_files(text: &str) -> BTreeMap<String, String> {
    let mut states = BTreeMap::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (fields, _) = split_fields(line, 2);
        let [name, state] = fields[..] else {
            continue;
        };
        if is_header(name) {
            continue;
        }
        states.insert(name.to_string(), state.to_string());
    }
    states
}

/// `systemctl show -p …` prints one `Key=Value` per line. An empty value stays absent
/// so the caller can say "not reported" instead of showing a blank fact.
pub fn parse_show(text: &str) -> BTreeMap<String, String> {
    let mut properties = BTreeMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            continue;
        }
        properties.insert(key.to_string(), value.to_string());
    }
    properties
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_unit_rows_and_strips_the_failure_marker() {
        let rows = parse_units(
            "  NetworkManager.service                        loaded    active   running Network Manager\n\
             \u{25cf} cups.service                                loaded    failed   failed  CUPS Scheduler\n",
        );
        let network = rows.get("NetworkManager.service").expect("network manager");
        assert_eq!(network.load, "loaded");
        assert_eq!(network.active, "active");
        assert_eq!(network.sub, "running");
        assert_eq!(network.description, "Network Manager");

        let cups = rows.get("cups.service").expect("cups");
        assert_eq!(cups.active, "failed");
        assert_eq!(cups.sub, "failed");
    }

    #[test]
    fn keeps_a_description_with_several_words_verbatim() {
        let rows =
            parse_units("  pipewire.service loaded active running PipeWire  Multimedia  Service\n");
        let row = rows.get("pipewire.service").expect("pipewire");
        assert_eq!(row.description, "PipeWire  Multimedia  Service");
    }

    #[test]
    fn reads_unit_file_states_and_ignores_a_legend_and_the_preset_column() {
        let states = parse_unit_files(
            "UNIT FILE                                     STATE           PRESET\n\
             bluetooth.service                             disabled        enabled\n\
             pipewire.service                              enabled         enabled\n",
        );
        assert_eq!(states.len(), 2);
        assert_eq!(
            states.get("bluetooth.service").map(String::as_str),
            Some("disabled")
        );
        assert_eq!(
            states.get("pipewire.service").map(String::as_str),
            Some("enabled")
        );
    }

    #[test]
    fn reads_show_properties_and_skips_empty_ones() {
        let properties = parse_show(
            "Description=Voxtype dictation daemon\nActiveState=inactive\nFragmentPath=\n",
        );
        assert_eq!(
            properties.get("Description").map(String::as_str),
            Some("Voxtype dictation daemon")
        );
        assert!(!properties.contains_key("FragmentPath"));
    }
}
