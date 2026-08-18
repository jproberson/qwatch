# Design

## What it is for

Plenty of systems queue work by dropping files in a directory. A worker picks a
file up, renames or moves it, and moves it somewhere else when it fails. When
something gets stuck, whoever is on call needs to see what is sitting where,
read the payload, and put a failed item back in front of the workers. That is
usually an `ls`, a `cat`, and a nervous `mv`.

qwatch is that loop as a terminal browser: look, inspect, act, with the
destructive actions guarded so they refuse rather than guess.

## The model

Four concepts. Everything else is presentation.

**Root.** One directory holding the queues. Nothing outside it is ever read or
written.

**Queue.** A named group of work. One queue owns one directory per state.

**State.** A named stage a file can be in, backed by a directory.

**File.** One unit of work. Its state is the directory it sits in. Its filename
may carry more: which worker claimed it, what kind of job it is, its index.

A profile maps a real layout onto those four. This is one profile, not a
builtin:

```
ingest/                 root
  invoices/             queue "invoices", state "queued"
  invoices-failed/      queue "invoices", state "failed"
  receipts/
  receipts-failed/
```

And so is this:

```
jobs/                   root
  inbox/                queue "jobs", state "inbox"
  processing/
  failed/
```

### Discovering queues

Each state declares a directory template that may contain `{queue}`. Scanning
the root matches every directory against every template.

`invoices-failed` matches both `{queue}-failed` (queue `invoices`) and `{queue}`
(queue `invoices-failed`). **Most literal characters wins**, so the first
reading holds. A template that is exactly `{queue}` is always the last resort.

A template with no `{queue}` is a fixed directory belonging to a single queue
named after the root.

When a profile declares no states at all, every subdirectory of the root becomes
a state of one queue. When there are no subdirectories either, the root itself
is the only state. This is what makes `qwatch <directory>` work with no config.

### Deriving status

A file's *state* is structural. Its *status* is what the reader sees, and the
filename can refine it. Statuses match in declaration order, first match wins.
A file whose name the pattern rejects is `unknown`. A file that matches no
status takes the name of its state.

Colour follows meaning rather than position:

| Meaning | Colour |
| --- | --- |
| The state carries a priority, so its author flagged it | red |
| The status carries a `when` condition, so it is active | cyan |
| The state name reads as finished (`done`, `complete`, `archive`, …) | green |
| Anything else resting in a queue | amber |
| The filename pattern rejected the name | muted |

Green is reserved for work that is finished. A file merely sitting in a queue is
amber, because unclaimed work waiting to be picked up is mildly interesting and
green would say the opposite.

Colouring by state alone is the obvious mistake here: `waiting` and `running`
are two statuses of one `queued` state, so they would come out identical.

Any status can set `color` to override all of it. A colour is a name (`red`,
`orange`, `brightblue`, …), a number from 0 to 255 to pick out of the terminal
palette, or `#rrggbb`. Names from 0 to 15 resolve to the terminal's own palette
rather than fixed values, so a profile inherits the reader's theme.

### Display order

States are shown in declaration order, so a profile reads in pipeline order.
`priority` lifts a state above that order, which is how the state that needs
attention is shown first without writing the profile backwards.

### Filenames

A profile supplies a regex with named captures and a template to reassemble one.
Both directions are needed because actions rewrite names, and a regex alone
cannot render. Every placeholder used anywhere in a profile is checked against
the pattern's capture names at load time.

Sort key and age come from mtime. mtime survives rename and move on every
filesystem that matters here, so for a file written once at enqueue it is not an
approximation of enqueue time, it is enqueue time. Reading a timestamp out of
the filename is deliberately not implemented: it is redundant wherever mtime
holds, and `Entry::modified` is a single derived field, so it is cheap to add if
a layout ever rewrites files in place.

## Actions

Three primitives, each checkable, so each can be guarded.

