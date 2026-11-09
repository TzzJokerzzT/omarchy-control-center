//! Version checking and verified updates.
//!
//! Three rules, and they are the whole design:
//!
//! 1. **Checking is a read.** It asks the release API for the newest tag and compares it with
//!    the version this binary was built from. It is opt-in through `check_for_updates` in the
//!    configuration, because a program that contacts a server on every start should say so.
//! 2. **Nothing is installed without a published checksum.** The release must ship a
//!    `SHA256SUMS` asset; without it, the update refuses. An unverified binary is a supply
//!    chain hole with a friendly name.
//! 3. **Downloaded content is never executed.** The new binary is written next to the current
//!    one, verified, and then swapped in by an atomic rename. The running process keeps its
//!    own inode, and the user starts the new version when they choose to.
//!
//! `curl` is used for the two HTTP reads instead of adding an HTTP client: it is already
//! present on any Arch install, and it goes through the same process runner as everything
//! else — argv only, with a deadline, and a missing tool is reported instead of guessed at.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::infrastructure::process::{CommandRunner, CommandSpec, ProcessError};

/// Where releases live.
pub const DEFAULT_REPOSITORY: &str = "TzzJokerzzT/omarchy-control-center";

/// The asset names the release workflow publishes.
pub const BINARY_ASSET: &str = "omarchy-control-center-x86_64-linux";
pub const CHECKSUM_ASSET: &str = "SHA256SUMS";

const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UpdateError {
    #[error("curl was not found, so the release information cannot be read")]
    NoCurl,

    #[error("could not reach the release API: {detail}")]
    Network { detail: String },

    #[error("the release API answered with something unreadable: {detail}")]
    Parse { detail: String },

    #[error("the release does not publish a SHA256SUMS asset, so the download cannot be verified")]
    NoChecksum,

    #[error("the release does not publish {BINARY_ASSET}; assets: {available}")]
    NoBinaryAsset { available: String },

    #[error("the checksum file does not mention {asset}")]
    ChecksumMissingEntry { asset: String },

    #[error("checksum mismatch: expected {expected}, got {actual} — the file was NOT installed")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("could not install the new binary: {detail}")]
    Install { detail: String },
}

