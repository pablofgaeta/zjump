//! An fzf-style fuzzy jumper over every tab in every zellij session.
//!
//! The premise being tested is that zellij's plugin API already exposes enough
//! to do this in-process, with no companion CLI and no shelling out to `fzf`:
//!
//! * `get_session_list()` returns every live session with its tabs and panes.
//! * `Event::SessionUpdate` pushes that same data on change.
//! * `switch_session_with_focus()` jumps to a (session, tab) in one call.
//!
//! Sibling-session data only refreshes when *some* plugin asks for it, so this
//! polls `get_session_list()` on a timer the way `status-bar` does.

mod matcher;

use std::collections::{BTreeMap, HashMap};
use std::fs;

use ansi_term::{Colour, Style};
use zellij_tile::prelude::*;

use crate::matcher::match_query;

/// Sibling sessions go stale unless a plugin asks; matches status-bar's cadence.
const SESSION_POLL_SECS: f64 = 2.0;
const MARKS_PATH: &str = "/cache/zjump-marks.tsv";

#[derive(Default)]
struct State {
    targets: Vec<Target>,
    marks: Vec<Mark>,
    query: String,
    /// Index into the *filtered* list, not into `targets`.
    selected: usize,
    permissions_granted: bool,
    launcher: Launcher,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum Launcher {
    #[default]
    Search,
    Marks,
}

/// One jump destination: a tab inside a session, optionally with a focused pane.
#[derive(Clone)]
struct Target {
    session: String,
    tab_position: usize,
    label: String,
    is_current_session: bool,
    is_active_tab: bool,
    panes: usize,
    focused_pane: Option<(u32, bool)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Mark {
    session: String,
    tab_position: usize,
    pane_id: Option<(u32, bool)>,
    label: String,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.launcher = match configuration.get("mode").map(String::as_str) {
            Some("marks") => Launcher::Marks,
            _ => Launcher::Search,
        };
        self.marks = load_marks();
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
        ]);
        subscribe(&[EventType::PermissionRequestResult]);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(PermissionStatus::Granted) => {
                self.permissions_granted = true;
                // Selectable so the picker receives keys while focused.
                set_selectable(true);
                subscribe(&[EventType::SessionUpdate, EventType::Key, EventType::Timer]);
                self.poll_sessions();
                set_timeout(SESSION_POLL_SECS);
                true
            }
            Event::PermissionRequestResult(_) => {
                set_selectable(false);
                true
            }
            Event::Timer(_) => {
                self.poll_sessions();
                set_timeout(SESSION_POLL_SECS);
                true
            }
            Event::SessionUpdate(sessions, _resurrectable) => {
                self.rebuild_targets(sessions);
                true
            }
            Event::Key(key) => self.handle_key(key),
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        if !self.permissions_granted {
            print!("zjump: waiting for permission");
            return;
        }

        match self.launcher {
            Launcher::Search => self.render_search(rows, cols),
            Launcher::Marks => self.render_marks(rows, cols),
        }
    }
}

impl State {
    fn render_search(&self, rows: usize, cols: usize) {
        let matches = self.matches();
        let dim = Style::new().fg(Colour::Fixed(8));
        println!(
            "  {} {}{}  {}",
            Colour::Cyan.bold().paint(">"),
            self.query,
            dim.paint("▏"),
            dim.paint(format!("{}/{}", matches.len(), self.targets.len()))
        );

        // One row for the query line, one for the footer hint.
        let list_rows = rows.saturating_sub(2);
        let start = scroll_start(self.selected, matches.len(), list_rows);

        for (position, (index, indices)) in matches.iter().enumerate().skip(start).take(list_rows) {
            let target = &self.targets[*index];
            println!(
                "{}",
                target.render(indices, position == self.selected, cols)
            );
        }

        print!("{}", dim.paint("  ↑↓/^p^n move · enter jump · esc close"));
    }

