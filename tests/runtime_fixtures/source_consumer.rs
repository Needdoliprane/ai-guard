//! Engine CLI stand-in for the source-lifecycle test targets.
//!
//! Standard library only: the harness compiles this file with `$RUSTC` at test
//! time and installs the result as `docker` or `podman` in a private control
//! directory. It starts no container and qualifies no Docker or Podman engine.
//!
//! The product starts the engine CLI with a cleared environment and the
//! executable's directory as working directory, so every instruction and every
//! observation lives in `<executable>.fixture/`:
//!
//! | File | Writer | Meaning |
//! |---|---|---|
//! | `plan` | test | `key=value` directives, one per line (see `Plan`) |
//! | `started-<n>` | fixture | first action of invocation `n`: verb and PID |
//! | `calls` | fixture | one JSON array of arguments per invocation, and `effect <target> <verb>` lines |
//! | `observed/<n>/…` | fixture | exact bytes read at invocation `n` |
//! | `alive-<role>` | fixture | exclusive lock held for the whole life of the process |
//! | `ready-<role>` | fixture | published after the lock and after the first real read |
//! | `probe-<k>` / `ack-<role>-<k>` | test / fixture | liveness question and answer |
//! | `material-missing-<role>`, `material-changed-<role>` | fixture | a re-read by pathname failed or differed |
//! | `release` | test | a holding process finishes its scenario |
//! | `quit` | test | every process exits at once |
//!
//! A holding process re-opens its inputs by absolute pathname at every
//! iteration and keeps no descriptor on them in between: a read through a kept
//! descriptor would still succeed after the tree was removed. It writes nothing
//! to its output pipes while it holds.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, exit},
    time::{Duration, Instant},
};

const DESCENDANT: &str = "--fixture-descendant";
const TICK: Duration = Duration::from_millis(20);
const DEFAULT_DEADLINE_SECONDS: u64 = 90;
const DESCENDANT_READY_WINDOW: Duration = Duration::from_secs(20);
const DEFAULT_TARGET: &str = "unix:///fixture.sock";
const STDOUT_LINE: &[u8] = b"fixture-stdout-line\n";
const STDERR_LINE: &[u8] = b"FIXTURE-STDERR-CANARY\n";

const EXIT_UNSUPPORTED: i32 = 64;
const EXIT_QUIT: i32 = 96;
const EXIT_SETUP: i32 = 97;
const EXIT_DEADLINE: i32 = 98;

const SIGHUP: i32 = 1;
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;

unsafe extern "C" {
    fn signal(signal: i32, handler: usize) -> usize;
}

fn main() {
    let executable = std::env::current_exe().expect("fixture executable path");
    let state = executable.with_extension("fixture");
    fs::create_dir_all(&state).expect("fixture state directory");
    let arguments: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    if arguments.first().map(String::as_str) == Some(DESCENDANT) {
        descendant(&state, &arguments[1..]);
    } else {
        leader(&executable, &state, &arguments);
    }
}

/// Directives written by the test before the product runs.
///
/// - `source=<path>` and `mutate=<relative paths, comma separated>`: after its
///   own reads, every invocation `n` rewrites these files of the live source to
///   `live-version n`.
/// - `hold=first-call`: the first invocation holds, whatever its verb.
/// - `hold=consume`: every `build` or `compose` invocation holds after its reads.
/// - `descendant=same-group|own-group`: a consuming leader starts a descendant
///   that inherits both output pipes, reads the same inputs and holds.
/// - `leader-exit=<code>`: the leader exits once its descendant is ready.
/// - `burst=<stdout bytes>,<stderr bytes>` and `exit=<code>`: output and status
///   of a consuming leader that ends by itself.
/// - `ignore-signals=leader|descendant|both`: SIGINT, SIGTERM and SIGHUP.
/// - `alter-prepared=<verb>:<tmpdir>`: the first invocation of `<verb>` that
///   finds a prepared copy under `<tmpdir>` appends one byte to its
///   `context/Cargo.toml`.
/// - `deadline-seconds=<n>`: every process exits by itself after `n` seconds.
struct Plan(BTreeMap<String, String>);

