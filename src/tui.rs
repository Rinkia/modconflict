//! The ratatui front end: conflict list on the left, detail on the right.
//!
//! All state lives in `App`, which knows nothing about a terminal — so the
//! navigation and filtering logic is unit-tested without drawing anything.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::model::{Conflict, Severity};

pub struct App {
    conflicts: Vec<Conflict>,
    /// Indices into `conflicts` that pass the current filter.
    visible: Vec<usize>,
    selected: usize,
    filter: String,
    editing_filter: bool,
    /// Hide anything below this level.
    min_severity: Severity,
    quit: bool,
}

impl App {
    pub fn new(conflicts: Vec<Conflict>) -> App {
        let mut app = App {
            conflicts,
            visible: Vec::new(),
            selected: 0,
            filter: String::new(),
            editing_filter: false,
            min_severity: Severity::Info,
            quit: false,
        };
        app.refresh_visible();
        app
    }

    fn refresh_visible(&mut self) {
        let needle = self.filter.to_lowercase();
        self.visible = self
            .conflicts
            .iter()
            .enumerate()
            .filter(|(_, c)| c.severity() >= self.min_severity)
            .filter(|(_, c)| {
                needle.is_empty()
                    || c.title().to_lowercase().contains(&needle)
                    || c.mods().iter().any(|m| m.to_lowercase().contains(&needle))
            })
            .map(|(i, _)| i)
            .collect();

        self.selected = self.selected.min(self.visible.len().saturating_sub(1));
    }

    pub fn next(&mut self) {
        if self.selected + 1 < self.visible.len() {
            self.selected += 1;
        }
    }

    pub fn previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn selected_conflict(&self) -> Option<&Conflict> {
        self.visible.get(self.selected).map(|&i| &self.conflicts[i])
    }

    /// Cycle Info -> Warning -> Critical -> Info.
    pub fn cycle_severity(&mut self) {
        self.min_severity = match self.min_severity {
            Severity::Info => Severity::Warning,
            Severity::Warning => Severity::Critical,
            Severity::Critical => Severity::Info,
        };
        self.refresh_visible();
    }

    #[cfg(test)]
    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
        self.refresh_visible();
    }

    fn handle_key(&mut self, code: KeyCode) {
        if self.editing_filter {
            match code {
                KeyCode::Enter | KeyCode::Esc => self.editing_filter = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.refresh_visible();
                }
                KeyCode::Char(c) => {
                    self.filter.push(c);
                    self.refresh_visible();
                }
                _ => {}
            }
            return;
        }

        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.next(),
            KeyCode::Up | KeyCode::Char('k') => self.previous(),
            KeyCode::Char('f') => self.cycle_severity(),
            KeyCode::Char('/') => self.editing_filter = true,
            KeyCode::Char('c') => {
                self.filter.clear();
                self.refresh_visible();
            }
            _ => {}
        }
    }
}

pub fn run(conflicts: Vec<Conflict>) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, App::new(conflicts));
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, mut app: App) -> Result<()> {
    while !app.quit {
        terminal.draw(|frame| draw(frame, &app))?;

        // Windows terminals emit Release and Repeat events too; acting on all
        // of them would move the selection several rows per keypress.
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                app.handle_key(key.code);
            }
        }
    }
    Ok(())
}

fn draw(frame: &mut Frame, app: &App) {
    let [main, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(main);

    let items: Vec<ListItem> = app
        .visible
        .iter()
        .map(|&i| {
            let c = &app.conflicts[i];
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<5}", c.severity().to_string()),
                    Style::default().fg(severity_color(c.severity())),
                ),
                Span::raw(c.title()),
            ]))
        })
        .collect();

    let title = format!(
        " Conflicts {}/{} ",
        app.visible.len(),
        app.conflicts.len()
    );
    let list = List::new(items)
        .block(Block::bordered().title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");

    let mut state = ListState::default();
    if !app.visible.is_empty() {
        state.select(Some(app.selected));
    }
    frame.render_stateful_widget(list, left, &mut state);

    let detail = app
        .selected_conflict()
        .map(Conflict::detail)
        .unwrap_or_else(|| "No conflict matches the current filter.".to_string());
    frame.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Detail ")),
        right,
    );

    let help = if app.editing_filter {
        format!(" filter: {}_  (Enter to apply)", app.filter)
    } else {
        format!(
            " ↑↓/jk move   / filter   c clear   f severity>={}   q quit{}",
            app.min_severity,
            if app.filter.is_empty() {
                String::new()
            } else {
                format!("   [filter: {}]", app.filter)
            }
        )
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(Color::DarkGray)),
        footer,
    );
}