/// A parsed version. Only the parts this program uses: numeric triples, with an optional
/// pre-release suffix that sorts before the release it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl Version {
    /// Parses `0.2.0`, `v0.2.0` and `0.3.0-rc1`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre.to_string())),
            None => (text, None),
        };
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().unwrap_or("0").parse().ok()?;
        let patch = parts.next().unwrap_or("0").parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }

    pub fn is_newer_than(&self, other: &Self) -> bool {
        self > other
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    /// Semver ordering, written by hand because deriving it would compare the pre-release
    /// suffix as an `Option`, where `None` sorts *below* `Some` — which would rank `1.0.0`
    /// under `1.0.0-rc1`, exactly backwards.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (Some(left), Some(right)) => left.cmp(right),
            })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.pre {
            Some(pre) => write!(
                formatter,
                "{}.{}.{}-{pre}",
                self.major, self.minor, self.patch
            ),
            None => write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

/// What a check concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// This binary is the newest release.
    UpToDate { current: Version, latest: Version },
    /// A newer release exists, with the assets it publishes.
    Newer {
        current: Version,
        latest: Version,
        assets: Vec<ReleaseAssetView>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAssetView {
    pub name: String,
    pub url: String,
}

impl CheckOutcome {
    pub fn is_newer(&self) -> bool {
        matches!(self, Self::Newer { .. })
    }

    /// One line the interface can show without further interpretation.
    pub fn summary(&self) -> String {
        match self {
            Self::UpToDate { current, .. } => format!("v{current} is the newest release"),
            Self::Newer {
                current, latest, ..
            } => {
                format!("v{latest} is available (running v{current})")
            }
        }
    }
}

pub struct Updater<'a> {
    runner: &'a dyn CommandRunner,
    repository: String,
    /// Directory for the downloaded file before it is installed.
    download_dir: PathBuf,
}

impl<'a> Updater<'a> {
    pub fn new(runner: &'a dyn CommandRunner, download_dir: PathBuf) -> Self {
        Self {
            runner,
            repository: DEFAULT_REPOSITORY.to_string(),
            download_dir,
        }
    }

    fn curl(
        &self,
        url: &str,
        destination: Option<&Path>,
        timeout: std::time::Duration,
    ) -> Result<String, UpdateError> {
        if self.runner.resolve("curl").is_none() {
            return Err(UpdateError::NoCurl);
        }

        let mut args = vec![
            "--fail".to_string(),
            "--silent".to_string(),
            "--show-error".to_string(),
            "--location".to_string(),
            "--max-time".to_string(),
            timeout.as_secs().to_string(),
            "--proto".to_string(),
            "=https".to_string(),
        ];
        match destination {
            Some(path) => {
                args.push("--output".to_string());
                args.push(path.display().to_string());
                args.push("--".to_string());
                args.push(url.to_string());
            }
            None => args.push(url.to_string()),
        }

        let spec = CommandSpec::capture("curl", args).with_timeout(timeout);
        let output = self.runner.run(&spec).map_err(|error| match error {
            ProcessError::NotFound { .. } => UpdateError::NoCurl,
            other => UpdateError::Network {
                detail: other.to_string(),
            },
        })?;
        if !output.success() {
            return Err(UpdateError::Network {
                detail: format!(
                    "curl exited {:?}: {}",
                    output.exit_code,
                    output.stderr.trim()
                ),
            });
        }
        Ok(output.stdout)
    }

    fn release(&self) -> Result<Release, UpdateError> {
        let body = self.curl(
            &format!(
                "https://api.github.com/repos/{}/releases/latest",
                self.repository
            ),
            None,
            CHECK_TIMEOUT,
        )?;
        serde_json::from_str::<Release>(&body).map_err(|error| UpdateError::Parse {
            detail: error.to_string(),
        })
    }

    /// Reads the newest release and compares it with `current`.
    pub fn check(&self, current: &Version) -> Result<CheckOutcome, UpdateError> {
        let release = self.release()?;
        let latest = Version::parse(&release.tag_name).ok_or_else(|| UpdateError::Parse {
            detail: format!("tag_name {:?} is not a version", release.tag_name),
        })?;

        if latest.is_newer_than(current) {
            Ok(CheckOutcome::Newer {
                current: current.clone(),
                latest,
                assets: release
                    .assets
                    .into_iter()
                    .map(|asset| ReleaseAssetView {
                        name: asset.name,
                        url: asset.browser_download_url,
                    })
                    .collect(),
            })
        } else {
            Ok(CheckOutcome::UpToDate {
                current: current.clone(),
                latest,
            })
        }
    }

    /// Downloads, verifies and swaps in the newest release.
    ///
    /// Returns the installed version. Anything short of a matching checksum stops before a
    /// single byte is written to the install path.
    pub fn install(&self, current: &Version, executable: &Path) -> Result<Version, UpdateError> {
        let CheckOutcome::Newer { latest, assets, .. } = self.check(current)? else {
            return Ok(current.clone());
        };

        let binary = assets
            .iter()
            .find(|asset| asset.name == BINARY_ASSET)
            .ok_or_else(|| UpdateError::NoBinaryAsset {
                available: assets
                    .iter()
                    .map(|asset| asset.name.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            })?;
        let checksums = assets
            .iter()
            .find(|asset| asset.name == CHECKSUM_ASSET)
            .ok_or(UpdateError::NoChecksum)?;

        std::fs::create_dir_all(&self.download_dir).map_err(|error| UpdateError::Install {
            detail: error.to_string(),
        })?;

        let checksums_body = self.curl(&checksums.url, None, CHECK_TIMEOUT)?;
        let expected = expected_checksum(&checksums_body, BINARY_ASSET).ok_or_else(|| {
            UpdateError::ChecksumMissingEntry {
                asset: BINARY_ASSET.to_string(),
            }
        })?;

        let downloaded = self.download_dir.join(format!("{BINARY_ASSET}-{latest}"));
        self.curl(&binary.url, Some(&downloaded), DOWNLOAD_TIMEOUT)?;

        let actual = self.sha256(&downloaded)?;
        if actual != expected {
            let _ = std::fs::remove_file(&downloaded);
            return Err(UpdateError::ChecksumMismatch { expected, actual });
        }

        self.swap_in(&downloaded, executable)?;
        Ok(latest)
    }

    fn sha256(&self, path: &Path) -> Result<String, UpdateError> {
        let spec = CommandSpec::capture("sha256sum", vec![path.display().to_string()])
            .with_timeout(CHECK_TIMEOUT);
        let output = self
            .runner
            .run(&spec)
            .map_err(|error| UpdateError::Install {
                detail: format!("sha256sum: {error}"),
            })?;
        if !output.success() {
            return Err(UpdateError::Install {
                detail: format!("sha256sum exited {:?}", output.exit_code),
            });
        }
        output
            .stdout
            .split_whitespace()
            .next()
            .map(|hash| hash.to_lowercase())
            .ok_or_else(|| UpdateError::Install {
                detail: "sha256sum printed nothing".to_string(),
            })
    }

    /// Writes the new binary beside the current one and renames it over, so the swap is
    /// atomic and the running process is not disturbed.
    fn swap_in(&self, downloaded: &Path, executable: &Path) -> Result<(), UpdateError> {
        use std::os::unix::fs::PermissionsExt;

        let directory = executable.parent().ok_or_else(|| UpdateError::Install {
            detail: format!("{} has no parent directory", executable.display()),
        })?;
        // A rename only works within one filesystem, so the staging file lives next to the
        // target rather than in the state directory.
        let staged = directory.join(format!(
            ".{}.new",
            executable
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "omarchy-control-center".to_string())
        ));

        let bytes = std::fs::copy(downloaded, &staged).map_err(|error| UpdateError::Install {
            detail: format!("{}: {error}", staged.display()),
        })?;
        if bytes == 0 {
            let _ = std::fs::remove_file(&staged);
            return Err(UpdateError::Install {
                detail: "the downloaded binary is empty".to_string(),
            });
        }
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).map_err(
            |error| {
                let _ = std::fs::remove_file(&staged);
                UpdateError::Install {
                    detail: format!("chmod: {error}"),
                }
            },
        )?;

        std::fs::rename(&staged, executable).map_err(|error| {
            let _ = std::fs::remove_file(&staged);
            UpdateError::Install {
                detail: format!("rename: {error}"),
            }
        })?;
        let _ = std::fs::remove_file(downloaded);
        Ok(())
    }
}

