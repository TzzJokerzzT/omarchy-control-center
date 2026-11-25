//! Operation planning: what would happen, to what, and which facts are unknown.
//!
//! A plan is the only thing the executor accepts, and it carries a fingerprint of
//! every decision-relevant fact so a stale preview can be detected instead of
//! silently executed.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::domain::ComponentId;

/// The program a privileged command is wrapped in. The interface never runs as
/// root itself: it hands exactly one confirmed argv to `sudo`.
pub const SUDO: &str = "sudo";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Remove,
    Update,
    /// Freeing the space a leftover occupies by moving it into quarantine.
    Reclaim,
}

impl OperationKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Remove => "remove",
            Self::Update => "update",
            Self::Reclaim => "reclaim",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRequest {
    pub target: ComponentId,
    pub kind: OperationKind,
}

impl OperationRequest {
    pub fn new(target: ComponentId, kind: OperationKind) -> Self {
        Self { target, kind }
    }
}

/// One exact process invocation. `program` and `args` are separate fields on
/// purpose: nothing is ever joined into a shell string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedCommand {
    pub program: String,
    pub args: Vec<String>,
    pub requires_root: bool,
    /// Needs the controlling terminal (privilege prompt or interactive output).
    pub requires_tty: bool,
    /// The command asks for privileges itself: a package helper with its own `sudo`
    /// prompt, or a tool that goes through polkit. Wrapping such a command in `sudo`
    /// would hand a build helper root instead of letting it ask, so the interface only
    /// elevates a command that cannot elevate itself.
    ///
    /// An in-memory decision only: exporting it would change the plan JSON every
    /// `--print-plan` consumer already parses, so it is never serialized.
    #[serde(skip)]
    pub self_elevating: bool,
    pub rationale: String,
}

impl PlannedCommand {
    /// True when the interface must obtain root on the command's behalf.
    pub fn needs_interface_privileges(&self) -> bool {
        self.requires_root && !self.self_elevating
    }

    /// Display-only rendering. Never executed: the executor uses `program` and
    /// `args` verbatim, one `execve` per command.
    pub fn display(&self) -> String {
        let mut out = self.program.clone();
        for arg in &self.args {
            out.push(' ');
            out.push_str(arg);
        }
        out
    }