fn severity_color(severity: Severity) -> Color {
    match severity {
        Severity::Critical => Color::Red,
        Severity::Warning => Color::Yellow,
        Severity::Info => Color::Blue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Dep, DepKind};

    fn overlap(path: &str, mods: &[&str]) -> Conflict {
        Conflict::FileOverlap {
            path: path.to_string(),
            mods: mods.iter().map(|m| m.to_string()).collect(),
        }
    }

    fn missing(mod_id: &str, dep: &str) -> Conflict {
        Conflict::MissingDep {
            mod_id: mod_id.to_string(),
            dep: Dep {
                name: dep.to_string(),
                req: None,
                kind: DepKind::Required,
            },
        }
    }

    fn sample() -> App {
        App::new(vec![
            missing("alpha", "base"),
            overlap("assets/stone.png", &["beta", "gamma"]),
            overlap("assets/wood.png", &["beta", "delta"]),
        ])
    }

    #[test]
    fn starts_on_the_first_conflict() {
        let app = sample();
        assert_eq!(app.selected, 0);
        assert_eq!(app.visible.len(), 3);
    }

    #[test]
    fn does_not_move_past_the_last_row() {
        let mut app = sample();
        for _ in 0..10 {
            app.next();
        }
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn does_not_move_above_the_first_row() {
        let mut app = sample();
        app.previous();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn an_empty_list_has_no_selected_conflict() {
        let mut app = App::new(Vec::new());
        app.next();
        assert!(app.selected_conflict().is_none());
    }

    #[test]
    fn filter_matches_mod_names_as_well_as_titles() {
        let mut app = sample();

        app.set_filter("gamma");

        assert_eq!(app.visible.len(), 1);
        assert!(matches!(
            app.selected_conflict(),
            Some(Conflict::FileOverlap { .. })
        ));
    }

    #[test]
    fn filter_is_case_insensitive() {
        let mut app = sample();
        app.set_filter("GAMMA");
        assert_eq!(app.visible.len(), 1);
    }

    #[test]
    fn selection_stays_in_range_when_the_filter_shrinks_the_list() {
        let mut app = sample();
        app.next();
        app.next();
        assert_eq!(app.selected, 2);

        app.set_filter("gamma");

        assert_eq!(app.selected, 0);
        assert!(app.selected_conflict().is_some());
    }

    #[test]
    fn severity_filter_hides_warnings() {
        let mut app = sample();

        app.cycle_severity(); // >= Warning: all three still visible
        assert_eq!(app.visible.len(), 3);

        app.cycle_severity(); // >= Critical: only the missing dependency
        assert_eq!(app.visible.len(), 1);

        app.cycle_severity(); // back to Info
        assert_eq!(app.visible.len(), 3);
    }

    #[test]
    fn typing_into_the_filter_narrows_the_list_live() {
        let mut app = sample();
        app.handle_key(KeyCode::Char('/'));
        for c in "gamma".chars() {
            app.handle_key(KeyCode::Char(c));
        }
        assert_eq!(app.visible.len(), 1);

        app.handle_key(KeyCode::Backspace);
        app.handle_key(KeyCode::Enter);

        assert_eq!(app.filter, "gamm");
        assert!(!app.editing_filter);
    }

    #[test]
    fn q_does_not_quit_while_typing_a_filter() {
        let mut app = sample();
        app.handle_key(KeyCode::Char('/'));
        app.handle_key(KeyCode::Char('q'));

        assert!(!app.quit);
        assert_eq!(app.filter, "q");
    }

    #[test]
    fn q_quits_in_normal_mode() {
        let mut app = sample();
        app.handle_key(KeyCode::Char('q'));
        assert!(app.quit);
    }
}
