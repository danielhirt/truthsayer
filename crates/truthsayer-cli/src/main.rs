//! `truthsayer`: calibrated checks for Claude Code sessions.

mod config;
mod eval;
mod hook;
mod label;
mod replay;
mod report;
mod store;
mod transcript;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use config::{Config, Mode};
use hook::{Event, HookInput};
use truthsayer::records::{open_private_append, rotate_if_needed};
use truthsayer::rubric::builtin;

/// Set in the environment of the background process that log mode
/// starts, so that process does the work instead of starting another.
const DETACHED_ENV: &str = "TRUTHSAYER_DETACHED";

/// `hook.log` rotates at this size; one old file is kept.
const HOOK_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Parser)]
#[command(
    name = "truthsayer",
    version,
    about = "Calibrated checks for Claude Code sessions"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run as a Claude Code hook. Reads one hook event on stdin.
    Hook,
    /// Show the configuration and any setup problems.
    Doctor,
    /// List the built-in rubrics and their questions.
    Rubrics,
    /// Apply the current or edited rubrics to recorded answers. Makes no judge calls.
    Replay {
        /// The record file. Default: the record path from the configuration.
        #[arg(long)]
        records: Option<PathBuf>,
        /// A rubric file that replaces the built-in rubric with the same name. Repeatable.
        #[arg(long = "rubric")]
        rubrics: Vec<PathBuf>,
        /// How many changed records to list.
        #[arg(long, default_value_t = 20)]
        show: usize,
    },
    /// Give the true answer to one question on recorded calls.
    Label {
        /// The question, as rubric.question. Example: turn-end.unverified_claim
        #[arg(long)]
        question: String,
        #[arg(long)]
        records: Option<PathBuf>,
        /// The most records to label in this session.
        #[arg(long, default_value_t = 25)]
        limit: usize,
        /// Show the judge's answer. Hidden by default so it does not bias you.
        #[arg(long)]
        show_answer: bool,
        /// Also offer records that already have a true answer.
        #[arg(long)]
        relabel: bool,
    },
    /// Measure labeled questions: calibration, thresholds, and code heuristics.
    Report {
        #[arg(long)]
        records: Option<PathBuf>,
        /// tune, holdout, or all.
        #[arg(long, default_value = "all")]
        split: String,
        /// Only this question, as rubric.question.
        #[arg(long)]
        question: Option<String>,
        /// A rubric file whose thresholds to mark. Repeatable.
        #[arg(long = "rubric")]
        rubrics: Vec<PathBuf>,
    },
    /// Run a labeled case set through the judge and score it against the code heuristics.
    Eval {
        /// Case files (TOML), or directories of them.
        #[arg(required = true)]
        cases: Vec<PathBuf>,
        /// The directory for records.jsonl, truth.jsonl, run.json, and summary.md. It must not hold records yet.
        #[arg(long, required_unless_present = "dry_run")]
        out: Option<PathBuf>,
        /// How many times to ask about each case.
        #[arg(long, default_value_t = 1)]
        repeat: usize,
        /// How many judge calls to have in flight.
        #[arg(long, default_value_t = 4)]
        concurrency: usize,
        /// The judge model. Default: the model from the configuration.
        #[arg(long)]
        model: Option<String>,
        /// A rubric file that replaces the built-in rubric with the same name. Repeatable.
        #[arg(long = "rubric")]
        rubrics: Vec<PathBuf>,
        /// Check the cases and print the state each rubric will see. Makes no judge calls.
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Hook => {
            hook_main();
            // A hook never fails the tool call it watches.
            ExitCode::SUCCESS
        }
        Command::Doctor => doctor(),
        Command::Rubrics => {
            rubrics();
            ExitCode::SUCCESS
        }
        Command::Replay {
            records,
            rubrics,
            show,
        } => finish(replay_cmd(records, &rubrics, show)),
        Command::Label {
            question,
            records,
            limit,
            show_answer,
            relabel,
        } => finish(label_cmd(
            records,
            label::Options {
                question,
                limit,
                show_answer,
                relabel,
            },
        )),
        Command::Report {
            records,
            split,
            question,
            rubrics,
        } => finish(report_cmd(records, &split, question.as_deref(), &rubrics)),
        Command::Eval {
            cases,
            out,
            repeat,
            concurrency,
            model,
            rubrics,
            dry_run,
        } => finish(eval_cmd(
            &cases,
            out,
            repeat,
            concurrency,
            model,
            &rubrics,
            dry_run,
        )),
    }
}

fn finish(r: Result<(), String>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("truthsayer: {e}");
            ExitCode::from(1)
        }
    }
}

/// The record file to read: the flag, else the configured path.
fn records_path(flag: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(p) = flag {
        return Ok(p);
    }
    let cwd = std::env::current_dir().ok();
    let (cfg, _) = Config::load(cwd.as_deref());
    cfg.record
        .ok_or_else(|| "recording is off in the configuration; pass --records".to_string())
}

