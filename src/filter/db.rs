//! Database CLI filters: `psql`, `mysql`, `sqlite3`, `redis-cli`, `mongosh`, `prisma`.
//!
//! Tables are parsed from their rule lines (`----+----`, `+---+`, `┼`, `├`) and
//! re-emitted as `a|b|c` rows with cell padding trimmed; row counts come from the
//! tool's own trailer (`(N rows)`, `N rows in set`) so a `[+N more rows]` cap never
//! hides the total. Banners, prompts, borders and box decoration are the only
//! silent drops; everything else is kept, squeezed, or announced with [`more`].

use super::common::*;
use regex::Regex;
use std::sync::OnceLock;

// ─── shared ───────────────────────────────────────────────────────────────────

/// CRLF → LF so the line parsers see one record per line.
fn normalize(s: &str) -> String {
    if s.contains('\r') { s.replace("\r\n", "\n").replace('\r', "\n") } else { s.to_string() }
}

/// Glyphs that make up a horizontal rule (ASCII and box-drawing).
const RULE_CHARS: &str = "-+─┼┬┴├┤┌┐└┘═╪╬╤╧╠╣╔╗╚╝";
/// Glyphs where a rule crosses a column boundary.
const CUT_CHARS: &str = "+┼┬┴├┤┌┐└┘╪╬╤╧╠╣╔╗╚╝";
/// Vertical cell separators.
const BAR_CHARS: &str = "|│║";

fn is_bar(c: char) -> bool { BAR_CHARS.contains(c) }

/// `----+-----`, `+---+---+`, `─┼─`, `┌──┬──┐` … (needs at least one dash-like glyph).
fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 2
        && t.chars().all(|c| RULE_CHARS.contains(c) || c == ' ' || is_bar(c))
        && t.chars().any(|c| matches!(c, '-' | '─' | '═'))
}

/// Markdown table separator: `|----|:---:|`.
fn is_md_rule(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.len() >= 3 && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) && t.contains('-')
}

/// Char indices of the column crossings in a rule line (`+`/`┼`; markdown `|`).
fn rule_cuts(line: &str) -> Vec<usize> {
    let md = is_md_rule(line);
    line.char_indices()
        .enumerate()
        .filter(|(_, (_, c))| CUT_CHARS.contains(*c) || (md && *c == '|'))
        .map(|(i, _)| i)
        .collect()
}

/// Split `line` into trimmed cells at `cuts` (char positions of the rule's crossings).
/// Falls back to splitting on the bar glyphs when the row doesn't line up with the rule
/// (multibyte display widths, wrapped values). `bordered` rows carry leading/trailing bars.
fn slice_cells(line: &str, cuts: &[usize], bordered: bool) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let aligned = !cuts.is_empty() && cuts.iter().all(|&i| chars.get(i).is_some_and(|c| is_bar(*c)));
    let mut cells: Vec<String> = if aligned {
        let mut out = Vec::new();
        let mut start = 0;
        for &i in cuts {
            out.push(chars[start..i].iter().collect::<String>().trim().to_string());
            start = i + 1;
        }
        out.push(chars.get(start..).unwrap_or(&[]).iter().collect::<String>().trim().to_string());
        out
    } else {
        line.split(|c| is_bar(c)).map(|c| c.trim().to_string()).collect()
    };
    if bordered {
        if cells.first().is_some_and(|c| c.is_empty()) { cells.remove(0); }
        if cells.last().is_some_and(|c| c.is_empty()) { cells.pop(); }
    }
    cells
}

/// Rows already split into cells → `a|b|c` lines, capped at `list_max_lines`.
fn emit_table(header: Option<Vec<String>>, rows: Vec<Vec<String>>, out: &mut Vec<String>) {
    let max = limits().list_max_lines;
    if let Some(h) = header {
        out.push(h.join("|"));
    }
    let n = rows.len();
    for r in rows.into_iter().take(max) {
        out.push(r.join("|"));
    }
    if n > max {
        out.push(more(n - max, "rows"));
    }
}