    fn render_marks(&self, rows: usize, cols: usize) {
        let dim = Style::new().fg(Colour::Fixed(8));
        println!(
            "  {} marks {}",
            Colour::Cyan.bold().paint("zjump"),
            dim.paint(format!("{} saved", self.marks.len()))
        );

        let list_rows = rows.saturating_sub(2);
        let start = scroll_start(self.selected, self.marks.len(), list_rows);

        for (position, mark) in self.marks.iter().enumerate().skip(start).take(list_rows) {
            println!("{}", mark.render(position == self.selected, cols));
        }

        print!(
            "{}",
            dim.paint(
                "  j/k move · enter jump · a add current · d delete · ^j/^k reorder · esc close"
            )
        );
    }

    fn poll_sessions(&mut self) {
        // Both refreshes the local view and prompts zellij to rescan siblings,
        // which is what makes the pushed SessionUpdate events accurate.
        if let Ok(snapshot) = get_session_list() {
            self.rebuild_targets(snapshot.live_sessions);
        }
    }

    fn rebuild_targets(&mut self, sessions: Vec<SessionInfo>) {
        let previous = self
            .selected_target()
            .map(|target| (target.session.clone(), target.tab_position));

        self.targets = sessions
            .into_iter()
            .flat_map(|session| {
                let panes_by_tab: HashMap<usize, Vec<PaneInfo>> = session.panes.panes.clone();
                let pane_counts: HashMap<usize, usize> = panes_by_tab
                    .iter()
                    .map(|(tab_position, panes)| {
                        let count = panes.iter().filter(|pane| !pane.is_plugin).count();
                        (*tab_position, count)
                    })
                    .collect();
                let name = session.name;
                let is_current_session = session.is_current_session;

                session.tabs.into_iter().map(move |tab| {
                    let focused_pane = panes_by_tab.get(&tab.position).and_then(|panes| {
                        panes
                            .iter()
                            .find(|pane| pane.is_focused && !pane.is_plugin)
                            .map(|pane| (pane.id, pane.is_plugin))
                    });
                    Target {
                        label: format!("{}/{}", name, tab.name),
                        session: name.clone(),
                        is_current_session,
                        is_active_tab: tab.active,
                        panes: pane_counts.get(&tab.position).copied().unwrap_or(0),
                        tab_position: tab.position,
                        focused_pane,
                    }
                })
            })
            .collect();

        // Current session last: the whole point is jumping somewhere else.
        self.targets.sort_by(|left, right| {
            left.is_current_session
                .cmp(&right.is_current_session)
                .then_with(|| left.label.cmp(&right.label))
        });

        // Keep the cursor on the same destination across a refresh where possible.
        self.selected = previous
            .and_then(|(session, tab_position)| {
                self.matches().iter().position(|(index, _)| {
                    let target = &self.targets[*index];
                    target.session == session && target.tab_position == tab_position
                })
            })
            .unwrap_or(0)
            .min(self.visible_len().saturating_sub(1));
    }

    /// Filtered target indices with their highlight positions, best first.
    ///
    /// `sort_by` is stable, so an empty query (every score 0) preserves the
    /// ordering established in `rebuild_targets`.
    fn matches(&self) -> Vec<(usize, Vec<usize>)> {
        let mut scored: Vec<(i32, usize, Vec<usize>)> = self
            .targets
            .iter()
            .enumerate()
            .filter_map(|(index, target)| {
                match_query(&target.label, &self.query)
                    .map(|matched| (matched.score, index, matched.indices))
            })
            .collect();

        scored.sort_by(|left, right| right.0.cmp(&left.0));
        scored
            .into_iter()
            .map(|(_, index, indices)| (index, indices))
            .collect()
    }

    fn selected_target(&self) -> Option<&Target> {
        let matches = self.matches();
        let (index, _) = matches.get(self.selected)?;
        self.targets.get(*index)
    }

    fn selected_mark(&self) -> Option<&Mark> {
        self.marks.get(self.selected)
    }

