#![forbid(unsafe_code)]

//! Standalone MCP server binary for Arazzo workflows.

use std::fs;
use std::io::{self, BufRead, BufReader, BufWriter};
use std::path::Path;

use clap::Parser;

#[derive(Parser)]
#[command(name = "arazzo-mcp")]
#[command(about = "MCP server exposing Arazzo workflows as AI-agent tools")]
struct Args {
    /// Arazzo spec files to load
    specs: Vec<String>,

    /// Directory containing .arazzo.yaml files to load
    #[arg(long = "dir")]
    dir: Option<String>,

    /// Restrict validate_spec file access to these directories (repeatable)
    #[arg(long = "allowed-dir")]
    allowed_dir: Vec<String>,
}

fn main() {
    if let Some(report) = load_env_file(".env") {
        eprint_env_load_report(".env", &report);
    }

    let args = Args::parse();
    let spec_paths = match resolve_spec_paths(&args) {
        Ok(paths) => paths,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    };

    let reader = BufReader::new(io::stdin());
    let mut writer = BufWriter::new(io::stdout().lock());

    let allowed = if args.allowed_dir.is_empty() {
        None
    } else {
        Some(args.allowed_dir)
    };

    if let Err(err) = arazzo_mcp::run_mcp_stdio(reader, &mut writer, &spec_paths, allowed) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn resolve_spec_paths(args: &Args) -> Result<Vec<String>, String> {
    let mut paths = args.specs.clone();
    if let Some(dir) = &args.dir {
        let discovered = arazzo_mcp::state::discover_specs(dir)?;
        paths.extend(discovered);
    }
    Ok(paths)
}

// `.env` loading. This and the copy in `arazzo-cli/src/main.rs` are kept in lockstep: the
// same file must load and report identically from either binary (audit F20
// owns deduplicating them).

/// What `load_env_file` did with one `.env` file.
#[derive(Debug, Default, PartialEq, Eq)]
struct EnvLoadReport {
    /// Names the file set because the environment did not have them.
    set: usize,
    /// Names the file defined that were kept from the environment (ac-51491).
    kept: usize,
    /// 1-based line numbers of lines that could not be used, with the reason.
    ignored: Vec<(usize, IgnoredEnvLine)>,
}

/// Why a `.env` line was ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IgnoredEnvLine {
    Unreadable,
    NoSeparator,
    EmptyName,
    NulByte,
}

impl IgnoredEnvLine {
    fn reason(self) -> &'static str {
        match self {
            Self::Unreadable => "line could not be read",
            Self::NoSeparator => "no `=` separator",
            Self::EmptyName => "empty name",
            Self::NulByte => "NUL byte in name or value",
        }
    }
}

/// One parsed `.env` line.
#[derive(Debug, PartialEq, Eq)]
enum EnvLine {
    /// Blank or `#` comment: not a setting, never reported.
    Blank,
    Setting(String, String),
    Ignored(IgnoredEnvLine),
}

/// Loads `path` into the process environment, or returns `None` when there is
/// no file to read. The environment wins: a `.env` value is only a default for
/// a name that is not already set, even to the empty string (ac-51491).
fn load_env_file(path: impl AsRef<Path>) -> Option<EnvLoadReport> {
    let file = fs::File::open(path.as_ref()).ok()?;

    let mut report = EnvLoadReport::default();
    let reader = BufReader::new(file);
    for (index, line) in reader.lines().enumerate() {
        let parsed = match line {
            Ok(line) => parse_env_line(&line),
            Err(_) => EnvLine::Ignored(IgnoredEnvLine::Unreadable),
        };
        match parsed {
            EnvLine::Blank => {}
            EnvLine::Setting(key, value) => {
                if std::env::var_os(&key).is_none() {
                    std::env::set_var(key, value);
                    report.set += 1;
                } else {
                    report.kept += 1;
                }
            }
            EnvLine::Ignored(why) => report.ignored.push((index + 1, why)),
        }
    }
    Some(report)
}

/// Writes `report` to stderr: one warning per ignored line, then one summary
/// line. Counts only — a `.env` commonly holds secrets, so no name or value
/// is ever printed.
fn eprint_env_load_report(path: &str, report: &EnvLoadReport) {
    for (line, why) in &report.ignored {
        eprintln!("warning: {path}:{line}: ignored line: {}", why.reason());
    }
    eprintln!(
        "loaded {path}: set {}, kept {} already in the environment, ignored {}",
        report.set,
        report.kept,
        report.ignored.len()
    );
}

