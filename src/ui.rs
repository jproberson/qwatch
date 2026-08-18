pub mod render;
pub mod settings;
pub mod table;
pub mod theme;

use crate::action::{self, Change, Plan};
use crate::config::{Action, ActionKind, Layout, Profile, Scope, StatusColor};
use crate::keys::{Binding, Motion};
use crate::preview::{self, Line as PreviewLine};
use crate::scan::{self, Entry, Queue};
use crate::ui::settings::{Choice, Panel, Section, choosing, telling};
use crate::ui::table::{Order, Row};
use crate::ui::theme::Theme;
use crate::watch;
use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const POLL: Duration = Duration::from_millis(100);
const PAGE: isize = 10;

pub struct Prompt {
    pub question: String,
    pub detail: Vec<String>,
    changes: Vec<Change>,
}

pub struct App {
    pub profile: Profile,
    pub queues: Vec<Queue>,
    pub rows: Vec<Row>,
    pub cursor: usize,
    pub order: Order,
    pub layout: Layout,
    pub watching: bool,
    pub preview: Vec<PreviewLine>,
    pub preview_scroll: u16,
    pub prompt: Option<Prompt>,
    pub message: Option<String>,
    pub help: bool,
    pub panel: Option<Panel>,
    pub catalogue: BTreeMap<String, Profile>,
    pub help_scroll: u16,
    pub theme: Theme,
    pub now: SystemTime,
    pub list_state: ListState,
    pub list_area: Rect,
    pub preview_area: Rect,
    colors: BTreeMap<String, StatusColor>,
    bindings: Vec<(Binding, Action)>,
    canonical_root: PathBuf,
    running: bool,
    edit: Option<PathBuf>,
}

impl App {
    pub fn new(profile: Profile) -> Result<Self> {
        let layout = profile.layout;
        let watching = profile.watch.enabled;
        let colors = status_colors(&profile);
        let bindings: Vec<(Binding, Action)> = profile
            .action
            .iter()
            .filter_map(|action| Some((Binding::parse(&action.key).ok()?, action.clone())))
            .collect();

        let canonical_root = profile
            .root
            .canonicalize()
            .unwrap_or_else(|_| profile.root.clone());
        let mut app = Self {
            profile,
            queues: Vec::new(),
            rows: Vec::new(),
            cursor: 0,
            order: Order::default(),
            layout,
            watching,
            preview: Vec::new(),
            preview_scroll: 0,
            prompt: None,
            message: None,
            help: false,
            panel: None,
            catalogue: BTreeMap::new(),
            help_scroll: 0,
            theme: Theme::default(),
            now: SystemTime::now(),
            list_state: ListState::default(),
            list_area: Rect::ZERO,
            preview_area: Rect::ZERO,
            colors,
            bindings,
            canonical_root,
            running: true,
            edit: None,
        };
        app.rebuild(scan::scan(&app.profile)?);
        Ok(app)
    }

