use crate::config::Layout;
use crate::ui::table::Order;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Layout(Layout),
    Sort(Order),
    Watching(bool),
    Profile(String),
    Nothing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    pub chosen: bool,
    pub choice: Choice,
}

impl Entry {
    pub fn selectable(&self) -> bool {
        self.choice != Choice::Nothing
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub note: String,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Default)]
pub struct Panel {
    pub tab: usize,
    pub cursor: usize,
}

impl Panel {
    pub fn opened_at(sections: &[Section]) -> Self {
        let mut panel = Self::default();
        panel.settle(sections);
        panel
    }

    pub fn section<'a>(&self, sections: &'a [Section]) -> Option<&'a Section> {
        sections.get(self.tab)
    }

    pub fn chosen<'a>(&self, sections: &'a [Section]) -> Option<&'a Entry> {
        self.section(sections)?.entries.get(self.cursor)
    }

    pub fn switch_tab(&mut self, delta: isize, sections: &[Section]) {
        if sections.is_empty() {
            return;
        }
        let count = sections.len() as isize;
        self.tab = (self.tab as isize + delta).rem_euclid(count) as usize;
        self.cursor = 0;
        self.settle(sections);
    }

    pub fn step(&mut self, delta: isize, sections: &[Section]) {
        let Some(section) = self.section(sections) else {
            return;
        };
        let usable: Vec<usize> = section
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.selectable())
            .map(|(at, _)| at)
            .collect();
        if usable.is_empty() {
            let last = section.entries.len().saturating_sub(1) as isize;
            self.cursor = (self.cursor as isize + delta).clamp(0, last) as usize;
            return;
        }
        let here = usable.iter().position(|at| *at == self.cursor).unwrap_or(0) as isize;
        let moved = (here + delta).clamp(0, usable.len() as isize - 1) as usize;
        self.cursor = usable[moved];
    }

    fn settle(&mut self, sections: &[Section]) {
        let Some(section) = self.section(sections) else {
            return;
        };
        self.cursor = section
            .entries
            .iter()
            .position(|entry| entry.chosen && entry.selectable())
            .or_else(|| section.entries.iter().position(Entry::selectable))
            .unwrap_or(0);
    }
}

pub fn choosing(label: &str, chosen: bool, choice: Choice) -> Entry {
    Entry {
        label: label.to_string(),
        chosen,
        choice,
    }
}

pub fn telling(label: String) -> Entry {
    Entry {
        label,
        chosen: false,
        choice: Choice::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections() -> Vec<Section> {
        vec![
            Section {
                title: "layout".to_string(),
                note: String::new(),
                entries: vec![
                    choosing("table", false, Choice::Layout(Layout::Table)),
                    choosing("grouped", true, Choice::Layout(Layout::Grouped)),
                ],
            },
            Section {
                title: "keys".to_string(),
                note: String::new(),
                entries: vec![telling("j  move down".to_string())],
            },
        ]
    }

    #[test]
    fn opens_on_whatever_is_already_chosen() {
        let panel = Panel::opened_at(&sections());
        assert_eq!(panel.tab, 0);
        assert_eq!(panel.cursor, 1);
    }

    #[test]
    fn moving_stops_at_the_ends_rather_than_wrapping() {
        let sections = sections();
        let mut panel = Panel::opened_at(&sections);

        panel.step(-1, &sections);
        assert_eq!(panel.cursor, 0);
        panel.step(-1, &sections);
        assert_eq!(panel.cursor, 0);
        panel.step(5, &sections);
        assert_eq!(panel.cursor, 1);
    }

    #[test]
    fn tabs_wrap_around_because_there_are_few_of_them() {
        let sections = sections();
        let mut panel = Panel::opened_at(&sections);

        panel.switch_tab(1, &sections);
        assert_eq!(panel.tab, 1);
        panel.switch_tab(1, &sections);
        assert_eq!(panel.tab, 0);
        panel.switch_tab(-1, &sections);
        assert_eq!(panel.tab, 1);
    }

    #[test]
    fn a_section_that_only_tells_you_things_has_nothing_to_choose() {
        let sections = sections();
        let mut panel = Panel::opened_at(&sections);
        panel.switch_tab(1, &sections);

        assert_eq!(
            panel.chosen(&sections).map(|entry| entry.selectable()),
            Some(false)
        );
    }

    #[test]
    fn a_section_you_can_only_read_still_scrolls() {
        let mut sections = sections();
        sections[1].entries = (0..5).map(|at| telling(format!("line {at}"))).collect();

        let mut panel = Panel::opened_at(&sections);
        panel.switch_tab(1, &sections);
        assert_eq!(panel.cursor, 0);

        panel.step(3, &sections);
        assert_eq!(panel.cursor, 3);
        panel.step(99, &sections);
        assert_eq!(panel.cursor, 4);
    }

    #[test]
    fn switching_tab_lands_on_that_sections_own_choice() {
        let mut sections = sections();
        sections[1] = Section {
            title: "sort".to_string(),
            note: String::new(),
            entries: vec![
                choosing("queue", false, Choice::Sort(Order::Queue)),
                choosing("age", false, Choice::Sort(Order::Age)),
                choosing("status", true, Choice::Sort(Order::Status)),
            ],
        };
        let mut panel = Panel::opened_at(&sections);
        panel.switch_tab(1, &sections);
        assert_eq!(panel.cursor, 2);
    }
}
