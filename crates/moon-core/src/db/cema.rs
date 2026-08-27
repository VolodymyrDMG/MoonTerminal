//! FORK: harvesting CustomEMA expression values from the terminal's own core-log files.
//!
//! The bot evaluates a strategy's CustomEMA expressions on every task and PRINTS the values into
//! its server log — `... (48923) EMAFilter: Min(12hours, 1sec) = 1.64%  BTC(30sec, 1sec) = 0.03%`
//! — but never puts them in the report row, so the tuner had nothing to sweep. The terminal
//! already receives that log live and files it per core per day (`applog::DatedWriter`, at
//! `logs/<date>_<core>.log`), and the task number in parentheses is the same `taskid` the report
//! row carries. This module tails those files, keeps the values of tasks that BECAME DEALS in
//! `cema_vals` inside reports.sqlite, and the tuner LEFT-JOINs them by `(core_uid, taskid)` (see
//! `tuner::mod::cema_wrapped`).
//!
//! TWO sources feed the same table, both harvested by the sweep:
//!
//! 1. The deal's own COMMENT: MoonStrike-family strategies stamp the whole detect context —
//!    including the evaluated expressions — into the report row's `comment`. That is the best
//!    source there is: exact, keyed to the deal it describes, and as deep as the report history
//!    itself, so the first sweep backfills months, not days. Scanned incrementally by rowid
//!    (the mark rides `cema_scan` under [`COMMENTS_MARK`]); an upsert of an existing row keeps
//!    its rowid, which is fine — the expressions are stamped at BUY, on the row's first insert.
//! 2. The per-core LOG files, for strategies whose comments do not carry the values (the bot
//!    prints an `EMAFilter:` line on every task either way). File-based deliberately — the files
//!    survive restarts, one mechanism serves the backfill and the steady drip, and with file
//!    logging disabled in settings this source simply contributes nothing.
//!
//! Volume control: only a small fraction of EMAFilter lines belong to tasks that actually bought
//! (~6k lines/day/core against dozens of deals), so the sweep filters against the replica's
//! `taskid` set per core and DROPS the rest. A line too YOUNG to judge — its deal row may still
//! be in flight, or catch-up may still be replaying the period — stalls the file's offset instead
//! of being dropped, and is re-read on the next sweep until it matches or outgrows the grace
//! window. Offsets therefore never advance past an undecided line, which is what makes the
//! harvest exactly-once-ish without a staging table.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use rusqlite::Connection;

use super::tuner::CEMA_FIELDS;

/// One harvested value: a deal's task evaluated one expression to `v` percent.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub core_uid: u64,
    pub taskid: i64,
    /// Key from [`CEMA_FIELDS`] (`min12h`, `min5h`, `min45m`, `btc30s`).
    pub key: &'static str,
    pub v: f64,
    /// Unix milliseconds of the LINE (file date plus the line's clock), for retention.
    pub ts: i64,
}

/// One server whose log files the sweep may attribute, resolved by the label
/// `applog::sanitize_label` gives its name.
#[derive(Clone, Debug)]
pub struct SweepServer {
    pub uid: u64,
    pub name: String,
}

/// How long values are kept, in days. Wider than the default 14-day log retention so a user who
/// raised that setting keeps the depth, and old enough that a quarter of tuning history survives.
const VALS_KEEP_DAYS: i64 = 90;

/// Offset rows for files older than this are dropped; the files themselves are long purged.
const SCAN_KEEP_DAYS: i64 = 60;

/// Log files older than this many days are not swept even if the user keeps files longer:
/// values older than the tuner's practical horizon are not worth the first-run parse.
const SWEEP_HORIZON_DAYS: i64 = 45;

/// `cema_scan` key of the deal-comment scan's rowid mark. Underscores sort ABOVE the date-named
/// log files, so the date-prefix retention cut cannot reach it — and the prune excludes it
/// explicitly anyway.
const COMMENTS_MARK: &str = "__comments__";

/// Deal-comment rows read per chunk, and chunks per sweep. Bounds the first run over a
/// half-million-row replica to a few unnoticeable seconds a minute until the mark catches up.
const COMMENT_CHUNK: i64 = 20_000;
const COMMENT_CHUNKS_PER_SWEEP: usize = 3;