impl Plan {
    fn load(state: &Path) -> Self {
        let text = fs::read_to_string(state.join("plan")).unwrap_or_default();
        Self(
            text.lines()
                .filter_map(|line| line.split_once('='))
                .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
                .collect(),
        )
    }
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
    fn is(&self, key: &str, value: &str) -> bool {
        self.get(key) == Some(value)
    }
    fn number(&self, key: &str) -> Option<u64> {
        self.get(key).and_then(|value| value.parse().ok())
    }
    fn list(&self, key: &str) -> Vec<&str> {
        self.get(key)
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn leader(executable: &Path, state: &Path, arguments: &[String]) {
    let verb = verb(arguments);
    let invocation = claim_invocation(state, verb);
    let plan = Plan::load(state);
    arm_deadline(&plan);
    if matches!(plan.get("ignore-signals"), Some("leader" | "both")) {
        set_signals(SIG_IGN);
    }
    append(
        &state.join("calls"),
        &format!("{}\n", json_array(arguments)),
    );
    let observed = state.join("observed").join(invocation.to_string());
    fs::create_dir_all(&observed).expect("fixture observation directory");
    fs::write(observed.join("arguments"), json_array(arguments)).expect("fixture arguments");
    let target = arguments
        .windows(2)
        .find(|pair| pair[0] == "--host")
        .map_or_else(|| DEFAULT_TARGET.to_owned(), |pair| pair[1].clone());

    if plan.is("hold", "first-call") && invocation == 1 {
        let presence = Presence::enter(state, "leader");
        presence.ready();
        presence.hold(&Reading::default());
    }
    match verb {
        "context" | "info" => {
            alter_prepared(&plan, state, verb);
            mutate(&plan, state, invocation);
            if verb == "context" {
                emit_line(&json_string(&target));
            } else {
                emit_line(info(executable));
            }
        }
        "version" | "build" | "compose" => {
            append(&state.join("calls"), &format!("effect {target} {verb}\n"));
            if verb == "version" {
                mutate(&plan, state, invocation);
                emit_line("fixture");
            } else {
                consume(
                    executable, state, &plan, arguments, verb, invocation, &observed,
                );
            }
        }
        _ => {
            let _ = writeln!(io::stderr(), "fixture: unsupported engine arguments");
            exit(EXIT_UNSUPPORTED);
        }
    }
}

fn consume(
    executable: &Path,
    state: &Path,
    plan: &Plan,
    arguments: &[String],
    verb: &str,
    invocation: u64,
    observed: &Path,
) -> ! {
    let presence = Presence::enter(state, "leader");
    let reading = Inputs::parse(verb, arguments)
        .ok_or_else(|| io::Error::other("input arguments are missing"))
        .and_then(|inputs| inputs.read(Some(observed)));
    let reading = match reading {
        Ok(reading) => reading,
        Err(error) => {
            // A real engine CLI fails the command when an input is unreadable.
            publish(
                state,
                &format!("read-failed-{invocation}"),
                &error.to_string(),
            );
            let _ = writeln!(io::stderr(), "fixture: cannot read an input: {error}");
            exit(1);
        }
    };
    record_live(plan, observed);
    alter_prepared(plan, state, verb);
    mutate(plan, state, invocation);
    presence.ready();

    if let Some(group) = plan.get("descendant") {
        let mut command = Command::new(executable);
        command
            .arg(DESCENDANT)
            .arg(invocation.to_string())
            .args(arguments);
        if group == "own-group" {
            command.process_group(0);
        }
        // Standard output and error are inherited: the descendant keeps the
        // product's two pipes open for as long as it lives.
        if let Err(error) = command.spawn() {
            setup_failed(state, &format!("descendant was not started: {error}"));
        }
        let ready = state.join("ready-descendant");
        let started = Instant::now();
        while !ready.exists() {
            if started.elapsed() > DESCENDANT_READY_WINDOW {
                setup_failed(state, "descendant did not become ready");
            }
            leave_on_quit(state);
            std::thread::sleep(TICK);
        }
    }
    if let Some(code) = plan.number("leader-exit") {
        exit(code as i32);
    }
    if plan.is("hold", "consume") {
        presence.hold(&reading);
    }
    burst(plan);
    emit_line("fixture");
    exit(plan.number("exit").unwrap_or(0) as i32);
}

fn descendant(state: &Path, arguments: &[String]) {
    let plan = Plan::load(state);
    arm_deadline(&plan);
    // An ignored disposition is inherited through exec: set this role's own.
    match plan.get("ignore-signals") {
        Some("descendant" | "both") => set_signals(SIG_IGN),
        Some("leader") => set_signals(SIG_DFL),
        _ => {}
    }
    let engine_arguments = arguments.get(1..).unwrap_or_default();
    let presence = Presence::enter(state, "descendant");
    let reading = Inputs::parse(verb(engine_arguments), engine_arguments)
        .ok_or_else(|| io::Error::other("input arguments are missing"))
        .and_then(|inputs| inputs.read(None));
    match reading {
        Ok(reading) => {
            presence.ready();
            presence.hold(&reading);
        }
        Err(error) => setup_failed(state, &format!("descendant could not read: {error}")),
    }
}

fn verb(arguments: &[String]) -> &str {
    arguments
        .iter()
        .map(String::as_str)
        .find(|argument| {
            matches!(
                *argument,
                "context" | "info" | "version" | "build" | "compose"
            )
        })
        .unwrap_or("unknown")
}

/// `started-<n>` is created exclusively, so concurrent invocations never share
/// a number.
fn claim_invocation(state: &Path, verb: &str) -> u64 {
    for invocation in 1..=10_000 {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(state.join(format!("started-{invocation}")))
        {
            Ok(mut file) => {
                let _ = writeln!(file, "{verb} {}", std::process::id());
                return invocation;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("fixture cannot record its start: {error}"),
        }
    }
    panic!("fixture invocation numbers are exhausted");
}

fn arm_deadline(plan: &Plan) {
    let seconds = plan
        .number("deadline-seconds")
        .unwrap_or(DEFAULT_DEADLINE_SECONDS);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(seconds));
        exit(EXIT_DEADLINE);
    });
}