/// Drop columns whose header and every cell are empty (describe-table padding columns).
fn drop_empty_columns(header: &mut Vec<String>, rows: &mut [Vec<String>]) {
    let width = header.len();
    let keep: Vec<bool> = (0..width)
        .map(|i| rows.iter().any(|r| r.get(i).is_some_and(|c| !c.is_empty())))
        .collect();
    if keep.iter().all(|k| *k) { return }
    let filter = |v: &mut Vec<String>| {
        let mut i = 0;
        v.retain(|_| { let k = keep.get(i).copied().unwrap_or(true); i += 1; k });
    };
    filter(header);
    for r in rows.iter_mut() { filter(r) }
}

/// `key   | value` / `key: value` → `key: value` with padding squeezed.
fn kv_line(line: &str, sep: char) -> Option<String> {
    let (k, v) = line.split_once(sep)?;
    let k = k.trim();
    if k.is_empty() || k.contains(' ') { return None }
    Some(format!("{}: {}", k, v.trim()))
}

/// Everything after the header/trailer was fed through here: final safety cap.
fn finish(lines: Vec<String>) -> String {
    let l = limits();
    cap_vec(lines, l.passthrough_max_lines, "lines").join("\n")
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("static regex"))
}

// ─── psql ─────────────────────────────────────────────────────────────────────

/// `-[ RECORD 3 ]----` (expanded display) → Some(3).
fn psql_record_header(line: &str) -> Option<&str> {
    static RE: OnceLock<Regex> = OnceLock::new();
    re(&RE, r"^[-─]\[ RECORD (\d+) \][-─]*$").captures(line.trim()).map(|c| c.get(1).unwrap().as_str())
}

fn psql_banner(l: &str) -> bool {
    let t = l.trim();
    (t.starts_with("psql (") && t.ends_with(')'))
        || t == "Type \"help\" for help."
        || t.starts_with("SSL connection (")
        || t.starts_with("Is the server running")
        || t.starts_with("Password for user ")
        || t.starts_with("Password: ")
        || t == "Password:"
}

fn psql_footer(l: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    re(&RE, r"^\((\d+) rows?\)$").is_match(l.trim())
}

/// Strip an interactive prompt (`mydb=# `, `mydb-> `, `mydb=> `) from an echoed line.
fn psql_strip_prompt(l: &str) -> Option<&str> {
    static RE: OnceLock<Regex> = OnceLock::new();
    re(&RE, r"^[\w.-]+[=\-]?[#>] ?").find(l).map(|m| l[m.end()..].trim())
}

/// Looks like a row of the aligned table (leading pad or a bar somewhere).
fn psql_is_row(l: &str) -> bool {
    !l.trim().is_empty() && !psql_footer(l) && (l.starts_with(' ') || l.chars().next().is_some_and(is_bar) || l.chars().any(is_bar))
}