    fn current_target(&self) -> Option<Target> {
        self.targets
            .iter()
            .find(|target| target.is_current_session && target.is_active_tab)
            .cloned()
    }

    fn handle_key(&mut self, key: KeyWithModifier) -> bool {
        let ctrl = key.key_modifiers.contains(&KeyModifier::Ctrl);
        if ctrl && matches!(key.bare_key, BareKey::Char('c')) {
            close_self();
            return false;
        }
        match self.launcher {
            Launcher::Search => self.handle_search_key(key, ctrl),
            Launcher::Marks => self.handle_marks_key(key, ctrl),
        }
    }

    fn handle_search_key(&mut self, key: KeyWithModifier, ctrl: bool) -> bool {
        match key.bare_key {
            BareKey::Esc => {
                close_self();
                false
            }
            BareKey::Enter => {
                self.jump_selected_target();
                false
            }
            BareKey::Down | BareKey::Tab => self.move_selection(1),
            BareKey::Up => self.move_selection(-1),
            BareKey::Char('n') if ctrl => self.move_selection(1),
            BareKey::Char('p') if ctrl => self.move_selection(-1),
            BareKey::Char('u') if ctrl => {
                self.query.clear();
                self.selected = 0;
                true
            }
            BareKey::Backspace => {
                self.query.pop();
                self.selected = 0;
                true
            }
            BareKey::Char(character) if !ctrl => {
                self.query.push(character);
                self.selected = 0;
                true
            }
            _ => false,
        }
    }

    fn handle_marks_key(&mut self, key: KeyWithModifier, ctrl: bool) -> bool {
        match key.bare_key {
            BareKey::Esc => {
                close_self();
                false
            }
            BareKey::Enter => {
                self.jump_selected_mark();
                false
            }
            BareKey::Down | BareKey::Char('j') if !ctrl => self.move_selection(1),
            BareKey::Up | BareKey::Char('k') if !ctrl => self.move_selection(-1),
            BareKey::Char('j') if ctrl => self.reorder_mark(1),
            BareKey::Char('k') if ctrl => self.reorder_mark(-1),
            BareKey::Char('a') if !ctrl => self.add_current_mark(),
            BareKey::Char('d') if !ctrl => self.delete_selected_mark(),
            _ => false,
        }
    }

