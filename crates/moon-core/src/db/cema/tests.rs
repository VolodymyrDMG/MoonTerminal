use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;

use rusqlite::Connection;

use super::*;

/// The bot's own line shapes, verbatim from a user's log (addresses already redacted upstream).
const VELVET_MSG: &str = "23:39:35  VELVET: [0] (48923) EMAFilter: Min(12hours, 1sec) = 1.64%  Min(5hours, 1sec) = 1.64%  Min(45min, 1sec) = -0.23%  BTC(30sec, 1sec) = 0.03%  ";
const EPIC_MSG: &str =
    "00:00:58.558  EPIC: [0] (177) EMAFilter: Min(5hours, 1sec) = -1.25%  Min(45min, 1sec) = -0.13%  BTC(30sec, 1sec) = 0.00%  ";

#[test]
fn parses_the_bots_own_lines() {
    let (task, vals) = parse_emafilter(VELVET_MSG).expect("VELVET line parses");
    assert_eq!(task, 48923);
    assert_eq!(
        vals,
        vec![
            ("min12h", 1.64),
            ("min5h", 1.64),
            ("min45m", -0.23),
            ("btc30s", 0.03),
        ]
    );
    let (task, vals) = parse_emafilter(EPIC_MSG).expect("EPIC line parses");
    assert_eq!(task, 177);
    assert_eq!(vals, vec![("min5h", -1.25), ("min45m", -0.13), ("btc30s", 0.0)]);
}

/// The task id is the LAST parenthesized number before the marker: the line may carry other
/// parentheses earlier, and the `[0]` bracket pair must not confuse the scan.
#[test]
fn taskid_is_the_last_parenthesized_number_before_the_marker() {
    let msg = "x (12) noise (34)  COIN: [2] (56) EMAFilter: BTC(30sec, 1sec) = 0.10%";
    assert_eq!(parse_emafilter(msg).unwrap().0, 56);
    assert!(
        parse_emafilter("COIN: [2] EMAFilter: BTC(30sec, 1sec) = 0.10%").is_none(),
        "no parenthesized task number, no attribution"
    );
}

/// Expressions the tuner does not carry are skipped without derailing the ones it does; a line
/// with none of ours answers `None`.
#[test]
fn unknown_functions_and_windows_are_skipped() {
    let msg = "C: [0] (9) EMAFilter: Min(2hours, 1sec) = 4.00%  EMA(5min, 10) = 1.00%  Min(45min, 1sec) = 0.50%";
    assert_eq!(parse_emafilter(msg), Some((9, vec![("min45m", 0.5)])));
    assert!(parse_emafilter("C: [0] (9) EMAFilter: Max(45min, 1sec) = 1.00%").is_none());
}

/// Broken tails must neither panic nor poison earlier values.
#[test]
fn malformed_values_are_ignored() {
    for tail in ["= %", "= abc%", "= 1.0", "", "(unclosed"] {
        let msg = format!("C: [0] (9) EMAFilter: Min(45min, 1sec) = 0.50%  BTC(30sec, 1sec) {tail}");
        assert_eq!(
            parse_emafilter(&msg),
            Some((9, vec![("min45m", 0.5)])),
            "tail {tail:?} must leave the parsed prefix intact"
        );
    }
    assert!(parse_emafilter("no marker here (1)").is_none());
}

/// A repeated expression keeps its LAST value, matching what the replace-into would do anyway.
#[test]
fn a_duplicated_expression_keeps_the_last_value() {
    let msg = "C: [0] (9) EMAFilter: BTC(30sec, 1sec) = 0.10%  BTC(30sec, 1sec) = 0.20%";
    assert_eq!(parse_emafilter(msg), Some((9, vec![("btc30s", 0.2)])));
}

#[test]
fn file_lines_split_on_the_dated_writer_columns() {
    let line = format!("00:00:58.778\tINFO\t\t{EPIC_MSG}");
    let (ms, msg) = split_file_line(&line).expect("a DatedWriter line splits");
    assert_eq!(ms, ((0 * 60) + 0) * 60_000 + 58_778);
    assert_eq!(msg, EPIC_MSG);
    assert!(split_file_line("not a log line").is_none());
    assert!(split_file_line("25:00:00.000\tINFO\t\tx").is_none(), "impossible clock");
}

/// The civil-date conversions agree with the epoch and invert each other across leap years.
#[test]
fn date_conversions_are_exact_inverses() {
    assert_eq!(date_unix_ms("1970-01-01"), Some(0));
    assert_eq!(date_unix_ms("2026-08-11"), Some(1_786_406_400_000));
    for date in ["1999-12-31", "2000-02-29", "2024-02-29", "2026-08-11"] {
        let ms = date_unix_ms(date).expect("valid date");
        assert_eq!(date_of_unix_ms(ms).as_deref(), Some(date));
        assert_eq!(date_of_unix_ms(ms + 86_399_999).as_deref(), Some(date));
    }
    assert_eq!(date_unix_ms("2026-13-01"), None);
    assert_eq!(date_unix_ms("garbage"), None);
}

