//! Host detection: which distribution this is, whether Omarchy is present, and
//! whether we are allowed to run at all.

use std::path::Path;

const OS_RELEASE: &str = "/etc/os-release";
const PROC_STATUS: &str = "/proc/self/status";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInfo {
    pub os_name: Option<String>,
    pub os_id: Option<String>,
    pub version_id: Option<String>,
    pub kernel: Option<String>,
    pub is_arch_like: bool,
    pub is_omarchy: bool,
    /// Effective uid. `None` when it could not be determined.
    pub euid: Option<u32>,
}

impl SystemInfo {
    pub fn detect() -> Self {
        let os_release = std::fs::read_to_string(OS_RELEASE).unwrap_or_default();
        let parsed = OsRelease::parse(&os_release);
        let status = std::fs::read_to_string(PROC_STATUS).unwrap_or_default();
        let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|value| value.trim().to_string());

        Self {
            is_arch_like: parsed.is_arch_like(),
            is_omarchy: parsed.os_id.as_deref() == Some("omarchy")
                || Path::new("/etc/omarchy.conf").exists(),
            os_name: parsed.os_name,
            os_id: parsed.os_id,
            version_id: parsed.version_id,
            kernel,
            euid: effective_uid(&status),
        }
    }

    pub fn display_name(&self) -> String {
        let name = self.os_name.as_deref().unwrap_or("unknown system");
        match &self.version_id {
            Some(version) => format!("{name} {version}"),
            None => name.to_string(),
        }
    }
}

/// Why the application refuses to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupRefusal {
    Root,
}

impl std::fmt::Display for StartupRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Root => write!(
                formatter,
                "refusing to run as root: start omarchy-control-center as your own user. \
                 The few operations that need privileges ask sudo for exactly the confirmed \
                 command, and the interface itself never runs privileged."
            ),
        }
    }
}

/// The interface must never run privileged: a bug in the renderer or a stray
/// keypress must not be able to delete system files.
pub fn guard_not_root(euid: Option<u32>) -> Result<(), StartupRefusal> {
    match euid {
        Some(0) => Err(StartupRefusal::Root),
        _ => Ok(()),
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OsRelease {
    pub os_name: Option<String>,
    pub os_id: Option<String>,
    pub id_like: Option<String>,
    pub version_id: Option<String>,
    pub build_id: Option<String>,
}

impl OsRelease {
    pub fn parse(text: &str) -> Self {
        let mut release = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"').to_string();
            match key.trim() {
                "NAME" => release.os_name = Some(value),
                "ID" => release.os_id = Some(value),
                "ID_LIKE" => release.id_like = Some(value),
                "VERSION_ID" => release.version_id = Some(value),
                "BUILD_ID" => release.build_id = Some(value),
                _ => {}
            }
        }
        release
    }

    pub fn is_arch_like(&self) -> bool {
        let id_is_arch = self.os_id.as_deref() == Some("arch");
        let like_arch = self
            .id_like
            .as_deref()
            .is_some_and(|like| like.split_whitespace().any(|part| part == "arch"));
        id_is_arch || like_arch
    }
}

/// Reads the effective uid out of `/proc/self/status`.
pub fn effective_uid(status: &str) -> Option<u32> {
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            let mut fields = rest.split_whitespace();
            let _real = fields.next()?;
            return fields.next()?.parse().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const OMARCHY: &str = "NAME=\"Omarchy\"\nID=omarchy\nID_LIKE=arch\nBUILD_ID=\"4.5.0\"\n\
                           VERSION_ID=\"4.5.0\"\n";

    #[test]
    fn parses_omarchy_os_release_and_sees_arch_compatibility() {
        let release = OsRelease::parse(OMARCHY);
        assert_eq!(release.os_name.as_deref(), Some("Omarchy"));
        assert_eq!(release.os_id.as_deref(), Some("omarchy"));
        assert_eq!(release.version_id.as_deref(), Some("4.5.0"));
        assert!(release.is_arch_like());
    }

    #[test]
    fn a_non_arch_system_is_not_arch_like() {
        let release = OsRelease::parse("NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\n");
        assert!(!release.is_arch_like());
    }

    #[test]
    fn reads_the_effective_uid_not_the_real_one() {
        let status = "Name\tocc\nUid:\t1000\t0\t0\t0\nGid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(effective_uid(status), Some(0));
        assert_eq!(effective_uid("Name\tocc\n"), None);
    }

    #[test]
    fn refuses_to_run_privileged() {
        assert_eq!(guard_not_root(Some(0)), Err(StartupRefusal::Root));
        assert_eq!(guard_not_root(Some(1000)), Ok(()));
        assert_eq!(guard_not_root(None), Ok(()));
    }
}