/// How long an EMAFilter line may wait for its deal row before the sweep drops it. Covers the
/// buy-to-upsert gap (seconds) and a report catch-up replaying the recent past after a restart.
const MATCH_GRACE_MS: i64 = 30 * 60 * 1000;

/// Create the value and offset tables. Runs on the writer connection at startup.
pub(super) fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS cema_vals (
             core_uid INTEGER NOT NULL,
             taskid INTEGER NOT NULL,
             k TEXT NOT NULL,
             v REAL NOT NULL,
             ts INTEGER NOT NULL,
             PRIMARY KEY (core_uid, taskid, k)
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS cema_scan (
             file TEXT PRIMARY KEY,
             off INTEGER NOT NULL
         ) WITHOUT ROWID;
         -- Same DDL as init_db: normally app_meta already exists, but this module must also
         -- initialize a bare connection (tests, tools) without ordering assumptions.
         CREATE TABLE IF NOT EXISTS app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         -- One-time repair, flagged in app_meta so it runs ONCE: the first comment pass
         -- compared SECOND-valued buy dates against a millisecond cutoff, harvested nothing,
         -- and still parked its mark at the frontier. Dropping the mark makes the fixed pass
         -- re-read the whole history; the upserts are idempotent, so rows the log sweep
         -- already owns are simply confirmed.
         DELETE FROM cema_scan
          WHERE file = '__comments__'
            AND NOT EXISTS (SELECT 1 FROM app_meta WHERE key = 'cema_secs_fix');
         INSERT OR IGNORE INTO app_meta(key, value) VALUES('cema_secs_fix', '1');",
    )
}

/// Apply one sweep batch: upsert values, remember file offsets, prune what aged out.
///
/// Idempotent by construction — a re-read line REPLACEs its own key — so a stalled offset
/// re-delivering rows is harmless.
///
/// Args:
///     conn: Writer connection inside the batch transaction.
///     rows: Deal-matched values.
///     offsets: `(file name, next byte)` per swept file.
///
/// Returns:
///     Whether any VALUE row was written (offset-only batches change nothing the tuner reads).
pub(super) fn apply(
    conn: &Connection,
    rows: &[Row],
    offsets: &[(String, u64)],
) -> rusqlite::Result<bool> {
    {
        let mut ins = conn.prepare_cached(
            "INSERT OR REPLACE INTO cema_vals (core_uid, taskid, k, v, ts)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for r in rows {
            ins.execute(rusqlite::params![
                r.core_uid as i64,
                r.taskid,
                r.key,
                r.v,
                r.ts
            ])?;
        }
    }
    {
        let mut off = conn
            .prepare_cached("INSERT OR REPLACE INTO cema_scan (file, off) VALUES (?1, ?2)")?;
        for (file, o) in offsets {
            off.execute(rusqlite::params![file, *o as i64])?;
        }
    }
    let now = crate::util::now_unix_ms_i64();
    conn.execute(
        "DELETE FROM cema_vals WHERE ts < ?1",
        [now - VALS_KEEP_DAYS * 86_400_000],
    )?;
    // File names start with their date, so the lexicographic cut IS the date cut. The comment
    // mark is excluded by name — its underscores sort above every date anyway, but the intent
    // deserves to be written down rather than inferred from ASCII.
    if let Some(cut) = date_of_unix_ms(now - SCAN_KEEP_DAYS * 86_400_000) {
        conn.execute(
            "DELETE FROM cema_scan WHERE file < ?1 AND file != ?2",
            rusqlite::params![cut, COMMENTS_MARK],
        )?;
    }
    Ok(!rows.is_empty())
}