fn set_signals(handler: usize) {
    for number in [SIGHUP, SIGINT, SIGTERM] {
        // SAFETY: `signal` only replaces a disposition by SIG_DFL or SIG_IGN;
        // no handler function is installed.
        unsafe {
            signal(number, handler);
        }
    }
}

fn info(executable: &Path) -> &'static str {
    if executable.file_name().is_some_and(|name| name == "podman") {
        r#"{"host":{"security":{"rootless":true}},"store":{"graphRoot":"/fixture/graph","runRoot":"/fixture/run"},"version":{"Version":"fixture"}}"#
    } else {
        r#"{"ID":"fixture","SecurityOptions":["name=rootless"],"ServerVersion":"fixture"}"#
    }
}

/// The exclusive lock on `alive-<role>` is this process's proof of life: the
/// kernel releases it only when the process is gone.
struct Presence {
    state: PathBuf,
    role: &'static str,
    _lock: File,
}

impl Presence {
    fn enter(state: &Path, role: &'static str) -> Self {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(state.join(format!("alive-{role}")))
            .expect("fixture liveness file");
        lock.lock().expect("fixture liveness lock");
        Self {
            state: state.to_path_buf(),
            role,
            _lock: lock,
        }
    }
    fn ready(&self) {
        publish(
            &self.state,
            &format!("ready-{}", self.role),
            &std::process::id().to_string(),
        );
    }
    /// Returns when the test writes `release`. Nothing is written to the output
    /// pipes here: after the control process died such a write would fail.
    fn hold(&self, reading: &Reading) {
        let mut answered = BTreeSet::new();
        loop {
            leave_on_quit(&self.state);
            if self.state.join("release").exists() {
                return;
            }
            self.answer_probes(&mut answered);
            match reading.again() {
                Err(error) => self.report("material-missing", &error.to_string()),
                Ok(now) if now != *reading => self.report("material-changed", "content differs"),
                Ok(_) => {}
            }
            std::thread::sleep(TICK);
        }
    }
    fn answer_probes(&self, answered: &mut BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(&self.state) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(probe) = name.strip_prefix("probe-")
                && answered.insert(probe.to_owned())
            {
                publish(
                    &self.state,
                    &format!("ack-{}-{probe}", self.role),
                    &std::process::id().to_string(),
                );
            }
        }
    }
    fn report(&self, kind: &str, detail: &str) {
        let name = format!("{kind}-{}", self.role);
        if !self.state.join(&name).exists() {
            publish(&self.state, &name, detail);
        }
    }
}