pub(crate) fn filter_psql(output: &str) -> String {
    let text = normalize(output);
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        if t.is_empty() { i += 1; continue }

        // aligned table: header line followed by a rule
        if !is_rule(line) && psql_record_header(line).is_none()
            && lines.get(i + 1).is_some_and(|n| is_rule(n) && !is_md_rule(n))
        {
            let rule = lines[i + 1];
            let bordered = rule.trim_start().starts_with(|c: char| CUT_CHARS.contains(c)) && line.trim_start().starts_with(is_bar);
            let cuts = rule_cuts(rule);
            // border=2 top rule sits above the header: already emitted? it was skipped as a rule below
            let mut header = slice_cells(line, &cuts, bordered);
            let mut rows: Vec<Vec<String>> = Vec::new();
            let mut j = i + 2;
            let mut footer: Option<&str> = None;
            while j < lines.len() {
                let l = lines[j];
                if is_rule(l) { j += 1; if bordered { break } else { continue } }
                if psql_footer(l) { footer = Some(l.trim()); j += 1; break }
                if !psql_is_row(l) { break }
                rows.push(slice_cells(l, &cuts, bordered));
                j += 1;
            }
            let describe = header.iter().any(|h| h == "Column") && header.iter().any(|h| h == "Type");
            if describe { drop_empty_columns(&mut header, &mut rows) }
            emit_table(Some(header), rows, &mut out);
            if let Some(f) = footer { out.push(f.to_string()) }
            i = j;
            continue;
        }

        // expanded display: -[ RECORD n ]--- then `key | value` lines
        if let Some(n) = psql_record_header(line) {
            out.push(format!("record {}:", n));
            let mut j = i + 1;
            while j < lines.len() {
                let l = lines[j];
                if l.trim().is_empty() || psql_record_header(l).is_some() || psql_footer(l) { break }
                let sep = if l.contains('│') { '│' } else { '|' };
                out.push(format!(" {}", kv_line(l, sep).unwrap_or_else(|| squeeze_ws(l))));
                j += 1;
            }
            i = j;
            continue;
        }

        if is_rule(line) || psql_banner(line) { i += 1; continue }
        // caret line under `LINE 1:` (position is decoration)
        if t.chars().all(|c| c == '^') { i += 1; continue }
        // prompt-echoed input
        if let Some(rest) = psql_strip_prompt(line) {
            if line != rest && !rest.is_empty() { out.push(format!("> {}", squeeze_ws(rest))) }
            i += 1;
            continue;
        }
        // tuples-only (-t) rows still carry ` | ` separators
        if line.starts_with(' ') && line.contains(" | ") {
            out.push(line.split('|').map(str::trim).collect::<Vec<_>>().join("|"));
        } else {
            out.push(squeeze_ws(line));
        }
        i += 1;
    }
    finish(out)
}

// ─── mysql ────────────────────────────────────────────────────────────────────

fn mysql_banner(l: &str) -> bool {
    let t = l.trim();
    t.starts_with("mysql: [Warning] Using a password")
        || t.starts_with("Warning: Using a password")
        || t.starts_with("Welcome to the MySQL monitor")
        || t.starts_with("Welcome to the MariaDB monitor")
        || t.starts_with("Commands end with ;")
        || t.starts_with("Your MySQL connection id is")
        || t.starts_with("Your MariaDB connection id is")
        || t.starts_with("Server version:")
        || t.starts_with("Copyright (c)")
        || t.starts_with("Oracle is a registered trademark")
        || t.starts_with("affiliates. Other names may be trademarks")
        || t.starts_with("Type 'help;' or '\\h' for help.")
        || t.starts_with("Reading table information for completion")
        || t.starts_with("You can turn off this feature")
        || t.starts_with("Enter password:")
        || t == "Bye"
        || t.starts_with("mysql>") && t.trim_start_matches("mysql>").trim().is_empty()
}

/// `*************************** 2. row ***************************` → Some("2")
fn mysql_row_header(l: &str) -> Option<&str> {
    static RE: OnceLock<Regex> = OnceLock::new();
    re(&RE, r"^\*+ (\d+)\. row \*+$").captures(l.trim()).map(|c| c.get(1).unwrap().as_str())
}