/// Read one chunk of deal comments past the rowid mark and harvest their expression values.
///
/// The comment is the deal's own record, so no task matching and no server mapping is needed:
/// `core_uid`, `taskid` and the buy time ride the same row. Rows older than the value retention
/// are skipped in SQL — parsing them would insert values the same batch immediately prunes.
///
/// Args:
///     conn: Read-only reports connection.
///     from_rowid: Scan strictly after this rowid.
///     now_ms: Sweep clock, for the retention cut.
///
/// Returns:
///     Harvested rows, the last rowid seen (the new mark), and whether the chunk was FULL —
///     `None` when the replica lacks the needed columns or cannot answer.
fn comment_chunk(
    conn: &Connection,
    from_rowid: i64,
    now_ms: i64,
) -> Option<(Vec<Row>, i64, bool)> {
    // Report dates are UNIX SECONDS (see `analytics::time_zone` and the valuation worker's
    // `closedate.div_euclid(60)`), so the retention cut is applied in seconds and the harvested
    // stamp is scaled to the milliseconds every other `cema_vals` row uses. The first version
    // compared seconds against a millisecond cutoff, called every deal ancient, and harvested
    // NOTHING — see the mark repair in `init`.
    let cutoff_secs = now_ms / 1_000 - VALS_KEEP_DAYS * 86_400;
    // The frontier is snapshotted BEFORE the read, and the chunk is bounded to it: rows the
    // retention cut filters out must still advance the mark (or every sweep re-walks them), and
    // jumping to a frontier read AFTER the chunk would skip rows the writer landed mid-sweep.
    let frontier: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(rowid), ?1) FROM orders_rep",
            [from_rowid],
            |r| r.get(0),
        )
        .ok()?;
    let mut stmt = conn
        .prepare(
            "SELECT rowid, core_uid, taskid, comment,
                    COALESCE(NULLIF(buydate, 0), NULLIF(closedate, 0), 0)
             FROM orders_rep
             WHERE rowid > ?1 AND rowid <= ?2 AND taskid > 0 AND comment IS NOT NULL
               AND instr(comment, '(') > 0
               AND COALESCE(NULLIF(buydate, 0), NULLIF(closedate, 0), 0) >= ?3
             ORDER BY rowid LIMIT ?4",
        )
        .ok()?;
    let mut rows_out: Vec<Row> = Vec::new();
    let mut last = from_rowid;
    let mut seen = 0i64;
    let found = stmt
        .query_map(
            rusqlite::params![from_rowid, frontier, cutoff_secs, COMMENT_CHUNK],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            },
        )
        .ok()?;
    for row in found.flatten() {
        let (rowid, core_uid, taskid, comment, ts_secs) = row;
        last = last.max(rowid);
        seen += 1;
        for (key, v) in parse_values(&comment) {
            rows_out.push(Row {
                core_uid: core_uid as u64,
                taskid,
                key,
                v,
                ts: ts_secs * 1_000,
            });
        }
    }
    let full = seen >= COMMENT_CHUNK;
    if !full {
        // Everything up to the frontier is decided: matched rows are harvested above, the rest
        // carry nothing worth returning for.
        last = last.max(frontier);
    }
    Some((rows_out, last, full))
}

/// The value key for one logged expression, or `None` for an expression the tuner does not carry.
fn key_for(func: &str, window: &str) -> Option<&'static str> {
    let key = if func.eq_ignore_ascii_case("Min") {
        if window.eq_ignore_ascii_case("12hours") {
            "min12h"
        } else if window.eq_ignore_ascii_case("5hours") {
            "min5h"
        } else if window.eq_ignore_ascii_case("45min") {
            "min45m"
        } else {
            return None;
        }
    } else if func.eq_ignore_ascii_case("BTC") && window.eq_ignore_ascii_case("30sec") {
        "btc30s"
    } else if func.eq_ignore_ascii_case("Avg") {
        // The EMADetection echo of the Mast/hook strategies: short averaging windows.
        if window.eq_ignore_ascii_case("2sec") {
            "avg2s"
        } else if window.eq_ignore_ascii_case("5sec") {
            "avg5s"
        } else if window.eq_ignore_ascii_case("20sec") {
            "avg20s"
        } else if window.eq_ignore_ascii_case("40sec") {
            "avg40s"
        } else {
            return None;
        }
    } else {
        return None;
    };
    // The parser and the join must agree on every key, or a harvested value could never be read.
    debug_assert!(CEMA_FIELDS.iter().any(|(_, k)| *k == key));
    Some(key)
}