    /// What actually runs. The interface never runs as root, so a privileged
    /// command is invoked through `sudo --`, and the preview must say so instead of
    /// promising an argv the machine would refuse.
    pub fn display_effective(&self) -> String {
        if self.needs_interface_privileges() {
            format!("{SUDO} -- {}", self.display())
        } else {
            self.display()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationPlan {
    pub adapter: String,
    pub target: ComponentId,
    pub target_name: String,
    pub target_version: Option<String>,
    pub kind: OperationKind,
    pub commands: Vec<PlannedCommand>,
    /// Paths a reclaim would move. Empty for every other kind of operation.
    #[serde(default)]
    pub reclaim: Vec<crate::domain::ReclaimTarget>,
    /// Components that could be affected beyond the target itself.
    pub affected: Vec<String>,
    /// Reasons the operation must not run. Never executed while non-empty.
    pub blocking: Vec<String>,
    /// Facts the source could not determine, shown instead of guessed.
    pub unknown: Vec<String>,
    pub notes: Vec<String>,
}

impl OperationPlan {
    pub fn is_executable(&self) -> bool {
        if !self.blocking.is_empty() {
            return false;
        }
        match self.kind {
            OperationKind::Reclaim => !self.reclaim.is_empty() && self.commands.is_empty(),
            _ => !self.commands.is_empty(),
        }
    }

    /// Total bytes the reclaim would take out of its original location.
    pub fn reclaim_size(&self) -> u64 {
        self.reclaim.iter().map(|target| target.size).sum()
    }

    /// True when the terminal must be handed back before the first command runs:
    /// either a command is interactive or it asks `sudo` for privileges, and either
    /// way the interface must stop drawing over it.
    pub fn requires_handover(&self) -> bool {
        self.commands
            .iter()
            .any(|command| command.requires_tty || command.requires_root)
    }

    /// Exact phrase the human must type to confirm: the native identifier, never
    /// a generic "y" or "continue".
    pub fn confirmation_phrase(&self) -> &str {
        &self.target.native_id
    }

    /// Hash of every fact that would make the plan different if it was recomputed.
    /// Two plans that would touch different things never share a fingerprint.
    pub fn fingerprint(&self) -> String {
        let mut parts: Vec<String> = vec![
            self.adapter.clone(),
            self.target.to_string(),
            self.kind.label().to_string(),
        ];
        for command in &self.commands {
            parts.push(command.program.clone());
            parts.extend(command.args.iter().cloned());
        }
        for target in &self.reclaim {
            parts.push(target.path.display().to_string());
            parts.push(target.root.display().to_string());
            parts.push(target.size.to_string());
        }
        let mut affected = self.affected.clone();
        affected.sort();
        parts.extend(affected);
        parts.extend(self.blocking.iter().cloned());

        let mut hasher = DefaultHasher::new();
        for part in &parts {
            part.hash(&mut hasher);
            0u8.hash(&mut hasher);
        }
        format!("{:016x}", hasher.finish())
    }
}

/// What actually happened, including the exit code we must never round to success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationOutcome {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub command_display: String,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub summary: String,
}

/// Keeps the end of a command output, which is where the error is, without
/// breaking a UTF-8 boundary.
pub fn tail_text(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let start = text.len() - max;
    let start = (start..text.len())
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or(text.len());
    format!("…{}", &text[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(args: &[&str]) -> PlannedCommand {
        PlannedCommand {
            program: "pacman".to_string(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            requires_root: true,
            requires_tty: true,
            self_elevating: false,
            rationale: "remove the package".to_string(),
        }
    }

    fn plan(blocking: Vec<String>, affected: Vec<String>) -> OperationPlan {
        OperationPlan {
            adapter: "pacman".to_string(),
            target: ComponentId::new("pacman", "btop"),
            target_name: "btop".to_string(),
            target_version: Some("1.4.0-1".to_string()),
            kind: OperationKind::Remove,
            commands: vec![command(&["-R", "btop"])],
            reclaim: Vec::new(),
            affected,
            blocking,
            unknown: Vec::new(),
            notes: Vec::new(),
        }
    }

    #[test]
    fn display_is_never_an_execution_string() {
        let command = command(&["-R", "btop"]);
        assert_eq!(command.display(), "pacman -R btop");
        assert_eq!(command.args, vec!["-R".to_string(), "btop".to_string()]);
    }

    #[test]
    fn a_privileged_command_displays_the_sudo_prefix_that_will_run() {
        let command = command(&["-R", "btop"]);
        assert_eq!(command.display_effective(), "sudo -- pacman -R btop");
    }

    #[test]
    fn a_command_that_asks_for_itself_is_never_wrapped_in_sudo() {
        // yay removes through pacman but prompts for the password itself; running it
        // under sudo would hand the helper root instead of letting it ask.
        let mut helper = command(&["-R", "some-aur-pkg"]);
        helper.self_elevating = true;
        assert!(helper.requires_root);
        assert!(!helper.needs_interface_privileges());
        assert_eq!(helper.display_effective(), "pacman -R some-aur-pkg");
    }

    #[test]
    fn an_unprivileged_command_displays_its_own_argv() {
        let mut command = command(&["uninstall", "org.x.Y"]);
        command.requires_root = false;
        assert_eq!(command.display_effective(), "pacman uninstall org.x.Y");
    }

    #[test]
    fn the_elevation_flag_is_kept_in_memory_and_never_exported() {
        // The flag decides how the command runs, so it must survive in memory...
        let mut helper = command(&["-R", "some-aur-pkg"]);
        helper.self_elevating = true;
        assert!(!helper.needs_interface_privileges());
        assert_eq!(helper.display_effective(), "pacman -R some-aur-pkg");

        // ...but the exported plan shape stays exactly what it was before the field existed.
        let json = serde_json::to_value(&helper).expect("serialize");
        let object = json.as_object().expect("an object");
        assert!(
            object.get("self_elevating").is_none(),
            "the flag must not reach the plan export: {json}"
        );
        assert_eq!(object.len(), 5, "unexpected plan shape: {json}");

        // A plan written before the field existed still loads, defaulting to "elevate".
        let legacy = serde_json::json!({
            "program": "pacman",
            "args": ["-R", "btop"],
            "requires_root": true,
            "requires_tty": true,
            "rationale": "remove the package",
        });
        let restored: PlannedCommand = serde_json::from_value(legacy).expect("deserialize");
        assert!(!restored.self_elevating);
        assert!(restored.needs_interface_privileges());
    }

    #[test]
    fn a_root_command_needs_the_terminal_handover_even_without_a_tty_prompt() {
        let mut root_without_tty = plan(Vec::new(), Vec::new());
        root_without_tty.commands[0].requires_tty = false;
        assert!(root_without_tty.requires_handover());

        let mut plain = plan(Vec::new(), Vec::new());
        plain.commands[0].requires_root = false;
        plain.commands[0].requires_tty = false;
        assert!(!plain.requires_handover());
    }

    #[test]
    fn a_blocked_plan_is_never_executable() {
        let blocked = plan(vec!["required by: hyprland".to_string()], Vec::new());
        assert!(!blocked.is_executable());

        let clean = plan(Vec::new(), Vec::new());
        assert!(clean.is_executable());
    }

    #[test]
    fn confirmation_phrase_is_the_native_identifier() {
        assert_eq!(plan(Vec::new(), Vec::new()).confirmation_phrase(), "btop");
    }

    #[test]
    fn fingerprint_changes_when_the_plan_would_touch_something_else() {
        let base = plan(Vec::new(), vec!["libfoo".to_string()]);
        let same = plan(Vec::new(), vec!["libfoo".to_string()]);
        let more_affected = plan(Vec::new(), vec!["libfoo".to_string(), "libbar".to_string()]);
        let now_blocked = plan(
            vec!["required by: hyprland".to_string()],
            vec!["libfoo".to_string()],
        );

        assert_eq!(base.fingerprint(), same.fingerprint());
        assert_ne!(base.fingerprint(), more_affected.fingerprint());
        assert_ne!(base.fingerprint(), now_blocked.fingerprint());
    }

    #[test]
    fn fingerprint_ignores_display_order_of_affected_components() {
        let first = plan(Vec::new(), vec!["a".to_string(), "b".to_string()]);
        let second = plan(Vec::new(), vec!["b".to_string(), "a".to_string()]);
        assert_eq!(first.fingerprint(), second.fingerprint());
    }
}