/// Shared by mysql and sqlite3: bordered ASCII/unicode tables, tab rows, vertical rows.
/// `vertical` turns `      key: value` lines after a `N. row` header into ` key: value`.
fn filter_table_tool(output: &str, banner: fn(&str) -> bool, keep_blank_free_list: bool) -> String {
    let text = normalize(output);
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::new();
    // in-progress bordered table
    let mut cuts: Vec<usize> = Vec::new();
    let mut rules = 0usize;
    let mut header: Option<Vec<String>> = None;
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut vertical = false;
    let mut i = 0;
    let flush = |rules: &mut usize, header: &mut Option<Vec<String>>, rows: &mut Vec<Vec<String>>, out: &mut Vec<String>| {
        if *rules > 0 || header.is_some() || !rows.is_empty() {
            emit_table(header.take(), std::mem::take(rows), out);
        }
        *rules = 0;
    };
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        // bordered table rules
        if is_rule(line) && !is_md_rule(line) {
            let starts_bordered = t.starts_with(|c: char| CUT_CHARS.contains(c));
            if starts_bordered {
                rules += 1;
                if rules == 1 { cuts = rule_cuts(line) }
                if rules >= 3 { flush(&mut rules, &mut header, &mut rows, &mut out) }
            }
            i += 1;
            continue;
        }
        if rules > 0 && t.starts_with(is_bar) {
            let cells = slice_cells(line, &cuts, true);
            if rules == 1 && header.is_none() { header = Some(cells) } else { rows.push(cells) }
            i += 1;
            continue;
        }
        // markdown table (`sqlite3 -markdown`)
        if t.starts_with('|') && lines.get(i + 1).is_some_and(|n| is_md_rule(n)) {
            let md_cuts = rule_cuts(lines[i + 1]);
            header = Some(slice_cells(line, &md_cuts, true));
            let mut j = i + 2;
            while j < lines.len() && lines[j].trim().starts_with('|') {
                rows.push(slice_cells(lines[j], &md_cuts, true));
                j += 1;
            }
            flush(&mut rules, &mut header, &mut rows, &mut out);
            i = j;
            continue;
        }
        // column mode (`sqlite3 -column`): header, dash rule with spaces, rows
        if !t.is_empty() && lines.get(i + 1).is_some_and(|n| {
            let nt = n.trim();
            nt.len() >= 2 && nt.chars().all(|c| c == '-' || c == ' ') && nt.contains(' ') && !nt.trim_start_matches('-').is_empty()
        }) {
            let rule = lines[i + 1];
            let starts: Vec<usize> = rule.char_indices().enumerate()
                .filter(|(k, (_, c))| *c == '-' && (*k == 0 || rule.chars().nth(k - 1) != Some('-')))
                .map(|(k, _)| k).collect();
            let slice = |l: &str| -> Vec<String> {
                let chars: Vec<char> = l.chars().collect();
                starts.iter().enumerate().map(|(k, &s)| {
                    let e = starts.get(k + 1).copied().unwrap_or(chars.len()).min(chars.len());
                    chars.get(s.min(chars.len())..e).unwrap_or(&[]).iter().collect::<String>().trim().to_string()
                }).collect()
            };
            header = Some(slice(line));
            let mut j = i + 2;
            while j < lines.len() && !lines[j].trim().is_empty() && !lines.get(j + 1).is_some_and(|n| n.trim().chars().all(|c| c == '-' || c == ' ') && n.contains('-')) {
                rows.push(slice(lines[j]));
                j += 1;
            }
            flush(&mut rules, &mut header, &mut rows, &mut out);
            i = j;
            continue;
        }
        if rules > 0 { flush(&mut rules, &mut header, &mut rows, &mut out) }

        if t.is_empty() { vertical = false; i += 1; continue }
        if banner(line) { i += 1; continue }
        if let Some(n) = mysql_row_header(line) {
            out.push(format!("row {}:", n));
            vertical = true;
            i += 1;
            continue;
        }
        if vertical {
            if let Some(kv) = kv_line(line, ':') { out.push(format!(" {}", kv)); i += 1; continue }
            vertical = false;
        }
        if t.starts_with("mysql>") || t.starts_with("MariaDB [") || t.starts_with("sqlite>") {
            let rest = t.splitn(2, '>').nth(1).unwrap_or("").trim();
            if !rest.is_empty() { out.push(format!("> {}", squeeze_ws(rest))) }
            i += 1;
            continue;
        }
        if line.contains('\t') {
            out.push(line.split('\t').map(str::trim).collect::<Vec<_>>().join("|"));
        } else if keep_blank_free_list {
            out.push(line.trim_end().to_string());
        } else {
            out.push(squeeze_ws(line));
        }
        i += 1;
    }
    flush(&mut rules, &mut header, &mut rows, &mut out);
    finish(out)
}

pub(crate) fn filter_mysql(output: &str) -> String {
    filter_table_tool(output, mysql_banner, false)
}