/// Reads the hash for `asset` out of a `sha256sum`-style file: `<hash>  <name>`.
pub fn expected_checksum(body: &str, asset: &str) -> Option<String> {
    for line in body.lines() {
        let mut parts = line.split_whitespace();
        let (Some(hash), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        let name = name.trim_start_matches('*');
        if name == asset
            && hash.len() == 64
            && hash.chars().all(|character| character.is_ascii_hexdigit())
        {
            return Some(hash.to_lowercase());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::test_support::FakeRunner;

    fn release_json(tag: &str) -> String {
        format!(
            r#"{{"tag_name":"{tag}","assets":[
                {{"name":"{BINARY_ASSET}","browser_download_url":"https://example.invalid/bin"}},
                {{"name":"{CHECKSUM_ASSET}","browser_download_url":"https://example.invalid/sums"}}
            ]}}"#
        )
    }

    const API: &str =
        "https://api.github.com/repos/TzzJokerzzT/omarchy-control-center/releases/latest";
    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn current() -> Version {
        Version::parse("0.2.0").expect("version")
    }

    #[test]
    fn parses_versions_with_and_without_a_leading_v() {
        assert_eq!(
            Version::parse("0.2.0"),
            Some(Version {
                major: 0,
                minor: 2,
                patch: 0,
                pre: None
            })
        );
        assert_eq!(
            Version::parse("v1.0.0"),
            Some(Version {
                major: 1,
                minor: 0,
                patch: 0,
                pre: None
            })
        );
        assert_eq!(Version::parse("  v1.2.3  "), Version::parse("1.2.3"));
        assert_eq!(Version::parse("nonsense"), None);
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert_eq!(Version::parse(""), None);
    }

    #[test]
    fn orders_versions_numerically_not_lexically() {
        let older = Version::parse("0.9.0").expect("v");
        let newer = Version::parse("0.10.0").expect("v");
        assert!(newer.is_newer_than(&older), "0.10.0 must beat 0.9.0");
        assert!(!older.is_newer_than(&newer));

        let release = Version::parse("1.0.0").expect("v");
        let candidate = Version::parse("1.0.0-rc1").expect("v");
        assert!(
            release.is_newer_than(&candidate),
            "a pre-release comes before its release"
        );
    }

    #[test]
    fn reports_a_newer_release() {
        let runner =
            FakeRunner::new().on(&format!("curl {}", api_args(API)), &release_json("v0.3.0"));
        let updater = Updater::new(&runner, std::env::temp_dir());
        let outcome = updater.check(&current()).expect("check");

        assert!(outcome.is_newer());
        assert_eq!(outcome.summary(), "v0.3.0 is available (running v0.2.0)");
        match outcome {
            CheckOutcome::Newer { latest, assets, .. } => {
                assert_eq!(latest.to_string(), "0.3.0");
                assert_eq!(assets.len(), 2);
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn reports_when_there_is_nothing_to_do() {
        let runner =
            FakeRunner::new().on(&format!("curl {}", api_args(API)), &release_json("v0.2.0"));
        let updater = Updater::new(&runner, std::env::temp_dir());
        let outcome = updater.check(&current()).expect("check");
        assert!(!outcome.is_newer());
        assert_eq!(outcome.summary(), "v0.2.0 is the newest release");
    }

    #[test]
    fn an_older_release_than_the_running_binary_is_not_an_update() {
        let runner =
            FakeRunner::new().on(&format!("curl {}", api_args(API)), &release_json("v0.1.0"));
        let updater = Updater::new(&runner, std::env::temp_dir());
        assert!(!updater.check(&current()).expect("check").is_newer());
    }

    #[test]
    fn a_missing_curl_is_reported_as_such() {
        let runner = FakeRunner::new();
        let updater = Updater::new(&runner, std::env::temp_dir());
        assert_eq!(updater.check(&current()).unwrap_err(), UpdateError::NoCurl);
    }

    #[test]
    fn an_unreadable_api_answer_is_a_parse_error() {
        let runner =
            FakeRunner::new().on(&format!("curl {}", api_args(API)), "<html>not json</html>");
        let updater = Updater::new(&runner, std::env::temp_dir());
        assert!(matches!(
            updater.check(&current()).unwrap_err(),
            UpdateError::Parse { .. }
        ));
    }

    #[test]
    fn a_failing_curl_is_a_network_error_not_a_silent_no_op() {
        let runner = FakeRunner::new().with_program("curl").on_exit(
            &format!("curl {}", api_args(API)),
            28,
            "",
            "curl: (28) timeout",
        );
        let updater = Updater::new(&runner, std::env::temp_dir());
        let error = updater.check(&current()).unwrap_err();
        assert!(
            matches!(error, UpdateError::Network { .. }),
            "unexpected: {error}"
        );
    }

    #[test]
    fn reads_the_hash_of_the_binary_out_of_a_checksum_file() {
        let body = format!("{HASH}  {BINARY_ASSET}\nffff  other-file\n");
        assert_eq!(
            expected_checksum(&body, BINARY_ASSET),
            Some(HASH.to_string())
        );
        assert_eq!(
            expected_checksum(&format!("{HASH} *{BINARY_ASSET}"), BINARY_ASSET),
            Some(HASH.to_string())
        );
        assert_eq!(expected_checksum(&body, "another-file"), None);
        assert_eq!(
            expected_checksum(&format!("notahash  {BINARY_ASSET}"), BINARY_ASSET),
            None
        );
    }

    #[test]
    fn refuses_to_install_without_a_checksum_asset() {
        let sandbox = std::env::temp_dir().join(format!("occ-update-{}", std::process::id()));
        let executable = sandbox.join("bin/omarchy-control-center");
        std::fs::create_dir_all(executable.parent().expect("dir")).expect("mkdir");
        std::fs::write(&executable, b"old").expect("write");

        let release = format!(
            r#"{{"tag_name":"v0.3.0","assets":[{{"name":"{BINARY_ASSET}","browser_download_url":"https://example.invalid/bin"}}]}}"#
        );
        let runner = FakeRunner::new().on(&format!("curl {}", api_args(API)), &release);
        let updater = Updater::new(&runner, sandbox.join("downloads"));
        let error = updater.install(&current(), &executable).unwrap_err();
        assert_eq!(error, UpdateError::NoChecksum);
        assert_eq!(
            std::fs::read(&executable).expect("read"),
            b"old",
            "nothing may change"
        );
        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn a_checksum_mismatch_stops_before_touching_the_installed_binary() {
        let sandbox = std::env::temp_dir().join(format!("occ-update-bad-{}", std::process::id()));
        let executable = sandbox.join("bin/omarchy-control-center");
        std::fs::create_dir_all(executable.parent().expect("dir")).expect("mkdir");
        std::fs::write(&executable, b"old").expect("write");
        let downloads = sandbox.join("downloads");
        std::fs::create_dir_all(&downloads).expect("mkdir");

        let runner = FakeRunner::new()
            .on(&format!("curl {}", api_args(API)), &release_json("v0.3.0"))
            .on(
                &format!("curl {}", api_args("https://example.invalid/sums")),
                &format!("{HASH}  {BINARY_ASSET}\n"),
            )
            .on(
                &format!(
                    "curl {}",
                    download_args(
                        "https://example.invalid/bin",
                        &downloads,
                        &format!("{BINARY_ASSET}-0.3.0")
                    )
                ),
                "",
            )
            .with_program("sha256sum")
            .on(
                &format!(
                    "sha256sum {}",
                    downloads.join(format!("{BINARY_ASSET}-0.3.0")).display()
                ),
                "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff  file\n",
            );

        let updater = Updater::new(&runner, downloads);
        let error = updater.install(&current(), &executable).unwrap_err();
        assert!(
            matches!(error, UpdateError::ChecksumMismatch { .. }),
            "unexpected: {error}"
        );
        assert_eq!(
            std::fs::read(&executable).expect("read"),
            b"old",
            "a mismatched download must never replace the binary"
        );
        let _ = std::fs::remove_dir_all(&sandbox);
    }

    /// The other half of this feature lives in the workflow: it decides what is published,
    /// and this module decides what is accepted. The two halves cannot be exercised together
    /// from a unit test, so the contract is pinned against the workflow text itself.
    const RELEASE_WORKFLOW: &str = include_str!("../../.github/workflows/release.yml");

    fn squashed(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn the_release_workflow_publishes_the_assets_this_updater_accepts() {
        for asset in [BINARY_ASSET, CHECKSUM_ASSET] {
            assert!(
                RELEASE_WORKFLOW.contains(asset),
                "the release workflow does not publish {asset}, so every download it offers \
                 would be refused"
            );
        }
    }

    #[test]
    fn the_release_workflow_runs_the_gate_before_publishing() {
        // A tag does not trigger ci.yml, which only listens to pushes and pull requests on
        // main. Without the gate here, a tag can publish code that never ran it.
        for check in [
            "cargo fmt --check",
            "cargo clippy --all-targets -- -D warnings",
            "cargo test --all-targets",
        ] {
            assert!(
                RELEASE_WORKFLOW.contains(check),
                "the release workflow never runs `{check}`"
            );
        }
    }

    #[test]
    fn a_prerelease_tag_is_published_as_a_prerelease() {
        // The release API's /releases/latest answers with the newest release that is not a
        // prerelease. Publishing `v0.4.0-rc1` as a normal release would offer it to everyone
        // running 0.3.0 as though it were a stable update.
        let workflow = squashed(RELEASE_WORKFLOW);
        assert!(
            workflow.contains(r#"case "${GITHUB_REF_NAME}" in *-*) prerelease="--prerelease" ;;"#),
            "the workflow must decide from the tag whether it is a prerelease"
        );
        assert!(
            workflow.contains(r#"gh release create "${GITHUB_REF_NAME}""#),
            "the release is created from the tag"
        );
        assert!(
            workflow.contains("$prerelease"),
            "the publish step must pass the prerelease decision through"
        );
    }

    fn api_args(url: &str) -> String {
        format!("--fail --silent --show-error --location --max-time 10 --proto =https {url}")
    }

    fn download_args(url: &str, directory: &Path, file: &str) -> String {
        format!(
            "--fail --silent --show-error --location --max-time 120 --proto =https --output {} -- {url}",
            directory.join(file).display()
        )
    }
}