/// Parse one core log MESSAGE (no file columns) into its task id and expression values.
///
/// Expected shape, from the bot verbatim:
/// `00:00:58.558  EPIC: [0] (177) EMAFilter: Min(5hours, 1sec) = -1.25%  BTC(30sec, 1sec) = 0.00%`
/// The task id is the last parenthesized number BEFORE the `EMAFilter:` marker; each expression is
/// `Func(window, granularity) = value%`. Unknown functions or windows are skipped, a line with no
/// recognized expression answers `None`, and the granularity argument is deliberately ignored.
pub fn parse_emafilter(msg: &str) -> Option<(i64, Vec<(&'static str, f64)>)> {
    // Two spellings of the same echo: strike-family strategies print `EMAFilter:`, the
    // Mast/hook family prints `EMADetection:` — the tail format is identical.
    let (marker, len) = match msg.find("EMAFilter:") {
        Some(at) => (at, "EMAFilter:".len()),
        None => (msg.find("EMADetection:")?, "EMADetection:".len()),
    };
    let head = &msg[..marker];
    let close = head.rfind(')')?;
    let open = head[..close].rfind('(')?;
    let taskid: i64 = head[open + 1..close].trim().parse().ok()?;
    let vals = parse_values(&msg[marker + len..]);
    (!vals.is_empty()).then_some((taskid, vals))
}

/// Scan free text for `Func(window, granularity) = value%` expressions the tuner carries.
///
/// Shared by the log-line parser and the deal-comment parser: a MoonStrike comment embeds the
/// expressions mid-sentence between `Vol: … $` and `CPU: …`, so this scans EVERY parenthesis and
/// keeps only the shapes it knows. Junk parentheses (`(strategy <X>)`, `(-4.2% depth)`,
/// `(Avg: 3)`) fail the `= value%` tail or the name test and are skipped without derailing the
/// scan; a repeated expression keeps its LAST value.
pub fn parse_values(text: &str) -> Vec<(&'static str, f64)> {
    let mut vals: Vec<(&'static str, f64)> = Vec::new();
    let mut rest = text;
    while let Some(par) = rest.find('(') {
        // The function name is the alphanumeric run touching the parenthesis.
        let name_at = rest[..par]
            .rfind(|c: char| !c.is_ascii_alphanumeric())
            .map_or(0, |i| i + 1);
        let func = &rest[name_at..par];
        let Some(close_rel) = rest[par..].find(')') else {
            break;
        };
        let close = par + close_rel;
        let window = rest[par + 1..close].split(',').next().unwrap_or("").trim();
        // ` = -1.25%` after the parenthesis; anything else means this parenthesis was not an
        // expression, and the scan continues after it.
        let mut next = close + 1;
        let after = rest[close + 1..].trim_start();
        if let Some(v_str) = after.strip_prefix('=') {
            let v_str = v_str.trim_start();
            if let Some(pct) = v_str.find('%') {
                if let Ok(v) = v_str[..pct].trim().parse::<f64>() {
                    if let Some(key) = key_for(func, window) {
                        // One key per line wins LAST, matching how the replace-into behaves.
                        vals.retain(|(k, _)| *k != key);
                        vals.push((key, v));
                    }
                    // Continue after the value, not after the parenthesis.
                    let consumed = rest[close + 1..].len() - after.len() // the whitespace
                        + (after.len() - v_str.len())                    // the '=' and spaces
                        + pct
                        + 1;
                    next = close + 1 + consumed;
                }
            }
        }
        rest = &rest[next..];
    }
    vals
}

/// Split one FILE line into `(milliseconds of day, message)`.
///
/// `DatedWriter` writes `HH:MM:SS.mmm\tLEVEL\ttarget\tmessage`; a line that does not parse —
/// someone else's file, a truncated tail — is skipped by answering `None`.
fn split_file_line(line: &str) -> Option<(i64, &str)> {
    let mut parts = line.splitn(4, '\t');
    let hms = parts.next()?;
    let _level = parts.next()?;
    let _target = parts.next()?;
    let msg = parts.next()?;
    Some((ms_of_day(hms)?, msg))
}

/// `HH:MM:SS[.mmm]` into milliseconds of the day.
fn ms_of_day(hms: &str) -> Option<i64> {
    let (clock, ms) = match hms.split_once('.') {
        Some((c, m)) => (c, m.parse::<i64>().ok().filter(|m| *m < 1_000)?),
        None => (hms, 0),
    };
    let mut it = clock.splitn(3, ':');
    let h: i64 = it.next()?.parse().ok().filter(|h| *h < 24)?;
    let m: i64 = it.next()?.parse().ok().filter(|m| *m < 60)?;
    let s: i64 = it.next()?.parse().ok().filter(|s| *s < 60)?;
    Some(((h * 60 + m) * 60 + s) * 1_000 + ms)
}

/// Unix milliseconds of `YYYY-MM-DD` at 00:00 UTC, `None` for anything else.
fn date_unix_ms(date: &str) -> Option<i64> {
    let b = date.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y: i64 = date[0..4].parse().ok()?;
    let m: i64 = date[5..7].parse().ok().filter(|m| (1..=12).contains(m))?;
    let d: i64 = date[8..10].parse().ok().filter(|d| (1..=31).contains(d))?;
    // Howard Hinnant's days_from_civil, the standard branchless civil-date conversion.
    let y_adj = y - i64::from(m <= 2);
    let era = y_adj.div_euclid(400);
    let yoe = y_adj - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400_000)
}

/// `YYYY-MM-DD` of a unix-millisecond stamp, the exact inverse of [`date_unix_ms`].
fn date_of_unix_ms(ms: i64) -> Option<String> {
    let days = ms.div_euclid(86_400_000) + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + i64::from(m <= 2);
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// What one file contributed to a sweep.
struct FileTake {
    rows: Vec<Row>,
    /// Next byte to read, only ever at a line boundary the sweep has DECIDED everything before.
    next_off: u64,
}

/// Scan one log file from `from_off`, deciding each EMAFilter line against the core's deal set.
///
/// Args:
///     path: The `<date>_<label>.log` file.
///     from_off: First byte to read, from `cema_scan` (0 for a new file).
///     core_uid: Core the file's label resolved to.
///     date_ms: The file's date at midnight, unix milliseconds.
///     deal_tasks: Task ids of this core's deals in the replica.
///     now_ms: The sweep's clock, for the match grace.
///
/// Returns:
///     Matched rows and the offset to persist, or `None` when the file cannot be read.
fn scan_file(
    path: &Path,
    from_off: u64,
    core_uid: u64,
    date_ms: i64,
    deal_tasks: &HashSet<i64>,
    now_ms: i64,
) -> Option<FileTake> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    // A shrunk file is not ours to reason about (retention rewrote it?): start over.
    let mut off = if from_off > len { 0 } else { from_off };
    file.seek(SeekFrom::Start(off)).ok()?;
    let mut reader = BufReader::new(file);
    let mut rows = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf).ok()?;
        if n == 0 {
            break;
        }
        if *buf.last().unwrap() != b'\n' {
            // A partial tail the writer is still appending: leave it for the next sweep.
            break;
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some((day_ms, msg)) = split_file_line(line) {
            if let Some((taskid, vals)) = parse_emafilter(msg) {
                let ts = date_ms + day_ms;
                if deal_tasks.contains(&taskid) {
                    for (key, v) in vals {
                        rows.push(Row {
                            core_uid,
                            taskid,
                            key,
                            v,
                            ts,
                        });
                    }
                } else if now_ms - ts < MATCH_GRACE_MS {
                    // Too young to drop: its deal row may still be in flight. The offset stays
                    // BEFORE this line, so the next sweep re-reads it; everything already taken
                    // above re-applies idempotently.
                    break;
                }
                // Older than the grace and still no deal: the task never bought. Dropped.
            }
        }
        off += n as u64;
    }
    Some(FileTake {
        rows,
        next_off: off,
    })
}