    pub fn root_name(&self) -> String {
        self.profile
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.profile.root.display().to_string())
    }

    pub fn file_count(&self) -> usize {
        table::file_positions(&self.rows).len()
    }

    pub fn color_of(&self, status: &str) -> StatusColor {
        self.colors.get(status).copied().unwrap_or(MUTED)
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.rows.get(self.cursor).and_then(Row::entry)
    }

    pub fn preview_title(&self) -> String {
        match self.selected() {
            None => "nothing selected".to_string(),
            Some(entry) => match &entry.detail {
                Some(detail) => format!("{} {}", entry.label, detail),
                None => entry.label.clone(),
            },
        }
    }

    fn rebuild(&mut self, queues: Vec<Queue>) {
        let held = self.selected().map(|entry| entry.path.clone());
        self.rows = table::build(&self.profile, &queues, self.order, self.layout);
        self.queues = queues;
        self.cursor = held
            .and_then(|path| table::relocate(&self.rows, &path))
            .or_else(|| table::settle(&self.rows, self.cursor, true))
            .unwrap_or(0);
        self.refresh_preview();
    }

    fn refresh_preview(&mut self) {
        self.preview_scroll = 0;
        self.preview = match self.selected() {
            None => Vec::new(),
            Some(entry) => preview::of_file(&self.profile.preview, &entry.path.clone()),
        };
    }

    pub fn rescan(&mut self) -> Result<bool> {
        self.now = SystemTime::now();
        let queues = scan::scan(&self.profile)?;
        if table::build(&self.profile, &queues, self.order, self.layout) == self.rows {
            return Ok(false);
        }
        self.rebuild(queues);
        Ok(true)
    }

    fn move_cursor(&mut self, delta: isize) {
        if let Some(moved) = table::step(&self.rows, self.cursor, delta)
            && moved != self.cursor
        {
            self.cursor = moved;
            self.refresh_preview();
        }
    }

    fn jump(&mut self, to_end: bool) {
        let target = if to_end {
            table::last_file(&self.rows)
        } else {
            table::first_file(&self.rows)
        };
        if let Some(target) = target
            && target != self.cursor
        {
            self.cursor = target;
            self.refresh_preview();
        }
    }

    fn action_for(&self, event: KeyEvent) -> Option<Action> {
        self.bindings
            .iter()
            .find(|(binding, _)| binding.matches(event))
            .map(|(_, action)| action.clone())
    }

    fn reached_by(&self, scope: Scope) -> impl Iterator<Item = &Entry> {
        let selected = self.selected();
        self.rows
            .iter()
            .filter_map(Row::entry)
            .filter(move |entry| match (selected, scope) {
                (None, _) => false,
                (Some(_), Scope::All) => true,
                (Some(on), Scope::One) => entry.path == on.path,
                (Some(on), Scope::Status) => entry.status == on.status,
            })
    }

    fn in_scope(&self, scope: Scope) -> Vec<Entry> {
        self.reached_by(scope).cloned().collect()
    }

    pub fn labelled(&self, action: &Action) -> String {
        let reach = self.reached_by(action.scope).count();
        let Some(on) = self.selected() else {
            return action.name.clone();
        };
        match action.scope {
            Scope::One => action.name.clone(),
            Scope::All => format!("{} all {reach}", action.name),
            Scope::Status => format!("{} {reach} {}", action.name, on.status),
        }
    }

    fn begin(&mut self, action: &Action) {
        let entries = self.in_scope(action.scope);
        let Some(first) = entries.first() else {
            self.message = Some("nothing selected".to_string());
            return;
        };

        if action.kind == ActionKind::Edit {
            match action::plan(&self.profile, action, first) {
                Ok(Plan::Open(path)) => {
                    self.edit = Some(path);
                    self.running = false;
                }
                Ok(Plan::Change(_)) => {}
                Err(refusal) => self.message = Some(refusal.to_string()),
            }
            return;
        }

        let batch = action::plan_many(&self.profile, action, &entries);
        if batch.changes.is_empty() {
            self.message = Some(self.why_nothing(&batch));
            return;
        }
        if !action.confirm {
            self.carry_out(batch.changes);
            return;
        }
        self.prompt = Some(Prompt {
            question: self.question_for(action, &batch),
            detail: refusal_lines(&batch),
            changes: batch.changes,
        });
    }

    fn why_nothing(&self, batch: &action::Batch) -> String {
        match batch.refusals.as_slice() {
            [] => "nothing to do".to_string(),
            [(_, only)] => only.clone(),
            many => format!("all {} refused: {}", many.len(), many[0].1),
        }
    }

    fn question_for(&self, action: &Action, batch: &action::Batch) -> String {
        if action.scope == Scope::One && batch.changes.len() == 1 {
            return format!(
                "{}?",
                Plan::Change(batch.changes[0].clone()).describe(&self.canonical_root)
            );
        }
        let total = batch.changes.len() + batch.refusals.len();
        let counted = match batch.refusals.is_empty() {
            true => format!("{} files", batch.changes.len()),
            false => format!("{} of {total} files", batch.changes.len()),
        };
        format!(
            "{} {counted}{}?",
            action.name,
            self.scope_shown(action.scope)
        )
    }

    fn scope_shown(&self, scope: Scope) -> String {
        let Some(selected) = self.selected() else {
            return String::new();
        };
        match scope {
            Scope::Status => format!(" that are {}", selected.status),
            _ => String::new(),
        }
    }

    fn carry_out(&mut self, changes: Vec<Change>) {
        let mut failed = Vec::new();
        for change in &changes {
            if let Err(failure) = action::apply(change) {
                failed.push(failure.to_string());
            }
        }
        self.message = match failed.as_slice() {
            [] => None,
            [only] => Some(only.clone()),
            many => Some(format!(
                "{} of {} failed: {}",
                many.len(),
                changes.len(),
                many[0]
            )),
        };
        let _ = self.rescan();
    }

    fn answer(&mut self, key: KeyCode) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        if matches!(key, KeyCode::Char('y') | KeyCode::Char('Y')) {
            self.carry_out(prompt.changes);
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        self.message = None;

        if self.prompt.is_some() {
            self.answer(key.code);
            return;
        }
        if self.panel.is_some() {
            self.steer_panel(key);
            return;
        }
        if self.help {
            match self.profile.keys.motion_for(key) {
                Some(Motion::Down) => self.help_scroll = self.help_scroll.saturating_add(1),
                Some(Motion::Up) => self.help_scroll = self.help_scroll.saturating_sub(1),
                _ => {
                    self.help = false;
                    self.help_scroll = 0;
                }
            }
            return;
        }

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.running = false;
            return;
        }
        if let Some(motion) = self.profile.keys.motion_for(key) {
            self.travel(motion);
            return;
        }
        if let Some(action) = self.action_for(key) {
            self.begin(&action);
        }
    }

    pub fn sections(&self) -> Vec<Section> {
        let mut sections = vec![
            Section {
                title: "layout".to_string(),
                note: "how the list is shaped".to_string(),
                entries: [Layout::Table, Layout::Grouped]
                    .into_iter()
                    .map(|layout| {
                        choosing(
                            layout.named(),
                            self.layout == layout,
                            Choice::Layout(layout),
                        )
                    })
                    .collect(),
            },
            Section {
                title: "sort".to_string(),
                note: "what decides the order".to_string(),
                entries: [Order::Queue, Order::Age, Order::Status]
                    .into_iter()
                    .map(|order| choosing(order.label(), self.order == order, Choice::Sort(order)))
                    .collect(),
            },
        ];

        if self.catalogue.len() > 1 {
            sections.push(Section {
                title: "queues".to_string(),
                note: "which root to browse".to_string(),
                entries: self
                    .catalogue
                    .iter()
                    .map(|(name, profile)| {
                        choosing(
                            &format!("{name}  {}", profile.root.display()),
                            profile.root == self.profile.root,
                            Choice::Profile(name.clone()),
                        )
                    })
                    .collect(),
            });
        }

        sections.push(Section {
            title: "watching".to_string(),
            note: "redraw when the directories change".to_string(),
            entries: vec![
                choosing("on", self.watching, Choice::Watching(true)),
                choosing("off", !self.watching, Choice::Watching(false)),
            ],
        });
        sections.push(Section {
            title: "keys".to_string(),
            note: "set these in the config file".to_string(),
            entries: self
                .profile
                .action
                .iter()
                .map(|action| telling(format!("{:<12} {}", action.key, self.labelled(action))))
                .chain(
                    self.profile
                        .keys
                        .described()
                        .into_iter()
                        .map(|(keys, meaning)| telling(format!("{keys:<12} {meaning}"))),
                )
                .collect(),
        });
        sections
    }

    fn steer_panel(&mut self, key: KeyEvent) {
        let sections = self.sections();
        let Some(panel) = self.panel.as_mut() else {
            return;
        };

        match key.code {
            KeyCode::Esc => self.panel = None,
            KeyCode::Tab | KeyCode::Right => panel.switch_tab(1, &sections),
            KeyCode::BackTab | KeyCode::Left => panel.switch_tab(-1, &sections),
            KeyCode::Down | KeyCode::Char('j') => panel.step(1, &sections),
            KeyCode::Up | KeyCode::Char('k') => panel.step(-1, &sections),
            KeyCode::Enter => {
                let picked = panel.chosen(&sections).map(|entry| entry.choice.clone());
                if let Some(choice) = picked {
                    self.take_up(choice);
                }
            }
            _ => {}
        }
    }

    fn take_up(&mut self, choice: Choice) {
        match choice {
            Choice::Nothing => {}
            Choice::Layout(layout) => {
                self.layout = layout;
                self.relist();
            }
            Choice::Sort(order) => {
                self.order = order;
                self.relist();
            }
            Choice::Watching(on) => self.watching = on,
            Choice::Profile(name) => {
                if let Some(profile) = self.catalogue.get(&name).cloned() {
                    self.adopt(profile);
                }
            }
        }
    }

    fn adopt(&mut self, profile: Profile) {
        self.canonical_root = profile
            .root
            .canonicalize()
            .unwrap_or_else(|_| profile.root.clone());
        self.colors = status_colors(&profile);
        self.bindings = profile
            .action
            .iter()
            .filter_map(|action| Some((Binding::parse(&action.key).ok()?, action.clone())))
            .collect();
        self.layout = profile.layout;
        self.watching = profile.watch.enabled;
        self.profile = profile;
        self.cursor = 0;

        match scan::scan(&self.profile) {
            Ok(queues) => {
                self.rebuild(queues);
                self.message = None;
            }
            Err(failure) => self.message = Some(failure.to_string()),
        }
    }

    fn relist(&mut self) {
        let held = self.selected().map(|entry| entry.path.clone());
        self.rows = table::build(&self.profile, &self.queues, self.order, self.layout);
        self.cursor = held
            .and_then(|path| table::relocate(&self.rows, &path))
            .or_else(|| table::first_file(&self.rows))
            .unwrap_or(0);
    }

    fn travel(&mut self, motion: Motion) {
        match motion {
            Motion::Down => self.move_cursor(1),
            Motion::Up => self.move_cursor(-1),
            Motion::PageDown => self.move_cursor(PAGE),
            Motion::PageUp => self.move_cursor(-PAGE),
            Motion::First => self.jump(false),
            Motion::Last => self.jump(true),
            Motion::PreviewDown => {
                self.preview_scroll = self.preview_scroll.saturating_add(1);
            }
            Motion::PreviewUp => {
                self.preview_scroll = self.preview_scroll.saturating_sub(1);
            }
            Motion::Edit => self.open_selected(),
            Motion::Help => self.help = true,
            Motion::Quit => self.running = false,
            Motion::Rescan => {
                if let Err(failure) = self.rescan() {
                    self.message = Some(failure.to_string());
                }
            }
            Motion::Sort => {
                self.order = self.order.next();
                self.relist();
            }
            Motion::Layout => {
                self.layout = self.layout.other();
                self.relist();
            }
            Motion::Settings => {
                self.panel = Some(Panel::opened_at(&self.sections()));
            }
        }
    }

    fn on_mouse(&mut self, event: MouseEvent) {
        let at = Position::new(event.column, event.row);

        if self.preview_area.contains(at) {
            return match event.kind {
                MouseEventKind::ScrollDown => self.travel(Motion::PreviewDown),
                MouseEventKind::ScrollUp => self.travel(Motion::PreviewUp),
                _ => {}
            };
        }
        if !self.list_area.contains(at) {
            return;
        }
        match event.kind {
            MouseEventKind::ScrollDown => self.travel(Motion::Down),
            MouseEventKind::ScrollUp => self.travel(Motion::Up),
            MouseEventKind::Down(MouseButton::Left) => self.click(event.row),
            _ => {}
        }
    }

    fn click(&mut self, row: u16) {
        let clicked = self.list_state.offset() + (row - self.list_area.y) as usize;
        if let Some(landed) = table::settle(&self.rows, clicked, true)
            && landed != self.cursor
        {
            self.cursor = landed;
            self.refresh_preview();
        }
    }

    fn open_selected(&mut self) {
        if let Some(entry) = self.selected() {
            self.edit = Some(entry.path.clone());
            self.running = false;
        }
    }
}

