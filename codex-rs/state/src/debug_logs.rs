use crate::LogQuery;
use crate::LogRow;
use crate::StateRuntime;
use crate::logs_db_path;
use anyhow::Context;
use chrono::DateTime;
use clap::Parser;
use clap::ValueEnum;
use dirs::home_dir;
use owo_colors::OwoColorize;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub struct DebugLogsCommand {
    /// Path to CODEX_HOME. Defaults to $CODEX_HOME or ~/.codex.
    #[arg(long = "codex-home", env = "CODEX_HOME", value_name = "DIR")]
    codex_home: Option<PathBuf>,

    /// Minimum level to include, e.g. warn includes WARN and ERROR.
    #[arg(
        long = "level",
        value_enum,
        ignore_case = true,
        conflicts_with = "level_exact"
    )]
    level: Option<DebugLogLevel>,

    /// Exact level to include. Use this to delete TRACE rows without deleting higher levels.
    #[arg(
        long = "level-exact",
        value_enum,
        ignore_case = true,
        conflicts_with = "level"
    )]
    level_exact: Option<DebugLogLevel>,

    /// Earliest timestamp, as unix seconds or RFC3339.
    #[arg(long = "from", value_name = "RFC3339|UNIX")]
    from: Option<String>,

    /// Latest timestamp, as unix seconds or RFC3339.
    #[arg(long = "to", value_name = "RFC3339|UNIX")]
    to: Option<String>,

    /// Match log target substring. Repeatable.
    #[arg(long = "target", value_name = "TEXT")]
    target: Vec<String>,

    /// Match module path substring. Repeatable.
    #[arg(long = "module", value_name = "TEXT")]
    module: Vec<String>,

    /// Match source file substring. Repeatable.
    #[arg(long = "file", value_name = "TEXT")]
    file: Vec<String>,

    /// Match thread id. Repeatable.
    #[arg(long = "thread-id", value_name = "ID")]
    thread_id: Vec<String>,

    /// Search persisted log body text.
    #[arg(long = "search", value_name = "TEXT")]
    search: Option<String>,

    /// Include only rows without a thread id.
    #[arg(long = "threadless", default_value_t = false)]
    threadless: bool,

    /// Maximum rows to print when inspecting logs.
    #[arg(long = "backfill", default_value_t = 50)]
    backfill: usize,

    /// Print a shorter one-line row format.
    #[arg(long = "compact", default_value_t = false)]
    compact: bool,

    /// Delete matching rows. Dry-run unless --yes is also set.
    #[arg(long = "delete", default_value_t = false)]
    delete: bool,

    /// Actually perform a delete or vacuum operation.
    #[arg(long = "yes", default_value_t = false)]
    yes: bool,

    /// Rebuild logs_2.sqlite so deleted rows return disk space.
    #[arg(long = "vacuum", default_value_t = false)]
    vacuum: bool,
}

