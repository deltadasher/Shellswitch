use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Shell,
    Component,
    Session,
    Candidate,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Backend {
    Process { argv: Vec<String>, cwd: PathBuf },
    Systemd { unit: String },
    Review { reason: String },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub framework: String,
    pub source: PathBuf,
    pub evidence: Vec<String>,
    pub protocols: BTreeSet<String>,
    pub compositors: BTreeSet<String>,
    pub backend: Backend,
    #[serde(default)]
    pub required_globals: BTreeSet<String>,
    #[serde(default)]
    pub lifecycle: Option<crate::ownership::Lifecycle>,
    pub declared: bool,
    pub running_pids: Vec<u32>,
}
impl Candidate {
    pub fn searchable(&self) -> String {
        format!(
            "{} {} {:?} {} {}",
            self.name,
            self.framework,
            self.kind,
            self.source.display(),
            self.evidence.join(" ")
        )
        .to_lowercase()
    }
    pub fn command(&self) -> String {
        match &self.backend {
            Backend::Process { argv, cwd } => format!("cwd: {}\nargv: {:?}", cwd.display(), argv),
            Backend::Systemd { unit } => format!("systemctl --user start {unit}"),
            Backend::Review { reason } => format!("Review required: {reason}"),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub protocol: String,
    pub compositor: String,
    pub globals: Option<BTreeSet<String>>,
    pub evidence: Vec<String>,
}
impl Session {
    pub fn detect() -> Self {
        let env = |key| std::env::var(key).unwrap_or_default();
        let protocol = if !env("WAYLAND_DISPLAY").is_empty() {
            "wayland"
        } else if !env("DISPLAY").is_empty() {
            "x11"
        } else {
            "unknown"
        }
        .into();
        let mut evidence = vec![format!(
            "XDG_CURRENT_DESKTOP={}",
            env("XDG_CURRENT_DESKTOP")
        )];
        let compositor = [
            ("NIRI_SOCKET", "niri"),
            ("HYPRLAND_INSTANCE_SIGNATURE", "hyprland"),
            ("SWAYSOCK", "sway"),
        ]
        .into_iter()
        .find_map(|(key, value)| {
            if !env(key).is_empty() {
                evidence.push(format!("{key} present"));
                Some(value.to_owned())
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            env("XDG_CURRENT_DESKTOP")
                .split(':')
                .next()
                .unwrap_or_default()
                .to_lowercase()
        });
        let globals = if protocol == "wayland" {
            match crate::wayland::probe() {
                Ok(g) => {
                    evidence.push(format!("Wayland registry: {} advertised globals", g.len()));
                    Some(g)
                }
                Err(e) => {
                    evidence.push(format!("Wayland capability probe unavailable: {e}"));
                    None
                }
            }
        } else {
            None
        };
        Self {
            protocol,
            globals,
            compositor: if compositor.is_empty() {
                "unknown".into()
            } else {
                compositor
            },
            evidence,
        }
    }
    pub fn compatibility(&self, c: &Candidate) -> Result<(), String> {
        if c.kind == Kind::Session {
            return Err(
                "Full desktop session: select it at your login manager after logging out".into(),
            );
        }
        if !c.protocols.is_empty() && !c.protocols.contains(&self.protocol) {
            return Err(format!(
                "Requires {:?}; current session is {}",
                c.protocols, self.protocol
            ));
        }
        if !c.compositors.is_empty() && !c.compositors.contains(&self.compositor) {
            return Err(format!(
                "Requires {:?}; current compositor is {}",
                c.compositors, self.compositor
            ));
        }
        if self.protocol == "unknown" {
            return Err("No graphical session detected".into());
        }
        if !c.required_globals.is_empty() {
            let Some(globals) = &self.globals else {
                return Err("Required Wayland capabilities could not be verified; run inside the host graphical session".into());
            };
            let missing: Vec<_> = c.required_globals.difference(globals).collect();
            if !missing.is_empty() {
                return Err(format!(
                    "Compositor is missing required globals: {missing:?}"
                ));
            }
        }
        if let Backend::Review { reason } = &c.backend {
            return Err(reason.clone());
        }
        Ok(())
    }
}
pub fn stable_id(path: &std::path::Path) -> String {
    // Stable FNV-1a identifier, not a security or content hash.
    let mut h = 0xcbf29ce484222325u64;
    for b in path.as_os_str().as_encoded_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}
pub fn executable(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let valid = |p: &PathBuf| {
        p.is_file()
            && p.metadata()
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
    };
    if name.contains('/') {
        let p = PathBuf::from(name);
        return valid(&p).then_some(p);
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join(name))
        .find(valid)
}
