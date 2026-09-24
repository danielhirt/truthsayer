//! `truthsayer`: calibrated checks for Claude Code sessions.
//!
//! Commands:
//!
//! - `truthsayer hook`: the Claude Code hook. Reads one event on stdin.
//! - `truthsayer doctor`: shows the effective configuration and setup problems.
//! - `truthsayer rubrics`: lists the built-in rubrics and their questions.

mod config;
mod hook;
mod transcript;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use config::{Config, Mode};
use hook::{Event, HookInput};
use truthsayer::rubric::builtin;

/// Set in the environment of the background process that log mode
/// starts, so that process does the work instead of starting another.
const DETACHED_ENV: &str = "TRUTHSAYER_DETACHED";

const USAGE: &str = "\
truthsayer: calibrated checks for Claude Code sessions

Usage:
  truthsayer hook       Run as a Claude Code hook (reads the event on stdin)
  truthsayer doctor     Show the configuration and any setup problems
  truthsayer rubrics    List the built-in rubrics
  truthsayer --version  Show the version
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("hook") => {
            hook_main();
            // A hook never fails the tool call it watches.
            ExitCode::SUCCESS
        }
        Some("doctor") => doctor(),
        Some("rubrics") => {
            rubrics();
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            println!("truthsayer {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h" | "help") | None => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("truthsayer: unknown command `{other}`\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn log(msg: impl AsRef<str>) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    eprintln!("truthsayer [{secs}]: {}", msg.as_ref());
}

fn project_dir(input: &HookInput) -> Option<PathBuf> {
    std::env::var_os("CLAUDE_PROJECT_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| input.cwd.clone())
}

fn hook_main() {
    let mut raw = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut raw) {
        log(format!("cannot read stdin: {e}"));
        return;
    }
    let input: HookInput = match serde_json::from_str(&raw) {
        Ok(i) => i,
        Err(e) => {
            log(format!("cannot parse hook input: {e}"));
            return;
        }
    };
    let (cfg, warnings) = Config::load(project_dir(&input).as_deref());
    let event = Event::parse(&input.hook_event_name);

    if event == Event::SessionStart {
        let problems = hook::session_start_problems(&cfg, &warnings);
        if !problems.is_empty() {
            emit(&serde_json::json!({
                "systemMessage": format!("truthsayer: {}", problems.join(" "))
            }));
        }
        return;
    }
    for w in &warnings {
        log(w);
    }
    if cfg.mode == Mode::Off {
        return;
    }
    // Log mode never changes what Claude does, so the judge call runs
    // in the background and the tool call does not wait for it.
    if cfg.mode == Mode::Log && std::env::var_os(DETACHED_ENV).is_none() {
        match detach(&raw, &cfg) {
            Ok(()) => return,
            Err(e) => log(format!(
                "cannot start background check, running inline: {e}"
            )),
        }
    }

    let turn = input
        .transcript_path
        .as_deref()
        .map(transcript::read)
        .unwrap_or_default();
    let Some(plan) = hook::plan(&input, &turn, &cfg) else {
        return;
    };
    let judge = match hook::judge_from(&cfg) {
        Ok(j) => j,
        Err(e) => {
            log(format!("skipped: {e}"));
            return;
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log(format!("cannot start runtime: {e}"));
            return;
        }
    };
    let report = match runtime.block_on(hook::supervise(&plan, judge, &cfg, &input)) {
        Ok(r) => r,
        Err(e) => {
            log(format!("skipped: {e}"));
            return;
        }
    };
    if let Some(out) = hook::respond(plan.event, &report, cfg.mode, input.stop_hook_active) {
        emit(&out);
    }
}

fn emit(v: &serde_json::Value) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{v}");
}

/// Start this binary again in its own process group with the same
/// input, and return without waiting. Its stderr goes to `hook.log`
/// next to the record file.
fn detach(raw: &str, cfg: &Config) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let exe = std::env::current_exe()?;
    let stderr = match cfg.record.as_deref().and_then(Path::parent) {
        Some(dir) => Stdio::from(open_private_append(&dir.join("hook.log"))?),
        None => Stdio::null(),
    };
    let mut cmd = Command::new(exe);
    cmd.arg("hook")
        .env(DETACHED_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(stderr);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(raw.as_bytes())?;
    }
    Ok(())
}

fn open_private_append(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

fn doctor() -> ExitCode {
    let cwd = std::env::current_dir().ok();
    let project = std::env::var_os("CLAUDE_PROJECT_DIR")
        .map(PathBuf::from)
        .or(cwd);
    let (cfg, warnings) = Config::load(project.as_deref());
    let show = |p: Option<&Path>| p.map_or("(none)".to_string(), |p| p.display().to_string());

    println!("truthsayer {}", env!("CARGO_PKG_VERSION"));
    println!(
        "user config:    {}",
        show(config::user_config_path().as_deref())
    );
    println!(
        "project config: {}",
        show(
            project
                .map(|d| d.join(".claude").join("truthsayer.toml"))
                .as_deref()
        )
    );
    println!("mode:           {:?}", cfg.mode);
    println!("record:         {}", show(cfg.record.as_deref()));
    println!(
        "model:          {}",
        cfg.model
            .as_deref()
            .unwrap_or(truthsayer::openrouter::DEFAULT_MODEL)
    );
    println!(
        "endpoint:       {}",
        cfg.endpoint
            .as_deref()
            .unwrap_or(truthsayer::openrouter::DEFAULT_ENDPOINT)
    );
    println!("timeout:        {} ms", cfg.timeout_ms);
    println!("constraints:    {}", cfg.constraints.len());
    for c in &cfg.constraints {
        println!("  - {c}");
    }
    if !cfg.skip.is_empty() {
        println!("skipped:        {}", cfg.skip.join(", "));
    }
    let problems = hook::session_start_problems(&cfg, &warnings);
    if problems.is_empty() {
        println!("status:         ready");
        ExitCode::SUCCESS
    } else {
        for p in &problems {
            println!("problem:        {p}");
        }
        ExitCode::from(1)
    }
}

fn rubrics() {
    for r in builtin::all() {
        println!("{} (v{}): {}", r.name, r.version, r.description);
        for (id, q) in &r.questions {
            println!("  {id}: {}", q.instructions());
        }
    }
}
