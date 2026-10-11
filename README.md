# rotatelog

`logrotate` is the standard answer to "this file keeps growing forever,"
but it's a C daemon-adjacent tool built around `/etc/logrotate.d/` config
snippets, `cron`/`systemd.timer` wiring, and (usually) syslog integration.
If you just have one file some non-syslog process is appending to — a
one-off script's log, a sidecar's output, anything not already plumbed
into `/etc/logrotate.d/` — there's no equivalent you can just point at a
path and run. `rotatelog` is that: a single binary, one file, one command,
no config file to write first.

## Installation

```sh
cargo install --path .
```

## Usage

```bash
rotatelog app.log --max-size 10M --max-count 5
rotatelog app.log --max-age 1d --max-count 7 --compress
rotatelog app.log --max-size 100M --force        # rotate unconditionally
```

Typical use is a cron job or systemd timer running once a minute/hour;
`rotatelog` itself does nothing until the file actually crosses a
threshold, so calling it constantly is cheap and safe:

```cron
* * * * * /usr/local/bin/rotatelog /var/log/myapp/app.log --max-size 50M --max-count 10 --compress
```

```
$ rotatelog app.log --max-size 1K --max-count 2
rotatelog: rotated app.log (2.0 KiB) -> app.log.1
rotatelog:   shifted app.log.1 -> app.log.2
rotatelog:   pruned app.log.2 (past --max-count 2)
```

`--max-size` accepts a bare byte count or a `K`/`M`/`G` suffix
(case-insensitive, binary/1024-based — `10M` = 10,485,760 bytes).
`--max-age` accepts `s`/`m`/`h`/`d`/`w` suffixes (a bare number is
seconds), measured against the file's mtime.

## How rotation works

Either threshold is independently sufficient — `--max-size 10M --max-age
1d` rotates on whichever comes first. On rotation:

1. Every existing `app.log.N` (or `app.log.N.gz`) is renumbered up by one,
   highest index first so nothing gets clobbered mid-shift.
2. Any file whose new number would exceed `--max-count` is deleted instead
   of renamed — the oldest generation, pruned.
3. The live `app.log` is moved to `app.log.1`.
4. A fresh, empty `app.log` is created at the original path.
5. If `--compress` is set, the new `app.log.1` is gzipped to
   `app.log.1.gz` (real gzip format — `gunzip`/`zcat` read it directly)
   and the uncompressed copy is removed. Already-compressed older
   generations keep their `.gz` suffix as they shift up.

Running with neither `--max-size` nor `--max-age` set is refused with a
clear error unless `--force` is also passed, so a typo'd cron line doesn't
silently rotate on every single invocation.

## Status: built and verified against real files on disk

- **39 unit tests** (`cargo test --lib`) across four modules:
  - `size`: byte/`K`/`M`/`G`/`B` parsing (case-insensitive), overflow and
    garbage-input rejection, and the human-readable formatter used in
    output (`2048` -> `"2.0 KiB"`).
  - `duration`: `s`/`m`/`h`/`d`/`w` suffix parsing, bare numbers as
    seconds, garbage rejection.
  - `compress`: a real file gzip-compressed then decoded back with
    `flate2`'s own decoder to confirm byte-for-byte round-trip fidelity,
    a check that repetitive content actually shrinks, and that the
    uncompressed source is removed only after compression succeeds.
  - `rotate`: threshold detection with an injectable "now" (so age tests
    don't depend on real elapsed wall-clock time), the full shift/prune
    numbering logic, `.gz` suffix preservation across multiple shifts,
    unrelated same-directory files (`app.log.bak`, a different base name)
    left untouched, and clean errors (not panics) on a missing file.
- **Live end-to-end verification against the compiled binary and real
  files**, not just unit tests:
  - Grew a real scratch file past a configured `--max-size 1K`, ran the
    actual binary three times in a row with `--max-count 2`, and checked
    real file contents on disk after each run: `app.log.1` held the most
    recent generation's exact bytes, `app.log.2` the one before it, and
    the run that would have produced a third generation instead genuinely
    deleted the oldest file (confirmed with `ls` — no `app.log.3` ever
    existed).
  - Ran with `--compress`, then confirmed with the real `file` command
    that `app.log.1.gz` is `gzip compressed data`, and with real `gunzip
    -c` that decompressing it reproduces the exact original line.
  - Confirmed the below-threshold path is a genuine no-op (`--max-size
    10M` against a 6-byte file left the file and its mtime untouched,
    printed a "not rotating" message, exit code 0).
  - Confirmed `--force` rotates that same tiny file unconditionally.
  - Confirmed calling `rotatelog` with no `--max-size`/`--max-age`/
    `--force` at all fails with a clear message and exit code 1, rather
    than silently rotating.

**Not done / deliberately deferred**: no `copytruncate` mode and no
reopen-signal support — a long-lived process holding `app.log` open by
file descriptor (not by path) will keep writing into the now-renamed
`app.log.1` until it reopens the path itself, exactly like base
`logrotate` without `copytruncate`/a postrotate hook; that hook mechanism
(`--exec-after <cmd>` to `kill -HUP`/restart the writer) isn't built
either. No syslog/journald integration — this operates on plain files
only. No config file / multiple-file batch mode (`logrotate`'s
`/etc/logrotate.d/*` directory of stanzas) — one file, one set of flags,
per invocation; if you have many files, call it once per file (a shell
loop or several cron lines), which is also simpler to reason about than a
shared config.
