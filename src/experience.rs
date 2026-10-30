//! Plain-language guidance shared by the terminal UI and command line.
use crate::{
    control::{self, State},
    lifecycle,
    model::{Candidate, Session},
};
use anyhow::{Context, Result};
use std::path::Path;

pub fn summary(state: &State) -> String {
    let mut lines = vec![format!(
        "Selected: {}",
        state.selected.as_deref().unwrap_or("none")
    )];
    if let Some(active) = &state.active {
        let verified = active.process.as_ref().is_some_and(crate::process::alive);
        lines.push(format!(
            "{}: {}",
            if verified {
                "Running"
            } else {
                "Recorded active (process not verified)"
            },
            active.candidate.name
        ));
        lines.push(format!(
            "Active files: {}",
            active.candidate.source.display()
        ));
        if let Some(staged) = state.installed.get(&active.candidate.id) {
            lines.push(format!("Staged files: {}", staged.source.display()));
            lines.push(if staged.source!=active.candidate.source {"A different revision is staged, not active. Use Enter to review its switch plan."} else {"Active and staged revision paths match; local file edits are not checked."}.into());
        }
    } else {
        lines.push("Running: no managed shell recorded".into());
    }
    if let Some(trial) = &state.pending {
        lines.push(format!(
            "Trial: {} — {}s remaining. k / shellswitch keep; u / shellswitch revert.",
            trial.target.name,
            trial.deadline.saturating_sub(control::now())
        ));
    }
    if !state.disabled.is_empty() {
        lines.push(format!(
            "Disabled: {}",
            state
                .disabled
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines.join("\n")
}

pub fn guide(state: &State) -> String {
    let legacy = state
        .selected
        .as_ref()
        .and_then(|id| state.installed.get(id))
        .is_some_and(|c| !c.source.with_file_name("source.json").exists());
    let next = if state.pending.is_some() {
        "Check the desktop now. Run shellswitch keep to accept, or shellswitch revert to restore."
    } else if state.installed.is_empty() {
        "Start with shellswitch scan. Review a shell with shellswitch plan SHELL_ID.\nFor a local project, register a reviewed manifest with shellswitch install MANIFEST --payload PROJECT --yes."
    } else if state.selected.is_none() {
        "Choose a shell in the browser and press Enter to review the plan. Confirm with y, then k to keep or u to revert."
    } else if legacy {
        "Your selected package predates source tracking. Register a reviewed adapter against your editable checkout with shellswitch install MANIFEST --payload PROJECT --yes. This stages files only.
Use shellswitch plan SHELL_ID before activating them. The current desktop is unchanged."
    } else {
        "To load source changes: shellswitch update --yes. This starts a trial; use shellswitch keep only after checking your desktop.\nTo activate an already staged revision, use shellswitch switch SHELL_ID --yes."
    };
    format!(
        "{}\n\nNEXT STEP\n{}\n\nBROWSE AND SWITCH\nArrows choose a shell; / searches; Enter reviews a switch; y starts a 20-second trial.\nk keeps the trial; u reverts. Expiry restores the previous shell.\n\nUPDATE\nShift+U reviews an update of the highlighted shell from its remembered local source.\nIt stages files and starts a trial. It does not fetch Git changes, build, or download dependencies.\nLegacy packages need an explicit source via refresh --payload; never guess the checkout.\n\nSTART AT LOGIN\nTab to Autostart; Left/Right chooses compositor; Enter saves; Delete clears.\nSaving a login choice does not switch the current desktop.\n\nHELP AND RECOVERY\n? opens this help; d diagnoses; r rescans; q exits the browser.\nCommand line: shellswitch status --human, shellswitch doctor, shellswitch recover --yes.\nAdvanced keys: a adopt, x disable, l release hold, f repair, e recover.\nNiri switching requires explicit configuration enrollment; follow shellswitch enroll --help.\nNo shell is activated by opening this guide.",
        summary(state),
        next
    )
}

pub fn update(dir: &Path, id: &str, session: &Session, candidates: &[Candidate]) -> Result<()> {
    let state = {
        let store = control::Store::open(dir)?;
        store.load()?
    };
    let candidate = state
        .installed
        .get(id)
        .context("Shell is not registered; install a reviewed manifest first")?;
    control::update_plan(candidate, session, &state)?;
    let refreshed = lifecycle::refresh(dir, id, None)?;
    control::switch(dir, &refreshed, session, candidates)
}