const ATTENTION: StatusColor = StatusColor::Indexed(1);
const ACTIVE: StatusColor = StatusColor::Indexed(6);
const UNCLAIMED: StatusColor = StatusColor::Indexed(215);
const FINISHED: StatusColor = StatusColor::Indexed(2);
const MUTED: StatusColor = StatusColor::Indexed(8);

const FINISHED_WORDS: [&str; 8] = [
    "done",
    "complete",
    "finish",
    "processed",
    "archive",
    "success",
    "sent",
    "delivered",
];

fn status_colors(profile: &Profile) -> BTreeMap<String, StatusColor> {
    let mut colors: BTreeMap<String, StatusColor> = profile
        .state
        .iter()
        .map(|state| (state.name.clone(), resting_color(state)))
        .collect();

    for status in &profile.status {
        let inherited = status
            .state
            .as_ref()
            .and_then(|state| colors.get(state).copied())
            .unwrap_or(StatusColor::Plain);
        let chosen = status.color.unwrap_or(match status.when.is_empty() {
            true => inherited,
            false => ACTIVE,
        });
        colors.insert(status.name.clone(), chosen);
    }
    colors
}

fn resting_color(state: &crate::config::State) -> StatusColor {
    if state.priority > 0 {
        return ATTENTION;
    }
    let name = state.name.to_lowercase();
    match FINISHED_WORDS.iter().any(|word| name.contains(word)) {
        true => FINISHED,
        false => UNCLAIMED,
    }
}