/// One full sweep: tail every attributable log file and send the batch to the report writer.
///
/// Reads offsets and deal sets through a read-only connection; all writing happens on the writer
/// thread via [`super::DbMsg::Cema`]. Quiet on every failure — the sweep runs again in a minute.
pub fn sweep_and_send(servers: &[SweepServer], sink: &super::ReportSink) {
    let Ok(conn) = super::open_readonly() else {
        return;
    };
    let dir = crate::config::paths::logs_dir_no_create();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let now_ms = crate::util::now_unix_ms_i64();
    let horizon = now_ms - SWEEP_HORIZON_DAYS * 86_400_000;
    // Labels are the sanitized server names; identical sanitizations collide, in which case the
    // file is attributed to the FIRST server carrying it — same ambiguity the log files
    // themselves have.
    let labels: Vec<(String, u64)> = servers
        .iter()
        .map(|s| (crate::applog::sanitize_label(&s.name), s.uid))
        .collect();
    let mut rows: Vec<Row> = Vec::new();
    let mut offsets: Vec<(String, u64)> = Vec::new();
    let mut deal_cache: std::collections::HashMap<u64, HashSet<i64>> =
        std::collections::HashMap::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".log") else {
            continue;
        };
        if stem.len() < 12 || stem.as_bytes().get(10) != Some(&b'_') {
            continue;
        }
        let (date, label) = (&stem[..10], &stem[11..]);
        let Some(date_ms) = date_unix_ms(date) else {
            continue;
        };
        if date_ms < horizon {
            continue;
        }
        let Some(uid) = labels.iter().find(|(l, _)| l == label).map(|(_, u)| *u) else {
            continue; // The app log, or a server this terminal no longer knows.
        };
        let from_off = conn
            .query_row(
                "SELECT off FROM cema_scan WHERE file = ?1",
                [name],
                |r| r.get::<_, i64>(0),
            )
            .ok()
            .map_or(0, |o| o.max(0) as u64);
        let deal_tasks = deal_cache.entry(uid).or_insert_with(|| {
            let mut set = HashSet::new();
            let Ok(mut stmt) = conn.prepare(
                "SELECT DISTINCT taskid FROM orders_rep
                 WHERE core_uid = ?1 AND taskid > 0",
            ) else {
                return set;
            };
            let Ok(found) = stmt.query_map([uid as i64], |r| r.get::<_, i64>(0)) else {
                return set;
            };
            set.extend(found.flatten());
            set
        });
        let Some(take) = scan_file(
            &entry.path(),
            from_off,
            uid,
            date_ms,
            deal_tasks,
            now_ms,
        ) else {
            continue;
        };
        rows.extend(take.rows);
        if take.next_off != from_off {
            offsets.push((name.to_string(), take.next_off));
        }
    }
    // Source two: the deals' own comments, scanned incrementally by rowid. Bounded per sweep so
    // the first pass over a large replica spreads across a few minutes instead of one long stall.
    {
        let mut mark = conn
            .query_row(
                "SELECT off FROM cema_scan WHERE file = ?1",
                [COMMENTS_MARK],
                |r| r.get::<_, i64>(0),
            )
            .ok()
            .unwrap_or(0)
            .max(0);
        let start = mark;
        for _ in 0..COMMENT_CHUNKS_PER_SWEEP {
            let Some((chunk_rows, last, full)) = comment_chunk(&conn, mark, now_ms) else {
                break; // No comment/taskid columns yet — a fresh replica contributes later.
            };
            rows.extend(chunk_rows);
            mark = mark.max(last);
            if !full {
                break;
            }
        }
        if mark > start {
            offsets.push((COMMENTS_MARK.to_string(), mark as u64));
        }
    }
    if rows.is_empty() && offsets.is_empty() {
        return;
    }
    sink.send(super::DbMsg::Cema { rows, offsets });
}

/// Spawn the background sweeper: one pass shortly after startup (the writer needs a moment to
/// create the tables), then one per minute for as long as the process lives.
pub fn spawn_sweeper(servers: Vec<SweepServer>, sink: super::ReportSink) {
    if servers.is_empty() {
        return;
    }
    std::thread::Builder::new()
        .name("cema-sweep".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(5));
            loop {
                sweep_and_send(&servers, &sink);
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        })
        .ok();
}

#[cfg(test)]
mod tests;
