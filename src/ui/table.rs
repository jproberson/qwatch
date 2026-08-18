use crate::config::Profile;
use crate::scan::{Entry, Queue};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Order {
    #[default]
    Queue,
    Age,
    Status,
}

impl Order {
    pub fn next(self) -> Self {
        match self {
            Order::Queue => Order::Age,
            Order::Age => Order::Status,
            Order::Status => Order::Queue,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Order::Queue => "queue",
            Order::Age => "age",
            Order::Status => "status",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Columns,
    File(Box<Entry>),
    Empty(String),
}

impl Row {
    pub fn entry(&self) -> Option<&Entry> {
        match self {
            Row::File(entry) => Some(entry),
            _ => None,
        }
    }
}

pub fn build(profile: &Profile, queues: &[Queue], order: Order) -> Vec<Row> {
    let mut rows = vec![Row::Columns];

    if order == Order::Queue {
        for queue in queues {
            if queue.file_count() == 0 {
                rows.push(Row::Empty(queue.name.clone()));
                continue;
            }
            for state in &queue.states {
                rows.extend(state.entries.iter().cloned().map(into_row));
            }
        }
        return rows;
    }

    let mut entries: Vec<Entry> = queues.iter().flat_map(Queue::entries).cloned().collect();
    sorted(&mut entries, profile, order);
    rows.extend(entries.into_iter().map(into_row));
    rows.extend(
        queues
            .iter()
            .filter(|queue| queue.file_count() == 0)
            .map(|queue| Row::Empty(queue.name.clone())),
    );
    rows
}

fn into_row(entry: Entry) -> Row {
    Row::File(Box::new(entry))
}

fn sorted(entries: &mut [Entry], profile: &Profile, order: Order) {
    let ranks = state_ranks(profile);
    match order {
        Order::Age => entries.sort_by(|left, right| {
            left.modified
                .cmp(&right.modified)
                .then_with(|| left.queue.cmp(&right.queue))
                .then_with(|| left.file_name.cmp(&right.file_name))
        }),
        Order::Status => entries.sort_by(|left, right| {
            rank_of(&ranks, &left.state)
                .cmp(&rank_of(&ranks, &right.state))
                .then_with(|| left.modified.cmp(&right.modified))
                .then_with(|| left.queue.cmp(&right.queue))
        }),
        Order::Queue => {}
    }
}

fn state_ranks(profile: &Profile) -> BTreeMap<String, usize> {
    profile
        .states_in_display_order()
        .into_iter()
        .enumerate()
        .map(|(rank, state)| (state.name.clone(), rank))
        .collect()
}

fn rank_of(ranks: &BTreeMap<String, usize>, state: &str) -> usize {
    ranks.get(state).copied().unwrap_or(usize::MAX)
}

pub fn file_positions(rows: &[Row]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| matches!(row, Row::File(_)))
        .map(|(position, _)| position)
        .collect()
}

pub fn first_file(rows: &[Row]) -> Option<usize> {
    file_positions(rows).first().copied()
}

pub fn last_file(rows: &[Row]) -> Option<usize> {
    file_positions(rows).last().copied()
}

pub fn step(rows: &[Row], cursor: usize, delta: isize) -> Option<usize> {
    let files = file_positions(rows);
    if files.is_empty() {
        return None;
    }
    let last = files.len() - 1;
    let current = files
        .iter()
        .position(|&position| position == cursor)
        .unwrap_or_else(|| files.partition_point(|&position| position < cursor).min(last));
    let moved = (current as isize + delta).clamp(0, last as isize) as usize;
    Some(files[moved])
}

pub fn settle(rows: &[Row], target: usize, downward: bool) -> Option<usize> {
    let files = file_positions(rows);
    if downward {
        files
            .iter()
            .find(|&&position| position >= target)
            .or_else(|| files.last())
            .copied()
    } else {
        files
            .iter()
            .rev()
            .find(|&&position| position <= target)
            .or_else(|| files.first())
            .copied()
    }
}

pub fn relocate(rows: &[Row], path: &Path) -> Option<usize> {
    rows.iter()
        .position(|row| row.entry().is_some_and(|entry| entry.path == path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan;
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
"##;

    struct Fixture {
        _root: TempDir,
        profile: Profile,
    }

    impl Fixture {
        fn new(spec: &[(&str, &[&str])]) -> Self {
            let root = TempDir::new().unwrap();
            for (directory, files) in spec {
                std::fs::create_dir_all(root.path().join(directory)).unwrap();
                for file in *files {
                    std::fs::write(root.path().join(directory).join(file), "").unwrap();
                }
            }
            let mut profile: Profile = toml::from_str(PROFILE).unwrap();
            profile.validate().unwrap();
            profile.root = root.path().to_path_buf();
            Self { _root: root, profile }
        }

        fn rows(&self, order: Order) -> Vec<Row> {
            build(&self.profile, &scan::scan(&self.profile).unwrap(), order)
        }
    }

    fn shape(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Columns => "columns".to_string(),
                Row::Empty(queue) => format!("empty {queue}"),
                Row::File(entry) => format!("{} {} {}", entry.queue, entry.status, entry.label),
            })
            .collect()
    }

    fn populated() -> Fixture {
        Fixture::new(&[
            ("invoices", &[]),
            ("invoices-failed", &[]),
            ("receipts", &["x_RenderReport-0.txt"]),
            ("receipts-failed", &["x_ParseInvoice-0.txt", "3_ExtractTotals-1.txt"]),
        ])
    }

    #[test]
    fn opens_with_a_column_header_then_one_row_per_file() {
        let rows = populated().rows(Order::Queue);
        assert_eq!(rows[0], Row::Columns);
        assert_eq!(file_positions(&rows).len(), 3);
    }

    #[test]
    fn keeps_a_queue_together_and_lifts_its_failures_first() {
        let rows = populated().rows(Order::Queue);
        assert_eq!(
            shape(&rows),
            [
                "columns",
                "empty invoices",
                "receipts failed ParseInvoice",
                "receipts failed ExtractTotals",
                "receipts queued RenderReport",
            ]
        );
    }

    #[test]
    fn shows_a_queue_with_no_files_as_an_empty_row() {
        let rows = populated().rows(Order::Queue);
        assert!(rows.contains(&Row::Empty("invoices".to_string())));
    }

    #[test]
    fn sorts_every_failure_above_every_queued_file_by_status() {
        let rows = populated().rows(Order::Status);
        let statuses: Vec<&str> = rows
            .iter()
            .filter_map(Row::entry)
            .map(|entry| entry.status.as_str())
            .collect();
        assert_eq!(statuses, ["failed", "failed", "queued"]);
    }

    #[test]
    fn puts_empty_queues_last_when_not_ordered_by_queue() {
        let rows = populated().rows(Order::Age);
        assert_eq!(rows.last(), Some(&Row::Empty("invoices".to_string())));
    }

    #[test]
    fn cycles_through_every_order() {
        assert_eq!(Order::Queue.next(), Order::Age);
        assert_eq!(Order::Age.next(), Order::Status);
        assert_eq!(Order::Status.next(), Order::Queue);
    }

    #[test]
    fn the_cursor_starts_on_the_first_file_not_the_header() {
        let rows = populated().rows(Order::Queue);
        assert_eq!(first_file(&rows), Some(2));
    }

    #[test]
    fn stepping_never_lands_on_a_header_or_an_empty_row() {
        let rows = populated().rows(Order::Queue);
        let mut cursor = first_file(&rows).unwrap();
        for _ in 0..10 {
            cursor = step(&rows, cursor, 1).unwrap();
            assert!(rows[cursor].entry().is_some());
        }
        for _ in 0..10 {
            cursor = step(&rows, cursor, -1).unwrap();
            assert!(rows[cursor].entry().is_some());
        }
    }

    #[test]
    fn stepping_stops_at_the_ends_instead_of_wrapping() {
        let rows = populated().rows(Order::Queue);
        assert_eq!(step(&rows, first_file(&rows).unwrap(), -1), first_file(&rows));
        assert_eq!(step(&rows, last_file(&rows).unwrap(), 1), last_file(&rows));
    }

    #[test]
    fn a_jump_settles_onto_the_nearest_file_in_the_direction_of_travel() {
        let rows = populated().rows(Order::Queue);
        assert_eq!(settle(&rows, 0, true), first_file(&rows));
        assert_eq!(settle(&rows, 1, true), first_file(&rows));
        assert_eq!(settle(&rows, 999, true), last_file(&rows));
        assert_eq!(settle(&rows, 0, false), first_file(&rows));
    }

    #[test]
    fn a_page_jump_moves_by_many_files_at_once() {
        let rows = populated().rows(Order::Queue);
        assert_eq!(step(&rows, first_file(&rows).unwrap(), 100), last_file(&rows));
    }

    #[test]
    fn there_is_no_cursor_when_every_queue_is_empty() {
        let fixture = Fixture::new(&[("invoices", &[]), ("invoices-failed", &[])]);
        let rows = fixture.rows(Order::Queue);
        assert_eq!(first_file(&rows), None);
        assert_eq!(step(&rows, 0, 1), None);
        assert_eq!(settle(&rows, 0, true), None);
    }

    #[test]
    fn a_file_can_be_found_again_after_a_rescan() {
        let fixture = populated();
        let rows = fixture.rows(Order::Queue);
        let path = rows[2].entry().unwrap().path.clone();

        let after = fixture.rows(Order::Age);
        let found = relocate(&after, &path).unwrap();
        assert_eq!(after[found].entry().unwrap().path, path);
    }

    #[test]
    fn a_file_that_is_gone_cannot_be_found() {
        let rows = populated().rows(Order::Queue);
        assert_eq!(relocate(&rows, Path::new("/nowhere/gone.txt")), None);
    }
}