/// The inputs an engine command names on its command line.
struct Inputs {
    files: Vec<(&'static str, PathBuf)>,
    context: Option<PathBuf>,
}

impl Inputs {
    fn parse(verb: &str, arguments: &[String]) -> Option<Self> {
        let value = |flag: &str| {
            arguments
                .windows(2)
                .find(|pair| pair[0] == flag)
                .map(|pair| PathBuf::from(&pair[1]))
        };
        match verb {
            "build" => Some(Self {
                files: vec![("dockerfile", value("--file")?)],
                context: Some(PathBuf::from(arguments.last()?)),
            }),
            "compose" => {
                let mut files = vec![("compose-file", value("--file")?)];
                if let Some(environment) = value("--env-file") {
                    files.push(("env-file", environment));
                }
                Some(Self {
                    files,
                    context: None,
                })
            }
            _ => None,
        }
    }

    /// Reads every named input and, for a build, the whole context. With
    /// `observed`, the exact bytes and the entry modes are copied there.
    fn read(&self, observed: Option<&Path>) -> io::Result<Reading> {
        let mut files = Vec::new();
        for (label, path) in &self.files {
            let bytes = fs::read(path)?;
            if let Some(observed) = observed {
                fs::write(observed.join(label), &bytes)?;
            }
            files.push((path.clone(), bytes));
        }
        let directory = match &self.context {
            Some(context) => {
                let mut entries = Vec::new();
                let mut sample = None;
                walk(context, Path::new(""), observed, &mut entries, &mut sample)?;
                if let Some(observed) = observed {
                    let parent = context.parent().unwrap_or(context);
                    entries.push(format!("root {:o} .", mode(context)?));
                    entries.push(format!("parent {:o} ..", mode(parent)?));
                    entries.sort();
                    fs::write(observed.join("entries"), entries.join("\n") + "\n")?;
                }
                // One context file is re-read while holding.
                let preferred = context.join("Cargo.toml");
                if let Some(path) = preferred.is_file().then_some(preferred).or(sample) {
                    let bytes = fs::read(&path)?;
                    files.push((path, bytes));
                }
                context.clone()
            }
            None => self.files[0]
                .1
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
        };
        let listing = list(&directory)?;
        Ok(Reading {
            directory,
            listing,
            files,
        })
    }
}

/// What a process read first, and how to read it again by pathname.
#[derive(Default, PartialEq, Eq)]
struct Reading {
    directory: PathBuf,
    listing: Vec<String>,
    files: Vec<(PathBuf, Vec<u8>)>,
}

impl Reading {
    fn again(&self) -> io::Result<Self> {
        if self.directory.as_os_str().is_empty() {
            return Ok(Self::default());
        }
        let mut files = Vec::new();
        for (path, _) in &self.files {
            files.push((path.clone(), fs::read(path)?));
        }
        Ok(Self {
            directory: self.directory.clone(),
            listing: list(&self.directory)?,
            files,
        })
    }
}

fn list(directory: &Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(directory)? {
        names.push(entry?.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}

fn mode(path: &Path) -> io::Result<u32> {
    Ok(fs::symlink_metadata(path)?.permissions().mode() & 0o7777)
}

/// Depth-first walk that never follows a symbolic link.
fn walk(
    root: &Path,
    relative: &Path,
    observed: Option<&Path>,
    entries: &mut Vec<String>,
    sample: &mut Option<PathBuf>,
) -> io::Result<()> {
    for name in list(&root.join(relative))? {
        let relative = relative.join(&name);
        let path = root.join(&relative);
        let metadata = fs::symlink_metadata(&path)?;
        let bits = metadata.permissions().mode() & 0o7777;
        let kind = if metadata.file_type().is_symlink() {
            "link"
        } else if metadata.is_dir() {
            "dir"
        } else if metadata.is_file() {
            "file"
        } else {
            "other"
        };
        entries.push(format!("{kind} {bits:o} {}", relative.display()));
        match kind {
            "dir" => walk(root, &relative, observed, entries, sample)?,
            "file" => {
                let bytes = fs::read(&path)?;
                if let Some(observed) = observed {
                    let copy = observed.join("context").join(&relative);
                    if let Some(parent) = copy.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(copy, bytes)?;
                }
                sample.get_or_insert(path);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Copies the live canaries as they are when this consumer ran.
fn record_live(plan: &Plan, observed: &Path) {
    let Some(source) = plan.get("source") else {
        return;
    };
    for relative in plan.list("mutate") {
        if let Ok(bytes) = fs::read(Path::new(source).join(relative)) {
            let copy = observed.join("live").join(relative);
            if let Some(parent) = copy.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(copy, bytes);
        }
    }
}

/// Stands for a second writer of the live source between two engine commands.
fn mutate(plan: &Plan, state: &Path, invocation: u64) {
    let Some(source) = plan.get("source") else {
        return;
    };
    for relative in plan.list("mutate") {
        let path = Path::new(source).join(relative);
        if let Err(error) = fs::write(&path, format!("live-version {invocation}\n")) {
            publish(
                state,
                &format!("mutate-failed-{invocation}"),
                &format!("{}: {error}", path.display()),
            );
        }
    }
}

fn alter_prepared(plan: &Plan, state: &Path, verb: &str) {
    let Some((planned, temporary)) = plan
        .get("alter-prepared")
        .and_then(|value| value.split_once(':'))
    else {
        return;
    };
    if planned != verb || state.join("altered").exists() {
        return;
    }
    let Ok(entries) = fs::read_dir(temporary) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("ai-guard-distribution-")
        {
            continue;
        }
        let target = entry.path().join("context").join("Cargo.toml");
        let appended = OpenOptions::new()
            .append(true)
            .open(&target)
            .and_then(|mut file| file.write_all(b"#"));
        if appended.is_ok() {
            publish(state, "altered", &target.to_string_lossy());
            return;
        }
    }
}

fn burst(plan: &Plan) {
    let Some((stdout, stderr)) = plan.get("burst").and_then(|value| value.split_once(',')) else {
        return;
    };
    let fill = |mut writer: Box<dyn Write>, line: &[u8], total: usize| {
        let bytes: Vec<u8> = line.iter().copied().cycle().take(total).collect();
        let _ = writer.write_all(&bytes);
        let _ = writer.flush();
    };
    fill(
        Box::new(io::stdout()),
        STDOUT_LINE,
        stdout.trim().parse().unwrap_or(0),
    );
    fill(
        Box::new(io::stderr()),
        STDERR_LINE,
        stderr.trim().parse().unwrap_or(0),
    );
}

/// A removed state directory means the test is over as well.
fn leave_on_quit(state: &Path) {
    if state.join("quit").exists() || !state.exists() {
        exit(EXIT_QUIT);
    }
}

fn setup_failed(state: &Path, detail: &str) -> ! {
    publish(state, "setup-failed", detail);
    exit(EXIT_SETUP);
}

/// A closed output pipe must not turn into a panic of the stand-in.
fn emit_line(text: &str) {
    let mut stdout = io::stdout();
    let _ = writeln!(stdout, "{text}");
    let _ = stdout.flush();
}

fn append(path: &Path, text: &str) {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .expect("fixture record");
}

/// Readers never see a partial file: temporary name, then rename.
fn publish(state: &Path, name: &str, content: &str) {
    let temporary = state.join(format!(".{name}.{}.tmp", std::process::id()));
    fs::write(&temporary, content).expect("fixture marker");
    fs::rename(&temporary, state.join(name)).expect("fixture marker publication");
}

fn json_array(values: &[String]) -> String {
    let items: Vec<String> = values.iter().map(|value| json_string(value)).collect();
    format!("[{}]", items.join(","))
}

fn json_string(value: &str) -> String {
    let mut text = String::from('"');
    for character in value.chars() {
        match character {
            '"' => text.push_str("\\\""),
            '\\' => text.push_str("\\\\"),
            control if (control as u32) < 0x20 => {
                text.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => text.push(other),
        }
    }
    text.push('"');
    text
}