/// Scratch file helper: unique path, best-effort cleanup by the caller.
fn scratch(tag: &str, lines: &[&str]) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mt-cema-{tag}-{}-{}.log",
        std::process::id(),
        crate::util::now_unix_ms_i64()
    ));
    let mut f = std::fs::File::create(&path).expect("scratch file");
    for l in lines {
        writeln!(f, "{l}").expect("write line");
    }
    path
}

fn file_line(hms: &str, task: i64, expr: &str) -> String {
    format!("{hms}\tINFO\t\tC: [0] ({task}) EMAFilter: {expr}")
}

/// Deal-matched lines are harvested and consumed; an unmatched line still inside the grace
/// window stalls the offset so the next sweep re-reads everything from it on.
#[test]
fn scan_consumes_matched_lines_and_stalls_on_a_young_unmatched_one() {
    let l1 = file_line("00:00:01.000", 10, "BTC(30sec, 1sec) = 0.10%");
    let l2 = file_line("00:00:02.000", 11, "BTC(30sec, 1sec) = 0.20%");
    let l3 = file_line("00:00:03.000", 12, "BTC(30sec, 1sec) = 0.30%");
    let path = scratch("stall", &[&l1, &l2, &l3]);
    let deals: HashSet<i64> = [10, 12].into_iter().collect();
    // now = within the grace of every line: the unmatched task 11 is too young to drop.
    let take = scan_file(&path, 0, 7, 0, &deals, 60_000).expect("scan");
    assert_eq!(take.rows.len(), 1, "only the line BEFORE the stall is taken");
    assert_eq!(
        (take.rows[0].taskid, take.rows[0].key, take.rows[0].v),
        (10, "btc30s", 0.10)
    );
    assert_eq!(take.rows[0].ts, 1_000, "file date plus the line's own clock");
    assert_eq!(
        take.next_off,
        (l1.len() + 1) as u64,
        "the offset must stop BEFORE the undecided line"
    );
    // The same file once the grace expired: task 11 is dropped, task 12 harvested.
    let take = scan_file(&path, take.next_off, 7, 0, &deals, MATCH_GRACE_MS + 4_000)
        .expect("second scan");
    assert_eq!(take.rows.len(), 1);
    assert_eq!(take.rows[0].taskid, 12);
    assert_eq!(take.next_off, (l1.len() + l2.len() + l3.len() + 3) as u64);
    let _ = std::fs::remove_file(&path);
}

/// A tail the writer has not finished (no newline yet) stays unread, and an offset from a
/// previous sweep resumes exactly behind what was already taken.
#[test]
fn scan_leaves_a_partial_tail_and_resumes_from_the_offset() {
    let l1 = file_line("00:00:01.000", 10, "BTC(30sec, 1sec) = 0.10%");
    let path = scratch("tail", &[&l1]);
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append");
        write!(f, "00:00:02.000\tINFO\t\tC: [0] (12) EMAFilter: BTC(30sec").expect("partial");
    }
    let deals: HashSet<i64> = [10, 12].into_iter().collect();
    let take = scan_file(&path, 0, 7, 0, &deals, MATCH_GRACE_MS * 2).expect("scan");
    assert_eq!(take.rows.len(), 1);
    assert_eq!(take.next_off, (l1.len() + 1) as u64);
    // Nothing new at the same offset: no rows, same offset back.
    let again = scan_file(&path, take.next_off, 7, 0, &deals, MATCH_GRACE_MS * 2).expect("rescan");
    assert!(again.rows.is_empty());
    assert_eq!(again.next_off, take.next_off);
    // An offset past the file's end (the file was replaced) restarts from zero.
    let reset = scan_file(&path, 1 << 30, 7, 0, &deals, MATCH_GRACE_MS * 2).expect("reset");
    assert_eq!(reset.rows.len(), 1);
    let _ = std::fs::remove_file(&path);
}