fn load(flag: Option<PathBuf>) -> Result<(PathBuf, Vec<store::Loaded>), String> {
    let path = records_path(flag)?;
    let (records, bad) = store::load_records(&path);
    if records.is_empty() {
        return Err(format!("no records found at {}", path.display()));
    }
    if bad > 0 {
        eprintln!("truthsayer: skipped {bad} lines that are not valid records");
    }
    Ok((path, records))
}

fn replay_cmd(
    records: Option<PathBuf>,
    rubric_files: &[PathBuf],
    show: usize,
) -> Result<(), String> {
    let (_, records) = load(records)?;
    let rubrics = replay::load_rubrics(rubric_files)?;
    quiet_pipe(replay::run(
        &records,
        &rubrics,
        show,
        &mut std::io::stdout().lock(),
    ))
}

fn label_cmd(records: Option<PathBuf>, opts: label::Options) -> Result<(), String> {
    let (path, records) = load(records)?;
    let truth_file = store::truth_path(&path);
    let truth = store::load_truth(&truth_file);
    let rubrics = builtin::all();
    let saved = label::run(
        &records,
        &rubrics,
        &truth,
        &truth_file,
        &opts,
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
    )?;
    println!(
        "
saved {saved} answers to {}",
        truth_file.display()
    );
    Ok(())
}

fn report_cmd(
    records: Option<PathBuf>,
    split: &str,
    question: Option<&str>,
    rubric_files: &[PathBuf],
) -> Result<(), String> {
    let split = store::Split::parse(split)
        .ok_or_else(|| format!("unknown split `{split}`: use tune, holdout, or all"))?;
    let (path, records) = load(records)?;
    let truth = store::load_truth(&store::truth_path(&path));
    let rubrics = replay::load_rubrics(rubric_files)?;
    quiet_pipe(report::run(
        &records,
        &truth,
        &rubrics,
        split,
        question,
        &mut std::io::stdout().lock(),
    ))
}

fn eval_cmd(
    paths: &[PathBuf],
    out: Option<PathBuf>,
    repeat: usize,
    concurrency: usize,
    model: Option<String>,
    rubric_files: &[PathBuf],
    dry_run: bool,
) -> Result<(), String> {
    let files = eval::expand(paths)?;
    let rubrics = replay::load_rubrics(rubric_files)?;
    let cases = eval::load_cases(&files, &rubrics)?;
    if dry_run {
        let mut stdout = std::io::stdout().lock();
        for c in &cases {
            quiet_pipe(eval::describe(c, &rubrics, &mut stdout))?;
        }
        println!("{} cases are valid", cases.len());
        return Ok(());
    }
    let cwd = std::env::current_dir().ok();
    let (mut cfg, _) = Config::load(cwd.as_deref());
    if model.is_some() {
        cfg.model = model;
    }
    let judge = hook::judge_from(&cfg)?;
    let opts = eval::Options {
        out: out.ok_or("--out is required")?,
        repeat: repeat.max(1),
        concurrency,
    };
    eprintln!(
        "truthsayer: {} cases x {} repeats = {} judge calls",
        cases.len(),
        opts.repeat,
        cases.len() * opts.repeat
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let text = runtime.block_on(eval::run(&cases, &rubrics, judge, &files, &opts))?;
    print!("{text}");
    eprintln!(
        "truthsayer: wrote {}; measure it with `truthsayer report --records {}`",
        opts.out.display(),
        opts.out.join("records.jsonl").display()
    );
    Ok(())
}

/// A closed stdout (for example `truthsayer report | head`) is not an
/// error worth reporting.
fn quiet_pipe(r: std::io::Result<()>) -> Result<(), String> {
    match r {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => other.map_err(|e| e.to_string()),
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
    // Log mode never changes what Claude does, so a tool event's judge
    // call runs in the background and the tool call does not wait for
    // it. Stop runs inline: it happens once per turn, after Claude is
    // done, and a background call can outlive the session's network
    // path when the session exits right after the stop.
    if cfg.mode == Mode::Log && event != Event::Stop && std::env::var_os(DETACHED_ENV).is_none() {
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
            log(format!("{} skipped: {e}", input.hook_event_name));
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
        Some(dir) => {
            let log = dir.join("hook.log");
            rotate_if_needed(&log, HOOK_LOG_MAX_BYTES, 1)?;
            Stdio::from(open_private_append(&log)?)
        }
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
    let backend = cfg.backend();
    println!("backend:        {backend:?} (key in {})", cfg.key_env());
    println!(
        "model:          {}",
        cfg.model.as_deref().unwrap_or(backend.default_model())
    );
    println!(
        "endpoint:       {}",
        cfg.endpoint
            .as_deref()
            .unwrap_or(backend.default_endpoint())
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