| Type | Does |
| --- | --- |
| `move` | Send a file to another state, optionally rewriting captures |
| `delete` | Remove a file |
| `edit` | Open it in `$EDITOR` |

Guards are builtin and not configurable:

- The path must resolve inside the root, with `..` and symlinks refused
- A `move` that changes neither directory nor name refuses as a no-op
- A `move` refuses when a file of the target name already exists

The no-op check runs before the exists check, and both the source and the
target are built from the canonicalized root. Otherwise the two refusals blur
together: on a root reached through a symlink, a file compared against itself
fails the equality test and reports the wrong reason.

That last guard is what produces a good refusal for free. Restarting a file that
is already waiting to be picked up would land it in the directory it is already
in under the name it already has, so it is rejected with no special case. And
`move` with a rewrite covers both relocation and rename-in-place with one rule:
when the target state resolves to the directory the file is already in, it is a
rename.

### Scope

An action works on one file by default. `scope` widens it to a set defined by
the row under the cursor, which keeps bulk work in the same mental model as
single work: point at a file, then say how far the action reaches.

| Scope | Reaches |
| --- | --- |
| `one` | The file under the cursor. The default |
| `status` | Every file with the same status |
| `all` | Every file listed |

Three, because those are the three questions worth asking of a stuck queue:
this one, everything that failed the same way, everything. Scoping by queue or
by job name was written and then taken out again: both read as plausible, and
neither answered a question the other two did not.

```toml
[[profile.ingest.action]]
key   = "D"
name  = "delete"
type  = "delete"
scope = "all"
```

A scoped action is labelled by what it would do to the row under the cursor
rather than by the name of its scope, so the footer reads `delete 2 ParseInvoice`
and `restart 3 failed`, and the counts move as the cursor does. Naming them by
scope instead (`delete status`, `delete all`) tells the reader nothing: two keys
both read as some flavour of delete, and when only one file carries that status
the wider one is genuinely identical to the narrow one, which the count admits
and a scope name hides.

Every guard still runs per file, and a file that refuses is skipped rather than
failing the batch, because "restart everything that failed" should not be
stopped by one file that was already restarted. The prompt counts both: `restart
8 of 12 files that are failed?`, with the first few refusals and their reasons
underneath. If every file refuses, nothing is confirmed at all and the reason is
reported instead.

Bulk needs one guard that single work does not. Two files can plan a move to the
same name, which each check passes alone because the target does not exist yet,
and `rename` on Unix overwrites silently, so the second would destroy the first.
Targets claimed earlier in a batch are refused later in it.

An `edit` action cannot take a scope, since opening twelve files in an editor is
not a thing anyone means. That is refused when the config loads.

A fourth type, `command`, would run a shell template against the selected file.
It is deliberately not in the first version. It is the only action that cannot
be guarded, and shipping without it means every safety property here is
absolute.

## Configuration

TOML, at `$XDG_CONFIG_HOME/qwatch/config.toml`, overridden by `--config`.
Unknown fields are rejected rather than ignored, so a typo is an error.

**Zero config must work.** `qwatch ./dir` needs no profile at all.

```toml
[profile.ingest]
root = "~/work/ingest"

[[profile.ingest.state]]
name = "queued"
dir  = "{queue}"

[[profile.ingest.state]]
name     = "failed"
dir      = "{queue}-failed"
priority = 10

[profile.ingest.filename]
pattern  = '^(?<claim>[\dx])_(?<worker>\w+)_(?<job>[A-Za-z]\w*)-(?<index>\d+)\.json$'
template = "{claim}_{worker}_{job}-{index}.json"
label    = "{job}"
detail   = "#{index}"

[[profile.ingest.status]]
name  = "failed"
state = "failed"

[[profile.ingest.status]]
name  = "running"
state = "queued"
when  = { claim = '^\d+$' }
badge = "worker {claim}"

[[profile.ingest.status]]
name  = "waiting"
state = "queued"

[[profile.ingest.action]]
key      = "r"
name     = "restart"
type     = "move"
to_state = "queued"
set      = { claim = "x" }

[[profile.ingest.action]]
key  = "d"
name = "delete"
type = "delete"

[profile.ingest.ignore]
names = [".DS_Store"]

[profile.ingest.preview]
format = "delimited-json"
labels = ["job"]

[[profile.ingest.preview.detect]]
pattern = '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
label   = "id"
```