/// Parses one `.env` line. A line `std::env::set_var` would panic on — an
/// empty name, or a NUL byte in the name or value (ac-342bd) — is ignored,
/// like one with no `=`. A value wrapped in matching quotes is unwrapped and
/// unescaped; a lone quote is a literal one-character value.
fn parse_env_line(line: &str) -> EnvLine {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return EnvLine::Blank;
    }
    let Some((key, value)) = line.split_once('=') else {
        return EnvLine::Ignored(IgnoredEnvLine::NoSeparator);
    };
    let key = key.trim();
    let trimmed = value.trim();
    let value = if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\'", "'")
            .replace("\\\\", "\\")
    } else {
        trimmed.to_string()
    };
    if key.is_empty() {
        return EnvLine::Ignored(IgnoredEnvLine::EmptyName);
    }
    if key.contains('\0') || value.contains('\0') {
        return EnvLine::Ignored(IgnoredEnvLine::NulByte);
    }
    EnvLine::Setting(key.to_string(), value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_line(line: &str) -> Option<(String, String)> {
        match parse_env_line(line) {
            EnvLine::Setting(key, value) => Some((key, value)),
            EnvLine::Blank | EnvLine::Ignored(_) => None,
        }
    }

    #[test]
    fn ignored_env_lines_carry_their_reason() {
        for (line, why) in [
            ("NO_SEPARATOR", IgnoredEnvLine::NoSeparator),
            ("=v", IgnoredEnvLine::EmptyName),
            ("  =v", IgnoredEnvLine::EmptyName),
            ("K\0=v", IgnoredEnvLine::NulByte),
            ("K=a\0b", IgnoredEnvLine::NulByte),
            ("K=\"a\0b\"", IgnoredEnvLine::NulByte),
        ] {
            assert_eq!(parse_env_line(line), EnvLine::Ignored(why), "{line:?}");
        }
        for line in ["", "   ", "# comment", "  # indented"] {
            assert_eq!(parse_env_line(line), EnvLine::Blank, "{line:?}");
        }
    }

    fn pair(key: &str, value: &str) -> Option<(String, String)> {
        Some((key.to_string(), value.to_string()))
    }

    #[test]
    fn env_lines_set_var_would_panic_on_are_skipped() {
        for line in ["=v", "  =v", "K\0=v", "K=a\0b", "K=\"a\0b\""] {
            assert_eq!(env_line(line), None, "{line:?}");
        }
    }

    #[test]
    fn env_lines_without_a_setting_are_skipped() {
        for line in ["", "   ", "# comment", "  # indented", "NO_SEPARATOR"] {
            assert_eq!(env_line(line), None, "{line:?}");
        }
    }

    #[test]
    fn env_line_parsing_is_unchanged_for_valid_lines() {
        assert_eq!(env_line("K=\""), pair("K", "\""));
        assert_eq!(env_line("K='"), pair("K", "'"));
        assert_eq!(env_line("K=\"\""), pair("K", ""));
        assert_eq!(env_line("K=''"), pair("K", ""));
        assert_eq!(env_line(r#"K="a\"b""#), pair("K", "a\"b"));
        assert_eq!(env_line(r"K='it\'s'"), pair("K", "it's"));
        assert_eq!(env_line(r#"K="a\\b""#), pair("K", r"a\b"));
        assert_eq!(env_line("K=a=b"), pair("K", "a=b"));
        assert_eq!(env_line(" K = v "), pair("K", "v"));
        assert_eq!(env_line("K="), pair("K", ""));
        assert_eq!(env_line("K=\"mismatched'"), pair("K", "\"mismatched'"));
    }

    /// Writes `contents` to a fresh `.env` in a per-test temp directory, loads
    /// it, and returns what the process environment holds for `key`. Each
    /// test uses its own variable name, so parallel tests cannot interfere.
    fn load_and_read(test: &str, key: &str, contents: &str) -> io::Result<Option<String>> {
        let dir =
            std::env::temp_dir().join(format!("arazzo-mcp-env-{test}-{}", std::process::id()));
        fs::create_dir_all(&dir)?;
        let path = dir.join(".env");
        fs::write(&path, contents)?;
        load_env_file(&path);
        fs::remove_dir_all(&dir)?;
        Ok(std::env::var(key).ok())
    }

    #[test]
    fn env_file_does_not_override_an_exported_variable() -> io::Result<()> {
        let key = "ARAZZO_MCP_TEST_EXPORTED";
        std::env::set_var(key, "from-env");
        let seen = load_and_read("exported", key, &format!("{key}=from-file\n"))?;
        assert_eq!(seen.as_deref(), Some("from-env"));
        Ok(())
    }

    #[test]
    fn env_file_sets_an_absent_variable() -> io::Result<()> {
        let key = "ARAZZO_MCP_TEST_ABSENT";
        std::env::remove_var(key);
        let seen = load_and_read("absent", key, &format!("{key}=from-file\n"))?;
        assert_eq!(seen.as_deref(), Some("from-file"));
        Ok(())
    }

    #[test]
    fn env_file_does_not_override_an_empty_exported_variable() -> io::Result<()> {
        let key = "ARAZZO_MCP_TEST_EMPTY";
        std::env::set_var(key, "");
        let seen = load_and_read("empty", key, &format!("{key}=from-file\n"))?;
        assert_eq!(seen.as_deref(), Some(""));
        Ok(())
    }
}