impl DebugLogsCommand {
    pub async fn run(self) -> anyhow::Result<()> {
        let codex_home = self.codex_home.clone().unwrap_or_else(default_codex_home);
        let logs_db_path = logs_db_path(codex_home.as_path());
        let runtime = StateRuntime::init(codex_home, "debug-logs".to_string()).await?;
        let filter = build_filter(&self)?;

        if self.delete {
            delete_matching_logs(runtime.as_ref(), &filter, self.yes, logs_db_path.as_path())
                .await?;
        }

        if self.vacuum {
            vacuum_logs(runtime.as_ref(), self.yes, logs_db_path.as_path()).await?;
        }

        if !self.delete && !self.vacuum {
            print_backfill(runtime.as_ref(), &filter, self.backfill, self.compact).await?;
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum DebugLogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl DebugLogLevel {
    fn as_upper(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }

    fn threshold_levels_upper(self) -> Vec<String> {
        let levels = match self {
            Self::Trace => &["TRACE", "DEBUG", "INFO", "WARN", "ERROR"][..],
            Self::Debug => &["DEBUG", "INFO", "WARN", "ERROR"][..],
            Self::Info => &["INFO", "WARN", "ERROR"][..],
            Self::Warn => &["WARN", "ERROR"][..],
            Self::Error => &["ERROR"][..],
        };
        levels.iter().map(ToString::to_string).collect()
    }
}

#[derive(Debug)]
struct LogFilter {
    levels_upper: Vec<String>,
    from_ts: Option<i64>,
    to_ts: Option<i64>,
    target_like: Vec<String>,
    module_like: Vec<String>,
    file_like: Vec<String>,
    thread_ids: Vec<String>,
    search: Option<String>,
    include_threadless: bool,
}

fn default_codex_home() -> PathBuf {
    if let Some(home) = home_dir() {
        return home.join(".codex");
    }
    PathBuf::from(".codex")
}

fn build_filter(cmd: &DebugLogsCommand) -> anyhow::Result<LogFilter> {
    let from_ts = cmd
        .from
        .as_deref()
        .map(parse_timestamp)
        .transpose()
        .context("failed to parse --from")?;
    let to_ts = cmd
        .to
        .as_deref()
        .map(parse_timestamp)
        .transpose()
        .context("failed to parse --to")?;
    let levels_upper = match (cmd.level_exact, cmd.level) {
        (Some(level), None) => vec![level.as_upper().to_string()],
        (None, Some(level)) => level.threshold_levels_upper(),
        (None, None) => Vec::new(),
        (Some(_), Some(_)) => anyhow::bail!("--level and --level-exact cannot be used together"),
    };

    Ok(LogFilter {
        levels_upper,
        from_ts,
        to_ts,
        target_like: non_empty_values(&cmd.target),
        module_like: non_empty_values(&cmd.module),
        file_like: non_empty_values(&cmd.file),
        thread_ids: non_empty_values(&cmd.thread_id),
        search: cmd
            .search
            .as_ref()
            .filter(|value| !value.is_empty())
            .cloned(),
        include_threadless: cmd.threadless,
    })
}

fn non_empty_values(values: &[String]) -> Vec<String> {
    values
        .iter()
        .filter(|value| !value.is_empty())
        .cloned()
        .collect()
}

fn parse_timestamp(value: &str) -> anyhow::Result<i64> {
    if let Ok(secs) = value.parse::<i64>() {
        return Ok(secs);
    }

    let dt = DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("expected RFC3339 or unix seconds, got {value}"))?;
    Ok(dt.timestamp())
}

async fn delete_matching_logs(
    runtime: &StateRuntime,
    filter: &LogFilter,
    yes: bool,
    logs_db_path: &Path,
) -> anyhow::Result<()> {
    ensure_delete_filter(filter)?;
    let query = to_log_query(
        filter, /*limit*/ None, /*after_id*/ None, /*descending*/ false,
    );
    let matching_rows = runtime
        .matching_log_count(&query)
        .await
        .context("failed to count matching logs")?;
    if yes {
        let deleted_rows = runtime
            .delete_logs(&query)
            .await
            .context("failed to delete matching logs")?;
        println!(
            "Deleted {deleted_rows} matching log rows from {}.",
            logs_db_path.display()
        );
    } else {
        println!(
            "Would delete {matching_rows} matching log rows from {}. Re-run with --yes to delete.",
            logs_db_path.display()
        );
    }
    Ok(())
}

fn ensure_delete_filter(filter: &LogFilter) -> anyhow::Result<()> {
    let has_filter = !filter.levels_upper.is_empty()
        || filter.from_ts.is_some()
        || filter.to_ts.is_some()
        || !filter.target_like.is_empty()
        || !filter.module_like.is_empty()
        || !filter.file_like.is_empty()
        || !filter.thread_ids.is_empty()
        || filter.search.is_some()
        || filter.include_threadless;
    anyhow::ensure!(has_filter, "--delete requires at least one filter");
    Ok(())
}

async fn vacuum_logs(runtime: &StateRuntime, yes: bool, logs_db_path: &Path) -> anyhow::Result<()> {
    if !yes {
        println!(
            "Would run VACUUM on {}. Re-run with --yes to compact the file.",
            logs_db_path.display()
        );
        return Ok(());
    }

    println!("Running VACUUM on {}.", logs_db_path.display());
    runtime.vacuum_logs().await?;
    println!("Vacuumed {}.", logs_db_path.display());
    Ok(())
}

async fn print_backfill(
    runtime: &StateRuntime,
    filter: &LogFilter,
    backfill: usize,
    compact: bool,
) -> anyhow::Result<()> {
    if backfill == 0 {
        return Ok(());
    }

    let query = to_log_query(
        filter,
        Some(backfill),
        /*after_id*/ None,
        /*descending*/ true,
    );
    let mut rows = runtime
        .query_logs(&query)
        .await
        .context("failed to fetch matching logs")?;
    rows.reverse();
    for row in rows {
        println!("{}", format_row(&row, compact));
    }
    Ok(())
}

fn to_log_query(
    filter: &LogFilter,
    limit: Option<usize>,
    after_id: Option<i64>,
    descending: bool,
) -> LogQuery {
    LogQuery {
        levels_upper: filter.levels_upper.clone(),
        from_ts: filter.from_ts,
        to_ts: filter.to_ts,
        target_like: filter.target_like.clone(),
        module_like: filter.module_like.clone(),
        file_like: filter.file_like.clone(),
        thread_ids: filter.thread_ids.clone(),
        search: filter.search.clone(),
        include_threadless: filter.include_threadless,
        after_id,
        limit,
        descending,
    }
}

fn format_row(row: &LogRow, compact: bool) -> String {
    let timestamp = formatter::ts(row.ts, row.ts_nanos, compact)
        .dimmed()
        .to_string();
    let level = formatter::level(&row.level);
    let message = formatter::message(row.message.as_deref().unwrap_or(""), compact);
    if compact {
        return format!("{timestamp} {level} {message}");
    }

    let thread_id = row
        .thread_id
        .as_deref()
        .unwrap_or("-")
        .blue()
        .dimmed()
        .to_string();
    let target = row.target.as_str().dimmed().to_string();
    let location = match (row.file.as_deref(), row.line) {
        (Some(file), Some(line)) => format!(" {file}:{line}").dimmed().to_string(),
        (Some(file), None) => format!(" {file}").dimmed().to_string(),
        _ => String::new(),
    };
    format!(
        "{timestamp} #{id} {level} [{thread_id}] {target}{location} - {message}",
        id = row.id
    )
}

mod formatter {
    use chrono::DateTime;
    use chrono::SecondsFormat;
    use chrono::Utc;
    use owo_colors::OwoColorize;

    pub(super) fn ts(ts: i64, ts_nanos: i64, compact: bool) -> String {
        let nanos = u32::try_from(ts_nanos).unwrap_or(0);
        match DateTime::<Utc>::from_timestamp(ts, nanos) {
            Some(dt) if compact => dt.format("%H:%M:%S").to_string(),
            Some(dt) => dt.to_rfc3339_opts(SecondsFormat::Millis, true),
            None => format!("{ts}.{ts_nanos:09}Z"),
        }
    }

    pub(super) fn level(level: &str) -> String {
        let padded = format!("{level:<5}");
        if level.eq_ignore_ascii_case("error") {
            return padded.red().bold().to_string();
        }
        if level.eq_ignore_ascii_case("warn") {
            return padded.yellow().bold().to_string();
        }
        if level.eq_ignore_ascii_case("info") {
            return padded.green().bold().to_string();
        }
        if level.eq_ignore_ascii_case("debug") {
            return padded.blue().bold().to_string();
        }
        if level.eq_ignore_ascii_case("trace") {
            return padded.magenta().bold().to_string();
        }
        padded.bold().to_string()
    }

    pub(super) fn message(message: &str, compact: bool) -> String {
        let message = truncate(message);
        if compact {
            return message.replace('\n', "\\n").bold().to_string();
        }
        message.bold().to_string()
    }

    fn truncate(message: &str) -> String {
        const LIMIT: usize = 4096;
        if message.len() <= LIMIT {
            return message.to_string();
        }

        let mut end = LIMIT;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        format!(
            "{} ... [truncated {} bytes]",
            &message[..end],
            message.len() - end
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use pretty_assertions::assert_eq;

    #[test]
    fn level_threshold_includes_more_severe_levels() {
        assert_eq!(
            DebugLogLevel::Warn.threshold_levels_upper(),
            vec!["WARN".to_string(), "ERROR".to_string()]
        );
        assert_eq!(
            DebugLogLevel::Trace.threshold_levels_upper(),
            vec![
                "TRACE".to_string(),
                "DEBUG".to_string(),
                "INFO".to_string(),
                "WARN".to_string(),
                "ERROR".to_string(),
            ]
        );
    }

    #[test]
    fn exact_level_filters_only_that_level() {
        let args = DebugLogsCommand::try_parse_from(["logs", "--level-exact", "trace", "--delete"])
            .expect("parse exact trace");
        let filter = build_filter(&args).expect("build filter");

        assert_eq!(filter.levels_upper, vec!["TRACE".to_string()]);
    }

    #[test]
    fn delete_requires_a_filter() {
        let args = DebugLogsCommand::try_parse_from(["logs", "--delete"]).expect("parse delete");
        let filter = build_filter(&args).expect("build filter");

        assert!(ensure_delete_filter(&filter).is_err());
    }

    #[test]
    fn delete_accepts_target_filter_without_confirmation() {
        let args =
            DebugLogsCommand::try_parse_from(["logs", "--delete", "--target", "noisy_crate"])
                .expect("parse delete dry-run");
        let filter = build_filter(&args).expect("build filter");

        assert!(args.delete);
        assert!(!args.yes);
        assert_eq!(filter.target_like, vec!["noisy_crate".to_string()]);
        assert!(ensure_delete_filter(&filter).is_ok());
    }
}
