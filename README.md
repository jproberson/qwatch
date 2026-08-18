# qwatch

Browse and unstick file-based work queues from the terminal.

Some systems queue work by dropping files in a directory: a worker picks one up,
renames it, and moves it aside when it fails. qwatch shows what is sitting
where, lets you read a file's contents, and puts failed work back in front of
the workers without you hand-writing an `mv`.

**Status: usable.** Every layer is built and tested: scanning, config,
actions with their guards, watching, preview rendering, and the browser itself.
Rough edges remain, and it has not been used in anger yet.

## Try it

No configuration needed:

```
qwatch ~/some/queue/directory --list
```

Every subdirectory is read as a state, or the directory itself if it has none.

## Configure it

`qwatch init` looks at a directory, works out how it is laid out, and writes a
starter config:

```
qwatch init ~/work/ingest                       # writes it to your config
qwatch init ~/work/ingest --print               # just shows it
qwatch init ~/work/ingest -o team-queues.toml   # somewhere you can share it
```

A profile written with `-o` is a plain file you can check into a team's own
repo, so everyone browses the same layout with `qwatch --config`.

It finds the suffix that pairs your queue directories with their failure
siblings, flags the failure state so it sorts first, and prints one of your real
filenames as a comment next to a worked example, so you can see what you are
writing a pattern against. It will not invent a pattern for you.

A profile teaches qwatch a real layout: which directories pair into a queue,
what the filenames mean, and what you are allowed to do to a file. See
[DESIGN.md](DESIGN.md) for the full format and the reasoning behind it.

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
```

That reads `ingest/invoices/` and `ingest/invoices-failed/` as one queue with
two states, and shows the failed ones first.

Config lives at `$XDG_CONFIG_HOME/qwatch/config.toml`, or pass `--config`.

## Why it works the way it does

**Directory matching is most-specific-wins.** `invoices-failed` matches both
`{queue}-failed` and `{queue}`. The template with more literal characters wins,
otherwise a failure directory would read as a queue of its own.

**mtime is treated as enqueue time.** Rename and move both preserve mtime, so
for a file written once when it is enqueued, mtime is not an approximation of
when it was enqueued, it is exactly that. Filename timestamps are not parsed
because they would be redundant.

**Destructive actions refuse rather than guess.** Every path must resolve inside
the root. A move refuses when something already occupies the target name, and
refuses as a no-op when it would change neither directory nor name. Actions
cannot run arbitrary shell commands, so those guarantees hold absolutely.

## Acting on many files at once

Any action can reach past the file under the cursor:

```toml
[[profile.ingest.action]]
key   = "D"
name  = "delete"
type  = "delete"
scope = "all"        # or queue, status, job
```

Each one is labelled by what it would do to the row you are on, so the footer
reads `delete 2 ParseInvoice` and `restart 3 failed` rather than something vague
about scopes, and the counts follow the cursor.

Guards still run per file, so one that refuses is skipped and counted rather
than stopping the rest, and the prompt tells you how many of each before
anything happens.

## Keys

vim defaults, all rebindable:

| Key | Does |
| --- | --- |
| `j` `k` `up` `down` | Move between files |
| `g` `G` | First file, last file |
| `J` `K` | Scroll the preview |
| `enter` | Open in `$EDITOR` |
| `R` | Rescan now |
| `s` | Change sort order |
| `t` | Cycle the layout: table, by queue, by status |
| `ctrl-s` | Settings: layout, sort, queues, watching, keys |
| `?` | Show the keys |
| `q` `esc` | Quit |

`ctrl-s` opens a settings panel with a section per thing worth changing: layout,
sort, watching, and keys. Rebinding is done there too, by pressing `enter` on a
motion and then the key you want. Choices are remembered in `remembered.toml`
beside your config, so they survive a restart without your config file being
rewritten.

Actions from your profile bind their own keys on top. Clicking a row selects it,
and the wheel scrolls whichever pane is under the pointer.

```toml
[profile.ingest.keys]
down = ["n", "ctrl-j"]
quit = ["ctrl-c"]        # this frees up `q` for an action of your own
```

## Install

You need Rust. [rustup.rs](https://rustup.rs) if you have not got it.

```
cargo install --git https://github.com/jproberson/qwatch
```

Run the same command again to update, or open `ctrl-s` and pick **about**, which
does it for you. Either way your config and settings are left alone: cargo
replaces a binary and never touches `~/.config/qwatch`. From a checkout,
`cargo install --path .` does the same thing.

Then `qwatch init <your queue directory>` to write a starting config, and
`qwatch` to browse it.

For tab completion:

```
qwatch completions zsh  > ~/.zfunc/_qwatch          # or bash, fish, elvish, powershell
```

## Build

```
cargo build --release
cargo test
```

## License

MIT. See [LICENSE](LICENSE).