fn refusal_lines(batch: &action::Batch) -> Vec<String> {
    if batch.refusals.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![format!("{} refused:", batch.refusals.len())];
    lines.extend(
        batch
            .refusals
            .iter()
            .take(3)
            .map(|(name, reason)| format!("  {name}: {reason}")),
    );
    if batch.refusals.len() > 3 {
        lines.push(format!("  and {} more", batch.refusals.len() - 3));
    }
    lines
}

pub fn run(profile: Profile, catalogue: BTreeMap<String, Profile>) -> Result<()> {
    let mut app = App::new(profile)?;
    app.catalogue = catalogue;

    loop {
        let mut terminal = ratatui::init();
        let mouse =
            app.profile.mouse && crossterm::execute!(std::io::stdout(), EnableMouseCapture).is_ok();
        let outcome = pump(&mut terminal, &mut app);
        if mouse {
            let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        }
        ratatui::restore();
        outcome?;

        match app.edit.take() {
            None => return Ok(()),
            Some(path) => {
                edit(&path)?;
                app.running = true;
                app.rescan()?;
            }
        }
    }
}

fn pump(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    let settings = watch::Settings {
        debounce: Duration::from_millis(app.profile.watch.debounce_ms),
        backstop: Duration::from_millis(app.profile.watch.backstop_ms),
    };
    let mut watched: Vec<PathBuf> = Vec::new();
    let mut watch: Option<watch::Watch> = None;
    let mut rearm = app.profile.watch.enabled;

    while app.running {
        if rearm && app.watching {
            let wanted = watch::targets(&app.profile, &app.queues);
            if wanted != watched {
                watch = watch::start(&wanted, settings).ok();
                watched = wanted;
            }
            rearm = false;
        }

        terminal.draw(|frame| render::draw(frame, app))?;

        if event::poll(POLL)? {
            match event::read()? {
                Event::Key(key) => app.on_key(key),
                Event::Mouse(mouse) => app.on_mouse(mouse),
                _ => {}
            }
        }
        if watch
            .as_ref()
            .is_some_and(|watch| watch.ticks().try_recv().is_ok())
            && app.rescan()?
        {
            rearm = app.profile.watch.enabled;
        }
    }
    Ok(())
}

fn edit(path: &std::path::Path) -> Result<()> {
    let fallback = match cfg!(windows) {
        true => "notepad",
        false => "vi",
    };
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| fallback.to_string());
    std::process::Command::new(editor).arg(path).status()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use tempfile::TempDir;

    const PROFILE: &str = r##"
root = "/replaced/by/the/test"

[[state]]
name = "queued"
dir  = "{queue}"

[[state]]
name     = "failed"
dir      = "{queue}-failed"
priority = 10

[filename]
pattern  = '^(?<claim>[\dx])_(?<job>[A-Za-z]\w*)-(?<index>\d+)\.txt$'
template = "{claim}_{job}-{index}.txt"
label    = "{job}"
detail   = "#{index}"

[[status]]
name  = "failed"
state = "failed"

[[status]]
name  = "running"
state = "queued"
when  = { claim = '^\d+$' }
badge = "worker {claim}"

[[status]]
name  = "waiting"
state = "queued"

[[action]]
key      = "r"
name     = "restart"
type     = "move"
to_state = "queued"
set      = { claim = "x" }

[[action]]
key  = "d"
name = "delete"
type = "delete"

[[action]]
key   = "D"
name  = "delete"
type  = "delete"
scope = "all"

[[action]]
key   = "X"
name  = "delete"
type  = "delete"
scope = "status"

[[action]]
key      = "A"
name     = "restart"
type     = "move"
to_state = "queued"
set      = { claim = "x" }
scope    = "status"

[preview]
format = "delimited-json"
labels = ["job"]

