pub mod render;
pub mod table;
pub mod theme;

use crate::action::{self, Change, Plan};
use crate::config::{Action, Profile, StatusColor};
use crate::keys::{Binding, Motion};
use crate::preview::{self, Line as PreviewLine};
use crate::scan::{self, Entry, Queue};
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
    change: Change,
}

pub struct App {
    pub profile: Profile,
    pub queues: Vec<Queue>,
    pub rows: Vec<Row>,
    pub cursor: usize,
    pub order: Order,
    pub preview: Vec<PreviewLine>,
    pub preview_scroll: u16,
    pub prompt: Option<Prompt>,
    pub message: Option<String>,
    pub help: bool,
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
            preview: Vec::new(),
            preview_scroll: 0,
            prompt: None,
            message: None,
            help: false,
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
        self.rows = table::build(&self.profile, &queues, self.order);
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
        if table::build(&self.profile, &queues, self.order) == self.rows {
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

    fn begin(&mut self, action: &Action) {
        let Some(entry) = self.selected() else {
            self.message = Some("nothing selected".to_string());
            return;
        };
        match action::plan(&self.profile, action, entry) {
            Err(refusal) => self.message = Some(refusal.to_string()),
            Ok(Plan::Open(path)) => {
                self.edit = Some(path);
                self.running = false;
            }
            Ok(Plan::Change(change)) => {
                let plan = Plan::Change(change.clone());
                if action.confirm {
                    self.prompt = Some(Prompt {
                        question: format!("{}?", plan.describe(&self.canonical_root)),
                        change,
                    });
                } else {
                    self.carry_out(change);
                }
            }
        }
    }

    fn carry_out(&mut self, change: Change) {
        if let Err(failure) = action::apply(&change) {
            self.message = Some(failure.to_string());
        }
        let _ = self.rescan();
    }

    fn answer(&mut self, key: KeyCode) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        if matches!(key, KeyCode::Char('y') | KeyCode::Char('Y')) {
            self.carry_out(prompt.change);
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
                let _ = self.rescan();
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

pub fn run(profile: Profile) -> Result<()> {
    let mut app = App::new(profile)?;

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
        if rearm {
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