    fn move_selection(&mut self, delta: isize) -> bool {
        let len = self.visible_len();
        if len == 0 {
            return false;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(len as isize) as usize;
        true
    }

    fn visible_len(&self) -> usize {
        match self.launcher {
            Launcher::Search => self.matches().len(),
            Launcher::Marks => self.marks.len(),
        }
    }

    fn jump_selected_target(&mut self) {
        let Some(target) = self.selected_target().cloned() else {
            return;
        };
        jump_to(
            &target.session,
            target.tab_position,
            None,
            target.is_current_session,
        );
        close_self();
    }

    fn jump_selected_mark(&mut self) {
        let Some(mark) = self.selected_mark().cloned() else {
            return;
        };
        let is_current_session = self
            .targets
            .iter()
            .any(|target| target.session == mark.session && target.is_current_session);
        jump_to(
            &mark.session,
            mark.tab_position,
            mark.pane_id,
            is_current_session,
        );
        close_self();
    }

    fn add_current_mark(&mut self) -> bool {
        let Some(target) = self.current_target() else {
            return false;
        };
        let mark = Mark {
            session: target.session,
            tab_position: target.tab_position,
            pane_id: target.focused_pane,
            label: target.label,
        };
        if let Some(position) = self.marks.iter().position(|existing| existing == &mark) {
            self.selected = position;
            return true;
        }
        self.marks.push(mark);
        self.selected = self.marks.len().saturating_sub(1);
        save_marks(&self.marks);
        true
    }

    fn delete_selected_mark(&mut self) -> bool {
        if self.marks.is_empty() {
            return false;
        }
        self.marks.remove(self.selected);
        self.selected = self.selected.min(self.marks.len().saturating_sub(1));
        save_marks(&self.marks);
        true
    }

    fn reorder_mark(&mut self, delta: isize) -> bool {
        if self.marks.len() < 2 {
            return false;
        }
        let next = (self.selected as isize + delta).rem_euclid(self.marks.len() as isize) as usize;
        self.marks.swap(self.selected, next);
        self.selected = next;
        save_marks(&self.marks);
        true
    }
}

fn jump_to(
    session: &str,
    tab_position: usize,
    pane_id: Option<(u32, bool)>,
    is_current_session: bool,
) {
    if is_current_session && pane_id.is_none() {
        // switch_tab_to is 1-indexed; TabInfo::position is 0-indexed.
        switch_tab_to(tab_position as u32 + 1);
    } else {
        switch_session_with_focus(session, Some(tab_position), pane_id);
    }
}

impl Target {
    fn render(&self, indices: &[usize], selected: bool, cols: usize) -> String {
        let suffix = self.suffix();
        let marker = if selected { "▌ " } else { "  " };
        // Budget in visible chars: marker, label, suffix. Styling adds bytes
        // but no width, so the arithmetic stays in char space.
        let budget = cols
            .saturating_sub(marker.chars().count())
            .saturating_sub(suffix.chars().count());

        let base = if selected {
            Style::new().fg(Colour::White).bold()
        } else {
            Style::new().fg(Colour::Fixed(250))
        };
        let hit = Style::new().fg(Colour::Cyan).bold();

        let mut out = String::from(marker);
        for (position, character) in self.label.chars().take(budget).enumerate() {
            let style = if indices.contains(&position) {
                hit
            } else {
                base
            };
            out.push_str(&style.paint(character.to_string()).to_string());
        }
        if self.label.chars().count() > budget {
            out.push('…');
        }
        if !suffix.is_empty() {
            out.push_str(&Style::new().fg(Colour::Fixed(8)).paint(suffix).to_string());
        }
        out
    }

    fn suffix(&self) -> String {
        let mut suffix = String::new();
        if self.is_active_tab {
            suffix.push_str(" ·active");
        }
        if self.panes > 1 {
            suffix.push_str(&format!(" ·{}p", self.panes));
        }
        suffix
    }
}

impl Mark {
    fn render(&self, selected: bool, cols: usize) -> String {
        let suffix = if self.pane_id.is_some() {
            " ·pane"
        } else {
            ""
        };
        let marker = if selected { "▌ " } else { "  " };
        let budget = cols
            .saturating_sub(marker.chars().count())
            .saturating_sub(suffix.chars().count());
        let base = if selected {
            Style::new().fg(Colour::White).bold()
        } else {
            Style::new().fg(Colour::Fixed(250))
        };

        let mut out = String::from(marker);
        for character in self.label.chars().take(budget) {
            out.push_str(&base.paint(character.to_string()).to_string());
        }
        if self.label.chars().count() > budget {
            out.push('…');
        }
        if !suffix.is_empty() {
            out.push_str(&Style::new().fg(Colour::Fixed(8)).paint(suffix).to_string());
        }
        out
    }

    fn serialize(&self) -> String {
        let (pane_id, pane_is_plugin) = self
            .pane_id
            .map(|(id, is_plugin)| (id.to_string(), is_plugin.to_string()))
            .unwrap_or_else(|| (String::new(), String::new()));
        [
            escape_field(&self.session),
            self.tab_position.to_string(),
            pane_id,
            pane_is_plugin,
            escape_field(&self.label),
        ]
        .join("\t")
    }