## Preview

Three formats, chosen per profile:

| Format | Shows |
| --- | --- |
| `raw` | The contents, unchanged. The default |
| `json` | Pretty printed, object keys sorted |
| `delimited-json` | A header of fields, then a pretty printed JSON tail |

`delimited-json` splits once on `split` (a tab by default) into a header and a
payload, then splits the header on `field_separator` (a comma). Each header
field is named by `labels` positionally, else by the first `detect` pattern it
matches, else `field N`. That is enough to name the fields worth naming without
pretending to understand the rest.

Object keys are sorted rather than left in file order, so the same payload always
renders the same way and a diff between two files means something.

A payload that will not parse is not an error: the reason is shown and the raw
text printed underneath, because a malformed payload is often exactly what you
opened the file to see.

Files are refused if they contain a NUL byte and truncated past 256 KB, so
opening the wrong thing cannot wedge the browser.

A `preview_command` escape hatch, in the shape of fzf's `--preview`, is
deliberately absent for the same reason `command` actions are: it runs arbitrary
shell, and here it would run on every cursor movement.

## UI

```
╭ ingest ─────────────────────────────────────────────╮╭ ExtractTotals #1 ─────────────────╮
│  QUEUE    STATUS  JOB                           AGE ││ job       ExtractTotals           │
│  invoices empty                                     ││ field 2   0                       │
│  receipts failed  ParseInvoice #0                0s ││ id        b27e4ac4-1111-2222-3333 │
│▌ receipts failed  ExtractTotals #1               0s ││ field 4   False                   │
│  receipts waiting RenderReport #0                0s ││                                   │
│                                                     ││ {                                 │
│                                                     ││   "AnalysisId": 4821,             │
│                                                     ││   "Retries": 2                    │
│                                                     ││ }                                 │
│                                                     ││                                   │
│                                                     ││                                   │
│                                                     ││                                   │
│                                                     ││                                   │
│                                                     ││                                   │
│                                                     ││                                   │
│                                                     ││                                   │
│                                                     ││                                   │
╰──────────────────────────────────────────── 3 files ╯╰───────────────────────────────────╯
 j/k move  r restart  d delete  R rescan  s sort:queue  ? help  q quit
```

A flat table on the left, one row per file with a queue column, and the preview
beside it. Both panes are boxed, which gives the root name and the selected
file's label somewhere to live, and the file count a home on the bottom edge.
The footer names the bound keys, so the help overlay is a courtesy rather than a
requirement.

No section headers: a queue column scales to many queues where stacked sections
do not, and it makes sorting by any column meaningful. `s` cycles between
ordering by queue (the default, which keeps a queue's rows together and its
attention states on top), by age across every queue, and by status.

A queue holding no files still gets a row, so you can see it exists. It is not
selectable, so it costs nothing to step past.

Behaviour, not configuration:

- The cursor rests only on file rows. Headers, blank lines and the empty marker
  are skipped by every movement, including page jumps, first, last and clicks,
  so every action always has a file under it
- The selected row is marked and emboldened rather than filled with a solid
  bar. A bar has to pick a background colour, and no reliable way exists to ask
  a terminal whether it is light or dark, so the one that looks right on the
  author's terminal looks wrong on somebody else's. A marker costs one column
  and lets every row keep its own colours
- A redraw is skipped when nothing visible changed, so reading a payload is not
  interrupted by an unrelated write under the root
- Text labels, never icons
- `NO_COLOR` is honoured

## Keys

Defaults are the vim ones, and every motion is rebindable under `[keys]`:

