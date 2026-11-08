//! Identifier validation.
//!
//! This is the injection boundary: a name that reaches `CommandSpec::args` must
//! come from the manager's own output and must satisfy an allowlist. A display
//! label is never accepted here.

/// pacman package names: alphanumerics plus `@ . _ + -`, never leading with `-`.
pub fn validate_pacman_name(value: &str) -> Result<(), &'static str> {
    validate_common(value)?;
    if value.starts_with('-') {
        return Err("must not start with '-' (that would be read as an option)");
    }
    if let Some(bad) = value
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '+' | '-')))
    {
        return Err(reason_for(bad));
    }
    Ok(())
}

/// flatpak references: `app/org.example.App/x86_64/stable` and plain app ids.
pub fn validate_flatpak_ref(value: &str) -> Result<(), &'static str> {
    validate_common(value)?;
    if value.starts_with('-') {
        return Err("must not start with '-' (that would be read as an option)");
    }
    if let Some(bad) = value.chars().find(|c| {
        !(c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '+' | '-' | '/'))
    }) {
        return Err(reason_for(bad));
    }
    // `..` would let a reference name a directory outside the flatpak store.
    for segment in value.split('/') {
        match segment {
            "" => return Err("has an empty path segment"),
            "." => return Err("has a current-directory segment"),
            ".." => return Err("has a parent-directory segment"),
            _ => {}
        }
    }
    Ok(())
}

fn validate_common(value: &str) -> Result<(), &'static str> {
    if value.is_empty() {
        return Err("must not be empty");
    }
    if value.len() > 255 {
        return Err("is longer than 255 bytes");
    }
    if value.trim() != value {
        return Err("has leading or trailing whitespace");
    }
    if let Some(bad) = value.chars().find(|c| c.is_control()) {
        return Err(reason_for(bad));
    }
    Ok(())
}

fn reason_for(bad: char) -> &'static str {
    match bad {
        ' ' | '\t' => "contains whitespace",
        '\0' => "contains a NUL byte",
        '"' | '\'' => "contains a quote",
        ';' => "contains a shell separator",
        '|' | '&' => "contains a shell control operator",
        '$' => "contains a shell expansion marker",
        '`' => "contains a command substitution marker",
        '\\' => "contains an escape character",
        '>' | '<' => "contains a redirection operator",
        ':' => "contains a colon",
        _ => "contains a character outside the allowed set",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejected(validate: fn(&str) -> Result<(), &'static str>, value: &str) -> bool {
        validate(value).is_err()
    }

    #[test]
    fn accepts_real_package_names() {
        for name in ["firefox", "python-setuptools", "gcc-libs", "lib32-glibc", "c++utilities", "nodejs-nopt"] {
            assert!(validate_pacman_name(name).is_ok(), "{name} should be accepted");
        }
        assert!(validate_pacman_name("a@b.c_d+e-f").is_ok());
    }

    #[test]
    fn rejects_injection_shaped_names() {
        let hostile = [
            "firefox; rm -rf /",
            "firefox|cat",
            "$(touch /tmp/pwned)",
            "`id`",
            "firefox chrome",
            "firefox\nrm",
            "firefox\"quote",
            "firefox\\escape",
            "-Rns",
            "",
            "   ",
            "a>out",
            "a<b",
        ];
        for value in hostile {
            assert!(rejected(validate_pacman_name, value), "{value:?} must be rejected");
        }
    }

    #[test]
    fn rejects_flatpak_traversal_and_options() {
        assert!(validate_flatpak_ref("app/org.mozilla.firefox/x86_64/stable").is_ok());
        assert!(validate_flatpak_ref("org.mozilla.firefox").is_ok());
        for value in ["../../etc/passwd", "-y", "app/../../../etc", "app id", "app;id", ""] {
            assert!(rejected(validate_flatpak_ref, value), "{value:?} must be rejected");
        }
    }

    #[test]
    fn rejects_oversized_identifiers() {
        let long = "a".repeat(256);
        assert!(rejected(validate_pacman_name, &long));
    }
}