// ─── sqlite3 ──────────────────────────────────────────────────────────────────

fn sqlite_banner(l: &str) -> bool {
    let t = l.trim();
    t.starts_with("SQLite version ") || t.starts_with("Enter \".help\" for usage hints.")
        || t.starts_with("Connected to a transient in-memory database")
        || t.starts_with("Use \".open FILENAME\"")
}

pub(crate) fn filter_sqlite3(output: &str) -> String {
    // list-mode rows (`a|b`) are already compact: keep them verbatim, trailing pad trimmed
    filter_table_tool(output, sqlite_banner, true)
}

// ─── redis-cli ────────────────────────────────────────────────────────────────

pub(crate) fn filter_redis(output: &str) -> String {
    // redis-cli output is already terse — strip prompt lines
    output.lines().filter(|l| !l.starts_with("127.0.0.1:") && !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
}

// ─── mongosh ──────────────────────────────────────────────────────────────────

/// Relaxed EJSON (`{ _id: ObjectId('a'), name: 'x' }`) → strict JSON so the shared
/// renderer can be used. Heuristic, and deliberately conservative: on any doubt the
/// caller falls back to the raw text.
fn ejson_to_json(s: &str) -> String {
    static WRAPPERS: [&str; 6] = ["ObjectId", "ISODate", "UUID", "BinData", "NumberDecimal", "Timestamp"];
    let mut out = s.to_string();
    for w in WRAPPERS {
        // ObjectId('abc') / ISODate("2026-01-01") → "abc"
        let re = regex::Regex::new(&format!(r#"{}\(\s*['"]?([^'")]*)['"]?\s*\)"#, w)).expect("static regex");
        out = re.replace_all(&out, "\"$1\"").into_owned();
    }
    for w in ["NumberLong", "NumberInt", "NumberDouble"] {
        let re = regex::Regex::new(&format!(r#"{}\(\s*['"]?([-0-9.]*)['"]?\s*\)"#, w)).expect("static regex");
        out = re.replace_all(&out, "$1").into_owned();
    }
    // bare keys → quoted keys
    let keys = regex::Regex::new(r"([\{,]\s*)([A-Za-z_$][A-Za-z0-9_$.]*)\s*:").expect("static regex");
    out = keys.replace_all(&out, "$1\"$2\":").into_owned();
    // single-quoted scalars → double-quoted
    let sq = regex::Regex::new(r"'([^'\n]*)'").expect("static regex");
    out = sq.replace_all(&out, "\"$1\"").into_owned();
    out
}

const MONGOSH_BANNER: [&str; 8] = [
    "Current Mongosh",
    "Connecting to:",
    "Using MongoDB:",
    "Using Mongosh:",
    "For mongosh info see",
    "mongosh: ",
    "Enterprise ",
    "------",
];

pub(crate) fn filter_mongosh(output: &str) -> String {
    let l = limits();
    let mut body: Vec<String> = Vec::new();
    let mut warnings = 0usize;
    let mut in_warnings = false;
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.contains("startup warnings") {
            in_warnings = true;
            warnings += 1;
            continue;
        }
        if in_warnings {
            // the warning block ends at the next prompt
            if t.contains('>') && !t.starts_with('-') {
                in_warnings = false;
            } else {
                warnings += 1;
                continue;
            }
        }
        if MONGOSH_BANNER.iter().any(|b| t.contains(b)) {
            continue;
        }
        // `test> db.users.find()` — the echoed command, and `switched to db x`
        if let Some(cmd) = t.split_once("> ") {
            if !cmd.0.contains(' ') {
                body.push(cmd.1.to_string());
                continue;
            }
        }
        if t.starts_with("switched to db") {
            continue;
        }
        body.push(t.to_string());
    }
    let text = body.join("\n");
    // documents render through the shared JSON path when they can be normalised
    let mut out = Vec::new();
    let docs: Vec<&str> = text.lines().collect();
    let mut rendered: Vec<String> = Vec::new();
    for chunk in &docs {
        let c = chunk.trim();
        if c.starts_with('{') || c.starts_with('[') {
            match parse_json(&ejson_to_json(c)) {
                Some(v) => {
                    rendered.extend(v.iter().map(compact_json));
                    continue;
                }
                None => rendered.push(c.to_string()),
            }
        } else {
            rendered.push(c.to_string());
        }
    }
    out.extend(cap_vec(rendered, l.list_max_lines, "lines"));
    if warnings > 0 {
        out.push(format!("({} startup warning lines)", warnings));
    }
    out.join("\n")
}

// ─── prisma ───────────────────────────────────────────────────────────────────

const PRISMA_BANNER: [&str; 6] = [
    "Environment variables loaded from",
    "Prisma schema loaded from",
    "Prisma schema loaded",
    "Update available",
    "Run `prisma generate`",
    "Visit https://pris.ly",
];

pub(crate) fn filter_prisma_db(output: &str) -> String {
    let l = limits();
    let mut keep: Vec<String> = Vec::new();
    let mut datasource: Option<String> = None;
    let mut tree = 0usize;
    for line in output.lines() {
        let t = strip_ansi(line.trim_end()).trim().to_string();
        if t.is_empty() {
            continue;
        }
        if PRISMA_BANNER.iter().any(|b| t.contains(b)) {
            continue;
        }
        // box-drawn "update available" frame and the migration file tree
        if t.starts_with('┌') || t.starts_with('│') || t.starts_with('└') || t.starts_with('┐') {
            continue;
        }
        if t.starts_with("└─") || t.starts_with("migrations/") {
            tree += 1;
            continue;
        }
        if let Some(rest) = t.strip_prefix("Datasource \"") {
            // `Datasource "db": PostgreSQL database "prism", schema "public" at "host:5432"`
            let db = rest.split('"').nth(2).unwrap_or("");
            let at = rest.rsplit("at \"").next().and_then(|r| r.split('"').next()).unwrap_or("");
            datasource = Some(format!("db: {}@{}", db, at));
            continue;
        }
        keep.push(truncate(&squeeze_ws(&t), 200));
    }
    let mut out: Vec<String> = datasource.into_iter().collect();
    out.extend(cap_vec(keep, l.max_diagnostics, "lines"));
    if tree > 0 {
        out.push(format!("({} migration file lines)", tree));
    }
    if out.is_empty() {
        out.push("prisma: ok".to_string());
    }
    out.join("\n")
}

pub(crate) fn filter_prisma(args: &[&str], output: &str) -> String {
    match find_subcommand(args) {
        Some("migrate") | Some("db") | Some("generate") | Some("format") | Some("validate")
        | Some("version") | Some("-v") => filter_prisma_db(output),
        _ => generic(output),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn psql_table_becomes_pipe_rows_and_keeps_row_count() {
        let out = filter_psql(
            "psql (16.1 (Ubuntu 16.1-1))\nType \"help\" for help.\n\n   name    | owner | encoding \n-----------+-------+----------\n prism     | anshu | UTF8\n movexa    | anshu | UTF8\n(2 rows)\n",
        );
        assert!(out.contains("prism"), "{}", out);
        assert!(out.contains("movexa"), "{}", out);
        assert!(out.contains("UTF8"), "{}", out);
        assert!(out.contains("2 rows"), "{}", out);
        assert!(!out.contains("Type \"help\""), "{}", out);
        assert!(!out.contains("-----------"), "{}", out);
    }

    #[test]
    fn psql_keeps_errors_and_command_tags() {
        let err = filter_psql("psql: error: connection to server at \"127.0.0.1\", port 5432 failed: Connection refused\n\tIs the server running on that host and accepting TCP/IP connections?\n");
        assert!(err.contains("Connection refused"), "{}", err);
        let tag = filter_psql("INSERT 0 3\nUPDATE 5\n");
        assert!(tag.contains("INSERT 0 3") && tag.contains("UPDATE 5"), "{}", tag);
        let sql_err = filter_psql("ERROR:  relation \"users\" does not exist\nLINE 1: select * from users;\n                      ^\n");
        assert!(sql_err.contains("relation \"users\" does not exist"), "{}", sql_err);
    }

    #[test]
    fn psql_row_cap_is_announced() {
        let mut raw = String::from(" id | name \n----+------\n");
        for i in 0..400 {
            raw.push_str(&format!(" {} | n{} \n", i, i));
        }
        raw.push_str("(400 rows)\n");
        let out = filter_psql(&raw);
        assert!(has_truncation(&out), "{}", out);
        assert!(out.contains("400 rows"), "{}", out);
    }

    #[test]
    fn mysql_and_sqlite_drop_borders() {
        let my = filter_mysql("+----+-------+\n| id | name  |\n+----+-------+\n|  1 | prism |\n+----+-------+\n1 row in set (0.00 sec)\n");
        assert!(my.contains("prism"), "{}", my);
        assert!(my.contains("1 row in set"), "{}", my);
        assert!(!my.contains("+----"), "{}", my);
        let lite = filter_sqlite3("1|prism|ok\n2|movexa|ok\n");
        assert!(lite.contains("1|prism|ok"), "{}", lite);
    }

    #[test]
    fn redis_strips_prompts_and_type_markers() {
        let out = filter_redis("127.0.0.1:6379> get k\n\"value\"\n127.0.0.1:6379> llen l\n(integer) 5\n");
        assert!(out.contains("value"), "{}", out);
        assert!(out.contains('5'), "{}", out);
        assert!(!out.contains("127.0.0.1:6379>"), "{}", out);
        assert!(filter_redis("(nil)\n").contains("nil"));
        assert!(filter_redis("OK\n").contains("OK"));
    }

    #[test]
    fn mongosh_drops_banner_and_renders_documents() {
        let out = filter_mongosh(
            "Current Mongosh Log ID:\t65f0\nConnecting to:\t\tmongodb://127.0.0.1:27017\nUsing MongoDB:\t\t7.0.5\nUsing Mongosh:\t\t2.1.1\n\nFor mongosh info see: https://docs.mongodb.com/mongodb-shell/\n\ntest> db.users.find()\n[ { _id: ObjectId('65f0aa'), name: 'anshu', active: true } ]\n",
        );
        assert!(out.contains("anshu"), "{}", out);
        assert!(!out.contains("Current Mongosh Log ID"), "{}", out);
        assert!(!out.contains("docs.mongodb.com"), "{}", out);
    }

    #[test]
    fn prisma_drops_banner_keeps_migrations_and_errors() {
        let mig = filter_prisma(
            &["migrate", "dev"],
            "Environment variables loaded from .env\nPrisma schema loaded from prisma/schema.prisma\nDatasource \"db\": PostgreSQL database \"prism\", schema \"public\" at \"localhost:5432\"\n\nApplying migration `20260909_init`\n\nThe following migration(s) have been applied:\n\nmigrations/\n  └─ 20260909_init/\n    └─ migration.sql\n\nYour database is now in sync with your schema.\n",
        );
        assert!(mig.contains("20260909_init"), "{}", mig);
        assert!(!mig.contains("Environment variables loaded"), "{}", mig);
        let err = filter_prisma(&["migrate", "deploy"], "Error: P1001: Can't reach database server at `localhost:5432`\n");
        assert!(err.contains("P1001"), "{}", err);
        assert!(err.contains("localhost:5432"), "{}", err);
    }

    #[test]
    fn empty_and_odd_inputs_never_panic() {
        for raw in ["", "\n\n", "+---+\n", "|\n", "(0 rows)\n", "── ✓ 🎉\n", "a\r\nb\r\n", "\t|\t|\t\n"] {
            let _ = filter_psql(raw);
            let _ = filter_mysql(raw);
            let _ = filter_sqlite3(raw);
            let _ = filter_redis(raw);
            let _ = filter_mongosh(raw);
            let _ = filter_prisma(&["migrate"], raw);
            let _ = filter_prisma(&["studio"], raw);
        }
    }
}