```toml
[profile.ingest.keys]
down = ["n", "ctrl-j"]
quit = ["ctrl-c"]
```

A binding is a character (`j`, `G`, case significant), a named key (`enter`,
`esc`, `pagedown`, `f5`, …), or either with a `ctrl-` or `alt-` prefix. A motion
that is not mentioned keeps its default, so rebinding one key does not silently
strip the rest. Getting that wrong is the obvious trap: a plain serde default
per field would leave every unmentioned motion bound to nothing.

Whatever the navigation keys end up being is what profile actions may not use,
rather than a fixed list. Rebinding `quit` to `ctrl-c` therefore frees `q` for
an action of your own, and a collision is refused when the config loads.

## Mouse

Clicking a row selects it, settling onto a real file when the click lands on a
header or below the last row. The wheel moves the cursor over the list and
scrolls the payload over the preview.

Capturing the mouse costs the terminal's own text selection, which matters in a
tool where copying a path is a normal thing to want, so `mouse = false` turns it
off.

## Watching

On by default, and tunable:

```toml
[profile.ingest.watch]
enabled     = true
debounce_ms = 120
backstop_ms = 4000    # 0 to trust the events alone
```

A burst of events collapses into one redraw, and the backstop looks anyway in
case an event never arrives.

## CLI

```
qwatch [DIRECTORY]        browse a directory with no config at all
qwatch --profile NAME     browse a profile from the config file
qwatch --config PATH      read a different config file
qwatch --list             list every file and exit
qwatch --json             list every file as JSON and exit
qwatch init [DIRECTORY]   look at a directory and write a starter config
qwatch init --output PATH  write it somewhere else, to share with a team
qwatch init --print        show it without writing anything
```

Listing is a flag rather than a subcommand because it modifies the same
operation. `init` is a subcommand because it is a different one: it writes a
config and never browses. The cost is that a directory literally named `init`
needs `./init`, which is worth it for a verb people expect to type.

### init

Writing the first profile is the real barrier to using this, so `init` reads the
directory and does as much of it as can be done honestly.

It looks for a suffix that pairs sibling directories, counting every candidate
and taking the most common one, so `jobs` + `jobs-deadletters` is recognised as
a pair even when `jobs-long` and `jobs-mjd` exist to confuse it. Failing that,
each subdirectory becomes a state. A state whose name looks like a failure
(`fail`, `error`, `dead`, `reject`, `quarantine`, `poison`, `stuck`, `retry`,
`invalid`) is given a priority, so it sorts and colours first.

What it will not do is guess a filename pattern. Instead it prints one of the
real filenames it found as a comment above a worked example, so the reader can
see exactly what they are writing a pattern against. A generated config always
loads and validates, which is checked by its tests.

`--json` exists so the scan layer is testable without a terminal and so the tool
is scriptable. It costs about fifty lines and pays for itself twice.

## Code

Zero comments is the goal, so names carry intent. The exception is knowledge
about the world rather than about the code, which lives here rather than
evaporating:

- **Why the watcher will need a backstop timer.** Filesystem events are not a
  guarantee. macOS coalesces and can drop them, a network mount may never send
  one, and a watch misses whatever happens in the moment before it arms. A
  backstop pass that finds nothing costs one directory read and no redraw.
- **Why mtime is trusted as enqueue time.** See above.
- **Why directory matching is most-specific-wins.** Otherwise a failure
  directory reads as a queue of its own.

```
src/
  main.rs      arguments, wiring
  config.rs    profile types, loading, validation
  name.rs      templates and patterns, parse and render
  status.rs    state plus filename -> status
  scan.rs      root -> queues -> files
  action.rs    primitives and guards
  watch.rs     events, debounce, backstop
  preview.rs   contents -> lines
  ui.rs        state, keys, event loop
  ui/table.rs  queues -> rows, cursor movement
  ui/render.rs rows -> frame
  ui/theme.rs  colours
```