    fn deserialize(line: &str) -> Option<Self> {
        let mut fields = line.split('\t');
        let session = unescape_field(fields.next()?)?;
        let tab_position = fields.next()?.parse().ok()?;
        let pane_id = match (fields.next()?, fields.next()?) {
            ("", "") => None,
            (id, is_plugin) => Some((id.parse().ok()?, is_plugin.parse().ok()?)),
        };
        let label = unescape_field(fields.next()?)?;
        Some(Self {
            session,
            tab_position,
            pane_id,
            label,
        })
    }
}

fn load_marks() -> Vec<Mark> {
    fs::read_to_string(MARKS_PATH)
        .map(|contents| contents.lines().filter_map(Mark::deserialize).collect())
        .unwrap_or_default()
}

fn save_marks(marks: &[Mark]) {
    let contents = marks
        .iter()
        .map(Mark::serialize)
        .collect::<Vec<_>>()
        .join("\n");
    let _ = fs::write(MARKS_PATH, contents);
}

fn escape_field(raw: &str) -> String {
    raw.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

fn unescape_field(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match chars.next()? {
            '\\' => out.push('\\'),
            't' => out.push('\t'),
            'n' => out.push('\n'),
            _ => return None,
        }
    }
    Some(out)
}

/// First visible row, so the cursor stays on screen.
fn scroll_start(selected: usize, len: usize, rows: usize) -> usize {
    if rows == 0 || len <= rows {
        return 0;
    }
    selected.saturating_sub(rows / 2).min(len - rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(label: &str) -> Target {
        Target {
            session: "s".into(),
            tab_position: 0,
            label: label.into(),
            is_current_session: false,
            is_active_tab: false,
            panes: 0,
            focused_pane: None,
        }
    }

    fn mark(label: &str) -> Mark {
        Mark {
            session: "s".into(),
            tab_position: 0,
            pane_id: None,
            label: label.into(),
        }
    }

    #[test]
    fn window_shows_everything_when_it_fits() {
        assert_eq!(scroll_start(0, 3, 10), 0);
        assert_eq!(scroll_start(2, 3, 3), 0);
    }

    #[test]
    fn window_centres_the_cursor_and_clamps_at_both_ends() {
        assert_eq!(scroll_start(10, 100, 10), 5);
        assert_eq!(scroll_start(99, 100, 10), 90);
        assert_eq!(scroll_start(0, 100, 10), 0);
    }

    #[test]
    fn window_handles_a_zero_height_pane() {
        assert_eq!(scroll_start(0, 10, 0), 0);
    }

    #[test]
    fn empty_query_keeps_target_order() {
        let state = State {
            targets: vec![target("a/one"), target("b/two")],
            ..Default::default()
        };
        let order: Vec<usize> = state.matches().iter().map(|(index, _)| *index).collect();
        assert_eq!(order, vec![0, 1]);
    }

    #[test]
    fn query_filters_and_ranks() {
        let state = State {
            targets: vec![
                target("aspects/x"),
                target("agent/status"),
                target("zzz/zzz"),
            ],
            query: "as".into(),
            ..Default::default()
        };
        let order: Vec<usize> = state.matches().iter().map(|(index, _)| *index).collect();
        assert_eq!(order, vec![1, 0], "word-boundary match should rank first");
    }

    #[test]
    fn truncates_a_label_that_overflows_the_pane() {
        let rendered = target("very-long-session/very-long-tab-name").render(&[], false, 14);
        assert!(rendered.contains('…'));
    }

    #[test]
    fn mark_round_trip_escapes_separators() {
        let original = Mark {
            session: "a\tb".into(),
            tab_position: 2,
            pane_id: Some((7, false)),
            label: "x\\y\nz".into(),
        };
        assert_eq!(Mark::deserialize(&original.serialize()), Some(original));
    }

    #[test]
    fn mark_reorder_wraps() {
        let mut state = State {
            launcher: Launcher::Marks,
            marks: vec![mark("a"), mark("b")],
            selected: 0,
            ..Default::default()
        };
        assert!(state.reorder_mark(-1));
        assert_eq!(state.selected, 1);
        assert_eq!(state.marks[1].label, "a");
    }
}
