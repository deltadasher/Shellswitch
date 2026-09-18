use crate::{
    control::{self, State},
    discovery::{self, Report},
    lifecycle,
    model::Kind,
};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{backend::TestBackend, prelude::*, widgets::*};
use std::{io, path::PathBuf, time::Duration};
const CYAN: Color = Color::Rgb(100, 218, 222);
const MUTED: Color = Color::Rgb(138, 151, 170);
const GOLD: Color = Color::Rgb(239, 195, 104);
const BG: Color = Color::Rgb(14, 19, 29);
struct App {
    query: String,
    searching: bool,
    tab: usize,
    selected: usize,
    scroll: u16,
    message: String,
    confirm: Option<String>,
}
impl Default for App {
    fn default() -> Self {
        Self {
            query: String::new(),
            searching: false,
            tab: 0,
            selected: 0,
            scroll: 0,
            message: "Discovery is read-only. Enter previews the exact switch plan.".into(),
            confirm: None,
        }
    }
}
fn filtered(report: &Report, a: &App) -> Vec<usize> {
    report
        .candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            c.searchable().contains(&a.query.to_lowercase())
                && match a.tab {
                    1 => c.kind == Kind::Shell,
                    2 => c.kind == Kind::Component,
                    3 => c.kind == Kind::Session,
                    4 => c.kind == Kind::Candidate,
                    _ => true,
                }
        })
        .map(|(i, _)| i)
        .collect()
}
fn block(title: &str) -> Block<'_> {
    Block::bordered()
        .title(title)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(57, 75, 93)))
}
fn draw(f: &mut Frame, report: &Report, a: &App, state: &State) {
    f.render_widget(
        Block::default().style(Style::default().bg(BG).fg(Color::Rgb(222, 230, 238))),
        f.area(),
    );
    if f.area().width < 76 || f.area().height < 20 {
        f.render_widget(
            Paragraph::new(
                "Shellswitch needs a terminal at least 76 × 20.\nResize, or press q to exit.",
            )
            .block(block(" SHELLSWITCH ")),
            f.area(),
        );
        return;
    }
    let vertical = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(4),
    ])
    .margin(1)
    .split(f.area());
    let active = state
        .active
        .as_ref()
        .map(|r| r.candidate.name.as_str())
        .unwrap_or("none");
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" SHELLSWITCH ", Style::default().fg(CYAN).bold()),
            Span::styled(" / ", Style::default().fg(MUTED)),
            Span::raw(format!(
                "{} · {}",
                report.session.compositor, report.session.protocol
            )),
            Span::styled(
                format!(
                    "    selected: {} · running: {active}",
                    state.selected.as_deref().unwrap_or("none")
                ),
                Style::default().fg(GOLD),
            ),
        ]))
        .block(block(" DISCOVER / INSPECT / SWITCH ")),
        vertical[0],
    );
    f.render_widget(
        Paragraph::new(format!(
            " {}{}",
            if a.searching {
                "Search › "
            } else {
                "/ Search  "
            },
            a.query
        ))
        .block(block(&format!(
            " {} candidates · {} files · {} scan notes ",
            report.candidates.len(),
            report.files_visited,
            report.warnings.len()
        ))),
        vertical[1],
    );
    let cols = Layout::horizontal([
        Constraint::Length(18),
        Constraint::Percentage(32),
        Constraint::Min(25),
    ])
    .split(vertical[2]);
    let groups = [
        "All entries",
        "Shells",
        "Components",
        "Sessions",
        "Needs review",
    ];
    let items: Vec<_> = groups
        .iter()
        .enumerate()
        .map(|(i, s)| {
            ListItem::new(format!("{} {}", if a.tab == i { "▸" } else { " " }, s)).style(
                if a.tab == i {
                    Style::default().fg(CYAN).bold()
                } else {
                    Style::default().fg(MUTED)
                },
            )
        })
        .collect();
    f.render_widget(List::new(items).block(block(" COLLECTIONS ")), cols[0]);
    let visible = filtered(report, a);
    let items: Vec<_> = visible
        .iter()
        .map(|i| {
            let c = &report.candidates[*i];
            let mark = if state.disabled.contains(&c.id) {
                "×"
            } else if state.selected.as_deref() == Some(&c.id) {
                "✓"
            } else if !c.running_pids.is_empty() {
                "●"
            } else if report.session.compatibility(c).is_ok() {
                "◇"
            } else {
                "·"
            };
            ListItem::new(vec![
                Line::from(format!("{mark} {}", c.name)),
                Line::from(Span::styled(
                    format!("  {}", c.framework),
                    Style::default().fg(MUTED),
                )),
            ])
        })
        .collect();
    let mut liststate = ListState::default().with_selected(if visible.is_empty() {
        None
    } else {
        Some(a.selected.min(visible.len() - 1))
    });
    f.render_stateful_widget(
        List::new(items)
            .block(block(" DISCOVERED "))
            .highlight_style(Style::default().bg(Color::Rgb(30, 64, 76)).fg(CYAN).bold())
            .highlight_symbol("▎"),
        cols[1],
        &mut liststate,
    );
    let detail = if let Some(i) = visible.get(a.selected) {
        let c = &report.candidates[*i];
        format!(
            "{}\n{} · {:?}\nID {}\n\nCOMPATIBILITY\n{}\n\nSOURCE\n{}\n\nDETECTION EVIDENCE\n{}\n\nLAUNCH BACKEND\n{}\n\nRUNNING PIDS\n{:?}\n\n{}",
            c.name,
            c.framework,
            c.kind,
            c.id,
            report
                .session
                .compatibility(c)
                .map(|_| "No declared conflict. Runtime trial still required.".to_string())
                .unwrap_or_else(|e| e),
            c.source.display(),
            c.evidence
                .iter()
                .map(|e| format!("• {e}"))
                .collect::<Vec<_>>()
                .join("\n"),
            c.command(),
            c.running_pids,
            if c.declared {
                "Local manifest: review its commands before switching."
            } else {
                "Static detection is evidence, not a guarantee."
            }
        )
    } else {
        "No matches. Clear your search, change collection, or add --root /path/to/shells.".into()
    };
    f.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: false })
            .scroll((a.scroll, 0))
            .block(block(" INSPECT · PgUp / PgDn ")),
        cols[2],
    );
    let status = if let Some(p) = &state.pending {
        format!(
            "TRIAL: {}s left · k keep · u revert",
            p.deadline.saturating_sub(control::now())
        )
    } else {
        a.message.clone()
    };
    f.render_widget(Paragraph::new(vec![Line::from(Span::styled(status,Style::default().fg(GOLD))),Line::from(Span::styled("↑↓ select  Tab group  / search  Enter plan  x disable  l release  d doctor  f repair  e recover  r scan  q quit",Style::default().fg(MUTED)))]).wrap(Wrap{trim:true}).block(block(" CONTROL ")),vertical[3]);
    if let Some(text) = &a.confirm {
        let area = Rect::new(
            f.area().x + 5,
            f.area().y + 3,
            f.area().width.saturating_sub(10),
            f.area().height.saturating_sub(6),
        );
        f.render_widget(Clear, area);
        f.render_widget(
            Paragraph::new(format!("{text}\n\n[y] Confirm    [Esc] Cancel"))
                .wrap(Wrap { trim: false })
                .block(block(" REVIEW ACTION ").border_style(Style::default().fg(GOLD)))
                .style(Style::default().bg(BG)),
            area,
        );
    }
}
struct Cleanup;
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}
pub fn run(mut report: Report, roots: Vec<PathBuf>, dir: PathBuf) -> Result<()> {
    enable_raw_mode()?;
    let _cleanup = Cleanup;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut a = App::default();
    let mut pending_action = 's';
    let automatic_roots = roots.iter().any(|p| p == &discovery::config_home());
    let mut last_scan = std::time::Instant::now();
    loop {
        let state = {
            let store = control::Store::open(&dir)?;
            store.load()?
        };
        terminal.draw(|f| draw(f, &report, &a, &state))?;
        let refresh_due = last_scan.elapsed() >= Duration::from_secs(5)
            && a.confirm.is_none()
            && state.pending.is_none();
        let key = if refresh_due {
            crossterm::event::KeyEvent::new(
                KeyCode::Char('r'),
                crossterm::event::KeyModifiers::NONE,
            )
        } else {
            if !event::poll(Duration::from_millis(200))? {
                continue;
            }
            let Event::Key(key) = event::read()? else {
                continue;
            };
            key
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if a.searching {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => a.searching = false,
                KeyCode::Backspace => {
                    a.query.pop();
                }
                KeyCode::Char(c) => a.query.push(c),
                _ => {}
            }
            a.selected = 0;
            a.scroll = 0;
            continue;
        }
        let visible = filtered(&report, &a);
        let selected = visible.get(a.selected).copied();
        if a.confirm.is_some() {
            if key.code == KeyCode::Esc {
                a.confirm = None;
                continue;
            }
            if key.code == KeyCode::Char('y') {
                a.confirm = None;
                let result = match (pending_action, selected) {
                    ('s', Some(i)) => control::switch(
                        &dir,
                        &report.candidates[i],
                        &report.session,
                        &report.candidates,
                    ),
                    ('a', Some(i)) => {
                        let c = &report.candidates[i];
                        control::adopt(&dir, c, c.running_pids.first().copied())
                    }
                    ('x', Some(i)) => lifecycle::disable(&dir, &report.candidates[i].id),
                    ('l', Some(i)) => lifecycle::release(&dir, &report.candidates[i].id),
                    ('f', _) => lifecycle::repair(&dir),
                    ('e', _) => lifecycle::recover_files(&dir).and_then(|_| {
                        let pending = control::Store::open(&dir)?.load()?.pending.is_some();
                        if pending {
                            control::revert(&dir)
                        } else {
                            Ok(())
                        }
                    }),
                    _ => Ok(()),
                };
                a.message = match result {
                    Ok(()) => "Action complete. Trial switches require k to keep.".into(),
                    Err(e) => {
                        let detail = format!(
                            "Action failed:\n\n{e:#}\n\nPress Esc to close this diagnostic."
                        );
                        a.confirm = Some(detail);
                        "Action failed; diagnostic opened.".into()
                    }
                };
                crate::process::annotate(&mut report.candidates);
            }
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => break,
            KeyCode::Char('/') => a.searching = true,
            KeyCode::Tab => {
                a.tab = (a.tab + 1) % 5;
                a.selected = 0;
                a.scroll = 0;
            }
            KeyCode::BackTab => {
                a.tab = (a.tab + 4) % 5;
                a.selected = 0;
                a.scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                a.selected = (a.selected + 1).min(visible.len().saturating_sub(1));
                a.scroll = 0;
            }
            KeyCode::Up => {
                a.selected = a.selected.saturating_sub(1);
                a.scroll = 0;
            }
            KeyCode::PageDown => a.scroll = a.scroll.saturating_add(6),
            KeyCode::PageUp => a.scroll = a.scroll.saturating_sub(6),
            KeyCode::Char('r') => {
                let mut scan_roots = roots.clone();
                if automatic_roots {
                    scan_roots.extend(discovery::default_roots());
                }
                scan_roots.sort();
                scan_roots.dedup();
                report = discovery::scan(&scan_roots);
                last_scan = std::time::Instant::now();
                for c in lifecycle::registry(&dir)? {
                    if c.lifecycle.is_some() {
                        report
                            .candidates
                            .retain(|old| old.name != c.name || old.lifecycle.is_some());
                    }
                    report.candidates.retain(|old| old.id != c.id);
                    report.candidates.push(c);
                }
                crate::process::annotate(&mut report.candidates);
                a.message = format!("Scan complete. {}", report.warnings.join("; "));
            }
            KeyCode::Enter => {
                if let Some(i) = selected {
                    match control::plan(&report.candidates[i], &report.session, &state) {
                        Ok(plan) => {
                            pending_action = 's';
                            a.confirm = Some(plan);
                        }
                        Err(e) => a.message = e.to_string(),
                    };
                }
            }
            KeyCode::Char('a') => {
                if let Some(i) = selected {
                    let c = &report.candidates[i];
                    pending_action = 'a';
                    a.confirm = Some(format!(
                        "Adopt {}\nMatching PIDs: {:?}\n\n{}\n\nThis grants Shellswitch control of the existing shell. Its next switch will stop it, and rollback will use the launch command above.",
                        c.name,
                        c.running_pids,
                        c.command()
                    ));
                }
            }
            KeyCode::Char('x') => {
                pending_action = 'x';
                if let Some(i) = selected {
                    a.confirm = Some(format!(
                        "Stop {} and keep it disabled?\nOnly its owned processes and declared services will be stopped. Its supported startup paths stay gated.\n\nUse l to release the hold; release does not start it.",
                        report.candidates[i].name
                    ));
                }
            }
            KeyCode::Char('l') => {
                if let Some(i) = selected {
                    pending_action = 'l';
                    a.confirm = Some(format!(
                        "Release the emergency hold for {}? It remains inactive until you select it.",
                        report.candidates[i].name
                    ));
                }
            }
            KeyCode::Char('d') => {
                let findings = lifecycle::diagnostics(&dir, &state);
                pending_action = 'd';
                a.confirm = Some(if findings.is_empty() {
                    "No detected ownership or lifecycle drift.".into()
                } else {
                    findings
                        .iter()
                        .map(|d| format!("{}: {}\n→ {}", d.code, d.detail, d.repair))
                        .collect::<Vec<_>>()
                        .join("\n\n")
                });
            }
            KeyCode::Char('f') => {
                pending_action = 'f';
                a.confirm = Some("Archive overwritten contents and restore saved configuration and startup gates? This does not start a shell.".into());
            }
            KeyCode::Char('e') => {
                pending_action = 'e';
                a.confirm = Some("Recover interrupted file operations and restore the previous working switch state?".into());
            }
            KeyCode::Char('k') => {
                a.message = control::keep(&dir)
                    .map(|_| "Trial kept".into())
                    .unwrap_or_else(|e| e.to_string())
            }
            KeyCode::Char('u') => {
                a.message = control::revert(&dir)
                    .map(|_| "Previous shell restored".into())
                    .unwrap_or_else(|e| e.to_string())
            }
            _ => {}
        }
    }
    Ok(())
}
pub fn snapshot(report: &Report) -> Result<String> {
    let backend = TestBackend::new(120, 36);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| draw(f, report, &App::default(), &State::default()))?;
    let b = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..36 {
        for x in 0..120 {
            text.push_str(b[(x, y)].symbol());
        }
        text.push('\n');
    }
    Ok(text)
}
pub fn snapshot_svg(report: &Report) -> Result<String> {
    let mut terminal = Terminal::new(TestBackend::new(120, 36))?;
    terminal.draw(|f| draw(f, report, &App::default(), &State::default()))?;
    let buffer = terminal.backend().buffer();
    let mut svg = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="792" viewBox="0 0 1200 792"><rect width="1200" height="792" fill="#0e131d"/><g font-family="DejaVu Sans Mono,monospace" font-size="16">"##,
    );
    let rgb = |c: Color, default: &str| match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => default.into(),
    };
    for y in 0..36u16 {
        for x in 0..120u16 {
            let cell = &buffer[(x, y)];
            let bg = rgb(cell.bg, "#0e131d");
            if bg != "#0e131d" {
                svg.push_str(&format!(
                    r#"<rect x="{}" y="{}" width="10" height="22" fill="{}"/>"#,
                    x * 10,
                    y * 22,
                    bg
                ));
            }
            if cell.symbol().trim().is_empty() {
                continue;
            }
            let symbol = cell
                .symbol()
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            svg.push_str(&format!(
                r#"<text x="{}" y="{}" fill="{}">{}</text>"#,
                x * 10,
                y * 22 + 17,
                rgb(cell.fg, "#dee6ee"),
                symbol
            ));
        }
    }
    svg.push_str("</g></svg>");
    Ok(svg)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_tui_renders() {
        let r = Report {
            session: crate::model::Session {
                protocol: "wayland".into(),
                compositor: "niri".into(),
                globals: None,
                evidence: vec![],
            },
            candidates: vec![],
            warnings: vec![],
            files_visited: 0,
        };
        let s = snapshot(&r).unwrap();
        assert!(s.contains("SHELLSWITCH"));
        assert!(s.contains("No matches"));
    }
}
