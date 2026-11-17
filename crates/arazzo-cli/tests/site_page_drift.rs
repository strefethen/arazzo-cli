#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const HERO_OPENAPI: &str = r#"openapi: 3.0.3
info:
  title: Example API
  version: "1.0"
servers:
  - url: https://api.example.com
paths:
  /auth/login:
    post:
      operationId: login
      responses:
        "200":
          description: ok
  /users/me:
    get:
      operationId: getProfile
      responses:
        "200":
          description: ok
"#;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_page() -> String {
    let path = repo_root().join("docs/index.html");
    match fs::read_to_string(&path) {
        Ok(page) => page,
        Err(err) => panic!("failed to read {}: {err}", path.display()),
    }
}

fn run_cli(args: &[&str]) -> Output {
    match Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .args(args)
        .current_dir(repo_root())
        .output()
    {
        Ok(output) => output,
        Err(err) => panic!("failed to run arazzo-cli {args:?}: {err}"),
    }
}

fn strip_tags(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    text
}

fn page_commands(page: &str) -> Vec<String> {
    page.lines()
        .map(|line| strip_tags(line).trim().to_string())
        .filter_map(|line| line.strip_prefix("$ arazzo-cli ").map(str::to_string))
        .collect()
}

fn hero_spec(page: &str) -> String {
    let open = "<pre class=\"code-content\"><code>";
    let Some((_, after_open)) = page.split_once(open) else {
        panic!("docs/index.html has no hero code block");
    };
    let Some((block, _)) = after_open.split_once("</code></pre>") else {
        panic!("docs/index.html hero code block is not closed");
    };
    strip_tags(block)
}

fn command_problems(command: &str) -> Vec<String> {
    let mut tokens = command.split_whitespace();
    let Some(verb) = tokens.next() else {
        return vec!["empty arazzo-cli command".to_string()];
    };

    let help = run_cli(&[verb, "--help"]);
    if !help.status.success() {
        return vec![format!("`{command}`: `{verb}` is not a subcommand")];
    }
    let help_text = String::from_utf8_lossy(&help.stdout);

    let mut problems = Vec::new();
    for token in tokens {
        if token.starts_with("--") && !help_text.contains(token) {
            problems.push(format!("`{command}`: `{verb}` has no flag {token}"));
        }
        if token.starts_with("examples/") && !repo_root().join(token).exists() {
            problems.push(format!("`{command}`: {token} does not exist"));
        }
    }
    problems
}

struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("arazzo-site-{name}-{}", std::process::id()));
        if let Err(err) = fs::create_dir_all(&path) {
            panic!("failed to create {}: {err}", path.display());
        }
        Self(path)
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        if let Err(err) = fs::write(&path, contents) {
            panic!("failed to write {}: {err}", path.display());
        }
        path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn path_arg(path: &Path) -> &str {
    match path.to_str() {
        Some(arg) => arg,
        None => panic!("non-UTF-8 scratch path: {}", path.display()),
    }
}

fn write_hero(name: &str, spec: &str) -> (ScratchDir, PathBuf) {
    let dir = ScratchDir::new(name);
    dir.write("openapi.yaml", HERO_OPENAPI);
    let spec_path = dir.write("auth-flow.arazzo.yaml", spec);
    (dir, spec_path)
}

#[test]
fn page_commands_exist_in_this_build() {
    let commands = page_commands(&read_page());
    assert!(
        commands.len() >= 5,
        "expected the Get Started commands in docs/index.html, found {commands:?}"
    );

    let problems: Vec<String> = commands.iter().flat_map(|c| command_problems(c)).collect();
    assert!(
        problems.is_empty(),
        "docs/index.html documents commands this build does not support:\n{}",
        problems.join("\n")
    );
}

#[test]
fn command_check_rejects_removed_commands_flags_and_examples() {
    assert_eq!(
        command_problems("no-such-command"),
        vec!["`no-such-command`: `no-such-command` is not a subcommand".to_string()]
    );
    assert_eq!(
        command_problems("run examples/httpbin-get.arazzo.yaml get-origin --no-such-flag"),
        vec![
            "`run examples/httpbin-get.arazzo.yaml get-origin --no-such-flag`: `run` has no flag --no-such-flag"
                .to_string()
        ]
    );
    assert_eq!(
        command_problems("validate examples/no-such.arazzo.yaml"),
        vec![
            "`validate examples/no-such.arazzo.yaml`: examples/no-such.arazzo.yaml does not exist"
                .to_string()
        ]
    );
}

#[test]
fn page_commands_ignore_markup_and_other_lines() {
    let page = concat!(
        "<span class=\"prompt\">$</span> arazzo-cli run <span class=\"arg\">a.yaml</span> w<br>\n",
        "<span class=\"prompt\">$</span> cd arazzo-cli\n",
        "<p>arazzo-cli serve</p>\n",
    );
    assert_eq!(page_commands(page), vec!["run a.yaml w".to_string()]);
}

#[test]
fn page_hero_spec_validates_and_dry_runs() {
    let (_dir, spec) = write_hero("hero", &hero_spec(&read_page()));
    let spec = path_arg(&spec);

    let validate = run_cli(&["validate", spec]);
    assert!(
        validate.status.success(),
        "docs/index.html hero spec no longer validates: {}",
        String::from_utf8_lossy(&validate.stderr)
    );

    let dry_run = run_cli(&[
        "run",
        spec,
        "authenticate",
        "--dry-run",
        "--input",
        "username=ada",
        "--input",
        "password=example",
    ]);
    assert!(
        dry_run.status.success(),
        "docs/index.html hero spec no longer dry-runs: {}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    let stdout = String::from_utf8_lossy(&dry_run.stdout);
    assert!(stdout.contains("POST https://api.example.com/auth/login"));
    assert!(stdout.contains("GET https://api.example.com/users/me"));
}

#[test]
fn hero_check_rejects_the_removed_request_path_syntax() {
    let page = read_page();
    let stale = hero_spec(&page).replace("operationId: login", "operationPath: POST /auth/login");
    let (_dir, spec) = write_hero("stale-hero", &stale);

    let dry_run = run_cli(&[
        "run",
        path_arg(&spec),
        "authenticate",
        "--dry-run",
        "--input",
        "username=ada",
        "--input",
        "password=example",
    ]);
    assert!(
        !dry_run.status.success(),
        "custom request-path syntax should be refused"
    );
}