[[preview.detect]]
pattern = '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
label   = "id"
"##;

    const PAYLOAD: &str = "ExtractTotals,0,b27e4ac4-1111-2222-3333-444455556666,False\t{\"AnalysisId\":4821,\"Retries\":2}";

    fn fixture() -> (TempDir, App) {
        let root = TempDir::new().unwrap();
        for directory in ["invoices", "invoices-failed", "receipts", "receipts-failed"] {
            std::fs::create_dir(root.path().join(directory)).unwrap();
        }
        std::fs::write(
            root.path().join("receipts/x_RenderReport-0.txt"),
            "RenderReport,0",
        )
        .unwrap();
        std::fs::write(
            root.path().join("receipts-failed/x_ParseInvoice-0.txt"),
            "ParseInvoice,0",
        )
        .unwrap();
        std::fs::write(
            root.path().join("receipts-failed/3_ExtractTotals-1.txt"),
            PAYLOAD,
        )
        .unwrap();

        let mut profile: Profile = toml::from_str(PROFILE).unwrap();
        profile.validate().unwrap();
        profile.root = root.path().to_path_buf();

        let mut app = App::new(profile).unwrap();
        app.theme = Theme::plain();
        (root, app)
    }

    fn as_text(buffer: &Buffer) -> String {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn screen(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render::draw(frame, app)).unwrap();
        as_text(terminal.backend().buffer())
    }

    #[test]
    fn renders_the_grouped_layout() {
        let (_root, mut app) = fixture();
        app.layout = crate::config::Layout::Grouped;
        app.relist();
        app.refresh_preview();
        println!("\n{}\n", screen(&mut app, 92, 20));
    }

    #[test]
    fn renders_the_settings_panel() {
        let (root, mut app) = fixture();
        let mut other = app.profile.clone();
        other.root = root.path().join("receipts");
        app.catalogue
            .insert("ingest".to_string(), app.profile.clone());
        app.catalogue.insert("receipts-only".to_string(), other);
        app.panel = Some(Panel::opened_at(&app.sections()));
        println!("\n{}\n", screen(&mut app, 92, 20));
    }

    #[test]
    fn renders_the_settings_keys_tab() {
        let (_root, mut app) = fixture();
        let sections = app.sections();
        let mut panel = Panel::opened_at(&sections);
        while sections[panel.tab].title != "keys" {
            panel.switch_tab(1, &sections);
        }
        app.panel = Some(panel);
        println!("\n{}\n", screen(&mut app, 92, 20));
    }

    #[test]
    fn renders_the_browser() {
        let (_root, mut app) = fixture();
        app.cursor = table::first_file(&app.rows).unwrap() + 1;
        app.refresh_preview();
        println!("\n{}\n", screen(&mut app, 92, 20));
    }

    #[test]
    fn renders_the_confirm_prompt() {
        let (_root, mut app) = fixture();
        app.cursor = table::first_file(&app.rows).unwrap() + 1;
        let restart = app
            .action_for(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
            .unwrap();
        app.begin(&restart);
        println!("\n{}\n", screen(&mut app, 92, 20));
    }

    #[test]
    fn renders_the_help_overlay() {
        let (_root, mut app) = fixture();
        app.help = true;
        println!("\n{}\n", screen(&mut app, 92, 20));
    }

    fn painted(app: &mut App) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(92, 20)).unwrap();
        terminal.draw(|frame| render::draw(frame, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn the_selected_row_keeps_its_column_colours() {
        let (_root, mut app) = fixture();
        app.theme = Theme::default();
        app.cursor = table::first_file(&app.rows).unwrap() + 1;

        let buffer = painted(&mut app);
        let row = 1 + app.cursor as u16;
        let styles: Vec<_> = (1..40).map(|x| buffer[(x, row)].style()).collect();
        assert!(
            !styles.windows(2).all(|pair| pair[0] == pair[1]),
            "the selected row was flattened into one style"
        );
    }

    #[test]
    fn only_the_selected_row_carries_the_marker() {
        let (_root, mut app) = fixture();
        app.cursor = table::first_file(&app.rows).unwrap() + 1;

        let buffer = painted(&mut app);
        let marked: Vec<u16> = (1..19)
            .filter(|y| buffer[(1u16, *y)].symbol() == "\u{2588}")
            .collect();
        assert_eq!(marked, [1 + app.cursor as u16]);
    }

    #[test]
    fn every_row_but_the_selected_one_is_dimmed() {
        use ratatui::style::Modifier;

        let (_root, mut app) = fixture();
        app.theme = Theme::default();
        app.cursor = table::first_file(&app.rows).unwrap() + 1;

        let buffer = painted(&mut app);
        let dimmed = |y: u16| {
            buffer[(4u16, y)]
                .style()
                .add_modifier
                .contains(Modifier::DIM)
        };

        assert!(!dimmed(1 + app.cursor as u16), "the selected row is dimmed");
        for other in table::file_positions(&app.rows) {
            if other != app.cursor {
                assert!(dimmed(1 + other as u16), "row {other} was not dimmed");
            }
        }
    }

    fn click_at(app: &mut App, row: u16) {
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: app.list_area.x + 2,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }

    #[test]
    fn a_click_selects_the_row_under_it() {
        let (_root, mut app) = fixture();
        painted(&mut app);
        let target = table::last_file(&app.rows).unwrap();

        let row = app.list_area.y + target as u16;
        click_at(&mut app, row);
        assert_eq!(app.cursor, target);
    }

    #[test]
    fn a_click_on_a_header_settles_onto_a_real_file() {
        let (_root, mut app) = fixture();
        painted(&mut app);
        app.cursor = table::last_file(&app.rows).unwrap();

        let row = app.list_area.y;
        click_at(&mut app, row);
        assert_eq!(Some(app.cursor), table::first_file(&app.rows));
    }

    #[test]
    fn a_click_past_the_last_file_lands_on_the_last_file() {
        let (_root, mut app) = fixture();
        painted(&mut app);

        let row = app.list_area.y + app.list_area.height - 1;
        click_at(&mut app, row);
        assert_eq!(Some(app.cursor), table::last_file(&app.rows));
    }

    #[test]
    fn a_click_outside_the_list_is_ignored() {
        let (_root, mut app) = fixture();
        painted(&mut app);
        let before = app.cursor;

        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: app.preview_area.x + 1,
            row: app.preview_area.y + 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.cursor, before);
    }

    #[test]
    fn the_wheel_moves_the_cursor_over_the_list_and_the_preview_over_the_preview() {
        let (_root, mut app) = fixture();
        painted(&mut app);
        let before = app.cursor;

        app.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: app.list_area.x + 2,
            row: app.list_area.y + 2,
            modifiers: KeyModifiers::NONE,
        });
        assert_ne!(app.cursor, before);

        let moved = app.cursor;
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: app.preview_area.x + 1,
            row: app.preview_area.y + 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.cursor, moved);
        assert_eq!(app.preview_scroll, 1);
    }

    #[test]
    fn a_rebound_key_moves_the_cursor_and_the_default_stops_working() {
        let (_root, mut app) = fixture();
        app.profile.keys = toml::from_str(r#"down = ["n"]"#).unwrap();
        let before = app.cursor;

        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.cursor, before);

        app.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_ne!(app.cursor, before);
    }

    #[test]
    fn survives_being_drawn_into_a_cramped_terminal() {
        let (_root, mut app) = fixture();
        for (width, height) in [(1, 1), (2, 2), (5, 3), (20, 4), (40, 6), (200, 60)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render::draw(frame, &mut app))
                .unwrap_or_else(|_| panic!("failed to draw at {width}x{height}"));
        }
    }

    #[test]
    fn survives_the_overlays_in_a_cramped_terminal() {
        let (_root, mut app) = fixture();
        app.help = true;
        for (width, height) in [(1, 1), (4, 2), (20, 5), (60, 8)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render::draw(frame, &mut app))
                .unwrap_or_else(|_| panic!("help failed to draw at {width}x{height}"));
        }
    }

    #[test]
    fn the_help_scrolls_when_it_cannot_all_fit() {
        let (_root, mut app) = fixture();
        app.help = true;

        let mut terminal = Terminal::new(TestBackend::new(70, 10)).unwrap();
        terminal
            .draw(|frame| render::draw(frame, &mut app))
            .unwrap();
        let text = as_text(terminal.backend().buffer());
        assert!(
            text.contains("to scroll"),
            "no hint that there is more:\n{text}"
        );

        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.help_scroll, 1);
        assert!(app.help, "scrolling should not close the help");

        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.help);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn a_long_confirmation_wraps_rather_than_being_cut_off() {
        let (_root, mut app) = fixture();
        app.cursor = table::first_file(&app.rows).unwrap() + 1;
        let restart = app
            .action_for(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
            .unwrap();
        app.begin(&restart);

        let mut terminal = Terminal::new(TestBackend::new(48, 14)).unwrap();
        terminal
            .draw(|frame| render::draw(frame, &mut app))
            .unwrap();
        let text = as_text(terminal.backend().buffer());

        assert!(
            text.contains("x_ExtractTotals-1.txt"),
            "tail of the prompt was lost:\n{text}"
        );
    }

    #[test]
    fn a_queue_that_appears_later_becomes_something_to_watch() {
        let (root, mut app) = fixture();
        let before = crate::watch::targets(&app.profile, &app.queues);

        std::fs::create_dir(root.path().join("payroll")).unwrap();
        std::fs::write(root.path().join("payroll/x_RunPayroll-0.txt"), "").unwrap();
        assert!(
            app.rescan().unwrap(),
            "the new queue should change the list"
        );

        let after = crate::watch::targets(&app.profile, &app.queues);
        assert!(!before.contains(&root.path().join("payroll")));
        assert!(
            after.contains(&root.path().join("payroll")),
            "a queue that appeared while running is never watched"
        );
    }

    #[test]
    fn a_rescan_reports_whether_anything_actually_changed() {
        let (root, mut app) = fixture();
        assert!(
            !app.rescan().unwrap(),
            "nothing changed but rescan said it did"
        );

        std::fs::write(root.path().join("receipts/x_NewJob-9.txt"), "").unwrap();
        assert!(app.rescan().unwrap(), "a new file should count as a change");
    }

    fn press(app: &mut App, key: char) {
        app.on_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
    }

    fn files_left(root: &TempDir) -> usize {
        ["invoices", "invoices-failed", "receipts", "receipts-failed"]
            .iter()
            .filter_map(|directory| std::fs::read_dir(root.path().join(directory)).ok())
            .map(|listing| listing.flatten().count())
            .sum()
    }

    #[test]
    fn deleting_everything_takes_the_whole_list_at_once() {
        let (root, mut app) = fixture();
        assert_eq!(files_left(&root), 3);

        press(&mut app, 'D');
        let prompt = app.prompt.as_ref().expect("a bulk delete must confirm");
        assert_eq!(prompt.question, "delete 3 files?");

        app.answer(KeyCode::Char('y'));
        assert_eq!(files_left(&root), 0);
        assert_eq!(app.file_count(), 0);
    }

    #[test]
    fn declining_a_bulk_delete_leaves_every_file_alone() {
        let (root, mut app) = fixture();
        press(&mut app, 'D');
        app.answer(KeyCode::Char('n'));
        assert_eq!(files_left(&root), 3);
    }

    #[test]
    fn deleting_by_status_takes_every_failure_and_leaves_the_rest() {
        let (root, mut app) = fixture();
        app.cursor = app
            .rows
            .iter()
            .position(|row| row.entry().is_some_and(|entry| entry.status == "failed"))
            .unwrap();

        press(&mut app, 'X');
        assert_eq!(
            app.prompt.as_ref().unwrap().question,
            "delete 2 files that are failed?"
        );

        app.answer(KeyCode::Char('y'));
        assert_eq!(files_left(&root), 1);
        assert!(
            app.rows
                .iter()
                .filter_map(Row::entry)
                .all(|entry| entry.status != "failed")
        );
    }

    #[test]
    fn a_bulk_restart_says_how_many_it_had_to_refuse() {
        let (_root, mut app) = fixture();
        app.cursor = app
            .rows
            .iter()
            .position(|row| row.entry().is_some_and(|entry| entry.status == "waiting"))
            .unwrap();

        press(&mut app, 'A');
        assert!(
            app.prompt.is_none(),
            "restarting waiting files should refuse outright"
        );
        assert!(app.message.as_deref().unwrap().contains("already"));
    }

    #[test]
    fn a_bulk_action_refuses_two_files_that_want_the_same_name() {
        let (root, mut app) = fixture();
        std::fs::write(
            root.path().join("receipts-failed/x_ExtractTotals-1.txt"),
            "",
        )
        .unwrap();
        app.rescan().unwrap();

        app.cursor = app
            .rows
            .iter()
            .position(|row| row.entry().is_some_and(|entry| entry.status == "failed"))
            .unwrap();

        press(&mut app, 'A');
        let prompt = app
            .prompt
            .as_ref()
            .expect("some should still be restartable");
        assert!(
            prompt
                .detail
                .iter()
                .any(|line| line.contains("already claims that name")),
            "no warning about the collision: {:?}",
            prompt.detail
        );
    }

    fn footer_of(app: &mut App, width: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
        terminal.draw(|frame| render::draw(frame, app)).unwrap();
        as_text(terminal.backend().buffer())
            .lines()
            .last()
            .unwrap()
            .to_string()
    }

    #[test]
    fn the_footer_keeps_help_and_quit_however_narrow_it_gets() {
        let (_root, mut app) = fixture();
        for width in [40, 60, 80, 120, 200] {
            let footer = footer_of(&mut app, width);
            assert!(footer.contains("help"), "no help at {width}: {footer:?}");
            assert!(footer.contains("quit"), "no quit at {width}: {footer:?}");
            assert!(
                footer.chars().count() <= width as usize,
                "footer overflows at {width}: {footer:?}"
            );
        }
    }

    #[test]
    fn the_footer_says_when_it_has_hidden_something() {
        let (_root, mut app) = fixture();
        assert!(footer_of(&mut app, 50).contains('\u{2026}'));
        assert!(!footer_of(&mut app, 200).contains('\u{2026}'));
    }

    #[test]
    fn an_action_label_says_what_it_will_reach() {
        let (root, mut app) = fixture();
        std::fs::write(root.path().join("receipts/x_ParseInvoice-7.txt"), "").unwrap();
        app.rescan().unwrap();
        app.cursor = app
            .rows
            .iter()
            .position(|row| {
                row.entry()
                    .is_some_and(|entry| entry.label == "ParseInvoice")
            })
            .unwrap();

        let labelled = |key: char| {
            let action = app
                .action_for(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE))
                .unwrap();
            app.labelled(&action)
        };

        assert_eq!(labelled('d'), "delete");
        assert_eq!(labelled('D'), "delete all 4");
        assert_eq!(labelled('X'), "delete 2 failed");
        assert_eq!(labelled('A'), "restart 2 failed");
    }

    #[test]
    fn a_label_counts_only_what_the_scope_reaches() {
        let (_root, mut app) = fixture();
        app.cursor = app
            .rows
            .iter()
            .position(|row| row.entry().is_some_and(|entry| entry.status == "waiting"))
            .unwrap();

        let restart_status = app
            .action_for(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.labelled(&restart_status), "restart 1 waiting");
    }

    #[test]
    fn switching_layout_keeps_the_file_you_were_on() {
        let (_root, mut app) = fixture();
        app.cursor = table::last_file(&app.rows).unwrap();
        let held = app.selected().unwrap().path.clone();

        app.on_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        assert_eq!(app.layout, crate::config::Layout::Grouped);
        assert_eq!(app.selected().unwrap().path, held);

        app.on_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        assert_eq!(app.layout, crate::config::Layout::Table);
        assert_eq!(app.selected().unwrap().path, held);
    }

    #[test]
    fn a_profile_can_open_straight_into_the_grouped_layout() {
        let (root, _) = fixture();
        let mut profile: Profile =
            toml::from_str(&format!("layout = \"grouped\"{PROFILE}")).unwrap();
        profile.root = root.path().to_path_buf();
        let app = App::new(profile).unwrap();

        assert_eq!(app.layout, crate::config::Layout::Grouped);
        assert!(app.rows.iter().any(|row| matches!(row, Row::Group { .. })));
    }

    fn tap(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn open_settings(app: &mut App) {
        app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    }

    #[test]
    fn control_s_opens_the_panel_and_escape_closes_it() {
        let (_root, mut app) = fixture();
        open_settings(&mut app);
        assert!(app.panel.is_some());

        tap(&mut app, KeyCode::Esc);
        assert!(app.panel.is_none());
        assert!(
            app.running,
            "escape should close the panel, not the browser"
        );
    }

    #[test]
    fn the_panel_swallows_keys_that_would_otherwise_act() {
        let (root, mut app) = fixture();
        open_settings(&mut app);
        tap(&mut app, KeyCode::Char('d'));

        assert!(
            app.prompt.is_none(),
            "d reached the delete action through the panel"
        );
        assert_eq!(files_left(&root), 3);
    }

    #[test]
    fn choosing_a_layout_applies_it() {
        let (_root, mut app) = fixture();
        assert_eq!(app.layout, Layout::Table);

        open_settings(&mut app);
        tap(&mut app, KeyCode::Down);
        tap(&mut app, KeyCode::Enter);

        assert_eq!(app.layout, Layout::Grouped);
        assert!(app.rows.iter().any(|row| matches!(row, Row::Group { .. })));
    }

    #[test]
    fn tab_moves_between_sections_and_lands_on_what_is_already_set() {
        let (_root, mut app) = fixture();
        app.order = Order::Status;
        open_settings(&mut app);

        tap(&mut app, KeyCode::Tab);
        let sections = app.sections();
        let panel = app.panel.as_ref().unwrap();
        assert_eq!(sections[panel.tab].title, "sort");
        assert_eq!(
            panel.chosen(&sections).map(|entry| entry.label.clone()),
            Some("status".to_string())
        );
    }

    #[test]
    fn turning_watching_off_stops_it_being_rearmed() {
        let (_root, mut app) = fixture();
        assert!(app.watching);

        open_settings(&mut app);
        let sections = app.sections();
        let watching = sections.iter().position(|s| s.title == "watching").unwrap();
        app.panel.as_mut().unwrap().tab = watching;
        tap(&mut app, KeyCode::Down);
        tap(&mut app, KeyCode::Enter);

        assert!(!app.watching);
    }

    #[test]
    fn choosing_another_queue_root_moves_the_whole_browser_to_it() {
        let (root, mut app) = fixture();
        let elsewhere = TempDir::new().unwrap();
        std::fs::create_dir(elsewhere.path().join("payroll")).unwrap();
        std::fs::write(elsewhere.path().join("payroll/x_RunPayroll-3.txt"), "").unwrap();

        let mut other = app.profile.clone();
        other.root = elsewhere.path().to_path_buf();
        app.catalogue
            .insert("here".to_string(), app.profile.clone());
        app.catalogue.insert("there".to_string(), other);

        open_settings(&mut app);
        let sections = app.sections();
        let queues = sections.iter().position(|s| s.title == "queues").unwrap();
        let there = sections[queues]
            .entries
            .iter()
            .position(|entry| entry.label.starts_with("there"))
            .unwrap();
        app.panel.as_mut().unwrap().tab = queues;
        app.panel.as_mut().unwrap().cursor = there;
        tap(&mut app, KeyCode::Enter);

        assert_ne!(app.profile.root, root.path());
        assert!(
            app.rows
                .iter()
                .filter_map(Row::entry)
                .any(|e| e.label == "RunPayroll")
        );
    }

    #[test]
    fn the_queues_section_stays_hidden_when_there_is_only_one() {
        let (_root, app) = fixture();
        assert!(app.sections().iter().all(|s| s.title != "queues"));
    }

    #[test]
    fn the_cursor_opens_on_the_first_file() {
        let (_root, app) = fixture();
        assert!(app.selected().is_some());
        assert_eq!(Some(app.cursor), table::first_file(&app.rows));
    }

    #[test]
    fn a_refused_action_reports_instead_of_acting() {
        let (_root, mut app) = fixture();
        app.cursor = table::last_file(&app.rows).unwrap();
        let restart = app
            .action_for(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
            .unwrap();
        app.begin(&restart);

        assert!(app.prompt.is_none());
        assert!(app.message.as_deref().unwrap().contains("already"));
    }

    #[test]
    fn confirming_a_restart_moves_the_file_and_keeps_it_selected() {
        let (root, mut app) = fixture();
        app.cursor = table::first_file(&app.rows).unwrap() + 1;
        let restart = app
            .action_for(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
            .unwrap();
        app.begin(&restart);

        assert!(app.prompt.is_some());
        app.answer(KeyCode::Char('y'));

        assert!(root.path().join("receipts/x_ExtractTotals-1.txt").exists());
        assert!(
            !root
                .path()
                .join("receipts-failed/3_ExtractTotals-1.txt")
                .exists()
        );
    }

    #[test]
    fn declining_a_prompt_changes_nothing() {
        let (root, mut app) = fixture();
        app.cursor = table::first_file(&app.rows).unwrap() + 1;
        let restart = app
            .action_for(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
            .unwrap();
        app.begin(&restart);
        app.answer(KeyCode::Char('n'));

        assert!(app.prompt.is_none());
        assert!(
            root.path()
                .join("receipts-failed/3_ExtractTotals-1.txt")
                .exists()
        );
    }

    #[test]
    fn a_rescan_that_changes_nothing_leaves_the_cursor_alone() {
        let (_root, mut app) = fixture();
        app.cursor = table::last_file(&app.rows).unwrap();
        app.preview_scroll = 4;
        app.rescan().unwrap();

        assert_eq!(app.cursor, table::last_file(&app.rows).unwrap());
        assert_eq!(app.preview_scroll, 4);
    }
}

#[cfg(test)]
mod color_tests {
    use super::*;

    fn colors(extra: &str) -> BTreeMap<String, StatusColor> {
        let profile: Profile = toml::from_str(&format!(
            r##"
root = "/tmp"

[[state]]
name = "queued"
dir  = "{{queue}}"

[[state]]
name     = "failed"
dir      = "{{queue}}-failed"
priority = 10

[filename]
pattern  = '^(?<claim>[\dx])_(?<job>\w+)$'
template = "{{claim}}_{{job}}"
label    = "{{job}}"

[[status]]
name  = "failed"
state = "failed"

[[status]]
name  = "running"
state = "queued"
when  = {{ claim = '^\d+$' }}

[[status]]
name  = "waiting"
state = "queued"
{extra}
"##
        ))
        .unwrap();
        status_colors(&profile)
    }

    #[test]
    fn a_state_that_needs_attention_is_marked_out_from_a_resting_one() {
        let colors = colors("");
        assert_eq!(colors["failed"], ATTENTION);
        assert_eq!(colors["waiting"], UNCLAIMED);
    }

    #[test]
    fn two_statuses_of_one_state_do_not_share_a_color() {
        let colors = colors("");
        assert_ne!(colors["waiting"], colors["running"]);
        assert_eq!(colors["running"], ACTIVE);
    }

    #[test]
    fn a_profile_can_name_the_color_itself() {
        let colors = colors("color = \"magenta\"");
        assert_eq!(colors["waiting"], StatusColor::parse("magenta").unwrap());
    }

    #[test]
    fn a_finished_looking_state_goes_green_while_a_waiting_one_does_not() {
        let profile: Profile = toml::from_str(
            r#"
root = "/tmp"
[[state]]
name = "inbox"
dir  = "inbox"
[[state]]
name = "done"
dir  = "done"
"#,
        )
        .unwrap();
        let colors = status_colors(&profile);
        assert_eq!(colors["done"], FINISHED);
        assert_eq!(colors["inbox"], UNCLAIMED);
        assert_ne!(colors["done"], colors["inbox"]);
    }

    #[test]
    fn a_colour_can_be_named_numbered_or_written_in_hex() {
        assert_eq!(StatusColor::parse("red").unwrap(), StatusColor::Indexed(1));
        assert_eq!(
            StatusColor::parse("Orange").unwrap(),
            StatusColor::Indexed(208)
        );
        assert_eq!(
            StatusColor::parse("215").unwrap(),
            StatusColor::Indexed(215)
        );
        assert_eq!(
            StatusColor::parse("#ffa07a").unwrap(),
            StatusColor::Rgb(255, 160, 122)
        );
        assert_eq!(StatusColor::parse("none").unwrap(), StatusColor::Plain);
    }

    #[test]
    fn a_colour_nobody_could_mean_is_refused() {
        let message = StatusColor::parse("puce").unwrap_err().to_string();
        assert!(message.contains("puce"), "{message}");
        assert!(StatusColor::parse("#ff").is_err());
        assert!(StatusColor::parse("999").is_err());
    }

    #[test]
    fn a_status_nobody_declared_is_muted() {
        let profile: Profile = toml::from_str("root = \"/tmp\"").unwrap();
        let app_colors = status_colors(&profile);
        assert!(!app_colors.contains_key("unknown"));
    }
}