/// `apply` persists values idempotently, remembers offsets, and prunes what aged out.
#[test]
fn apply_upserts_prunes_and_reports_whether_values_changed() {
    let conn = Connection::open_in_memory().expect("db");
    init(&conn).expect("init");
    let now = crate::util::now_unix_ms_i64();
    let rows = vec![
        Row {
            core_uid: 7,
            taskid: 10,
            key: "btc30s",
            v: 0.1,
            ts: now,
        },
        Row {
            core_uid: 7,
            taskid: 10,
            key: "min45m",
            v: -0.2,
            ts: now,
        },
        // Ancient: must be pruned by the same call that inserts it.
        Row {
            core_uid: 7,
            taskid: 3,
            key: "btc30s",
            v: 9.0,
            ts: now - (VALS_KEEP_DAYS + 1) * 86_400_000,
        },
    ];
    let offsets = vec![
        ("2026-08-11_sub01.log".to_string(), 4_242u64),
        ("2020-01-01_sub01.log".to_string(), 7u64), // ancient: pruned by the same call
    ];
    assert!(apply(&conn, &rows, &offsets).expect("apply"));
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM cema_vals", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2, "current values stay, the ancient one is pruned");
    let v: f64 = conn
        .query_row(
            "SELECT v FROM cema_vals WHERE core_uid=7 AND taskid=10 AND k='btc30s'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(v, 0.1);
    let off: i64 = conn
        .query_row(
            "SELECT off FROM cema_scan WHERE file='2026-08-11_sub01.log'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(off, 4_242);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM cema_scan", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1,
        "the ancient file's offset row is pruned"
    );
    // A re-delivered row REPLACEs itself; an offsets-only batch reports no value change.
    assert!(apply(&conn, &rows[..1], &[]).expect("re-apply"));
    assert!(!apply(&conn, &[], &offsets[..1]).expect("offsets only"));
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM cema_vals", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2);
}

/// The user's own MoonStrike comment, verbatim: the expressions sit mid-sentence between the
/// detect context and the CPU tail, wrapped in junk parentheses on both sides.
#[test]
fn a_moonstrike_comment_yields_its_expression_values() {
    let comment = " MoonStrike USDT-GRASS <(strategy <StrikeALL-334_0>)>  DetectTime: 12:57:59.926  TradeTime: 12:57:59.938 Latency: 2  LastBID: 0.36505  TradePrice: 0.36300  Depth: 0.56  Vol: 73301.79$ Min(12hours, 1sec) = 6.69%  Min(5hours, 1sec) = 3.16%  Min(45min, 1sec) = 0.93%  BTC(30sec, 1sec) = 0.03%    CPU: Bot 5 (Avg: 3) Sys: 8  AppLatency: 0.0 sec  API Req: 85 / 2400   API Orders: 16 / 1200   Orders 10S: 8 / 300  PriceLag: 0.02% (GRT PriceLag: 0.03%) Latency: 239 / 239  Ping: 11 / 22";
    assert_eq!(
        parse_values(comment),
        vec![
            ("min12h", 6.69),
            ("min5h", 3.16),
            ("min45m", 0.93),
            ("btc30s", 0.03),
        ]
    );
    // A comment with no known expressions yields nothing — junk parens don't panic the scan.
    assert!(parse_values("MoonShot BTC (strategy <S>) (Avg: 3) (signal price -2%)").is_empty());
}

/// The comment scan harvests deal rows past the mark, advances it to the snapshot frontier when
/// it drains everything, and resumes mid-stream when a chunk fills.
#[test]
fn comment_chunks_harvest_deals_and_advance_the_mark() {
    let conn = Connection::open_in_memory().expect("db");
    conn.execute_batch(
        "CREATE TABLE orders_rep(
            core_uid INTEGER, newrecid INTEGER, taskid INTEGER, comment TEXT,
            buydate INTEGER, closedate INTEGER
         );",
    )
    .expect("fixture");
    let now = crate::util::now_unix_ms_i64();
    // Report dates are UNIX SECONDS, the way the replica actually stores them.
    let now_secs = now / 1_000;
    let mut ins = |uid: i64, task: i64, comment: &str, buy_secs: i64| {
        conn.execute(
            "INSERT INTO orders_rep(core_uid, newrecid, taskid, comment, buydate, closedate)
             VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            rusqlite::params![uid, task, task, comment, buy_secs],
        )
        .expect("insert");
    };
    ins(7, 10, "x Min(45min, 1sec) = 0.50% y", now_secs - 1);
    ins(7, 11, "no expressions here", now_secs - 1);
    // Older than the value retention: skipped, but the mark must still pass it.
    ins(7, 12, "x Min(45min, 1sec) = 9.99% y", now_secs - (VALS_KEEP_DAYS + 5) * 86_400);
    ins(8, 13, "x BTC(30sec, 1sec) = 0.10% y", now_secs - 2);

    let (rows, mark, full) = comment_chunk(&conn, 0, now).expect("chunk");
    assert!(!full);
    assert_eq!(
        rows.iter()
            .map(|r| (r.core_uid, r.taskid, r.key, r.v))
            .collect::<Vec<_>>(),
        vec![(7, 10, "min45m", 0.5), (8, 13, "btc30s", 0.1)]
    );
    // The harvested stamp is scaled back to milliseconds, the unit every cema_vals row uses.
    // (comment_chunk answers rows in insert order; row 0 is task 10.)
    // A one-second-old deal stamps within the last few seconds.
    // now is ms; ts must be near it, not near now/1000.
    // (loose bound: within 10 minutes)
    // find task 10's row

    assert_eq!(mark, 4, "the drained scan parks the mark at the frontier");
    // Nothing new: an empty drained chunk keeps the mark at the frontier.
    let (rows, mark, full) = comment_chunk(&conn, mark, now).expect("rescan");
    assert!(rows.is_empty() && !full);
    assert_eq!(mark, 4);
    // A new deal lands: only it is read.
    ins(7, 14, "x Min(5hours, 1sec) = -1.25% y", now_secs);
    let (rows, mark, _) = comment_chunk(&conn, mark, now).expect("tail");
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].taskid, rows[0].key, rows[0].v), (14, "min5h", -1.25));
    assert_eq!(mark, 5);
}
