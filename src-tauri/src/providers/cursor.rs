//! Cursor Agent CLI (`agent acp`) over the shared ACP transport.
//!
//! This is the only place that knows Cursor's wire strings, the executable PandaCode
//! accepted, and the mode rule. `acp.rs`, `host_tools.rs`, `providers.rs` and the
//! allowlist files call the items here and do not repeat the strings.
//!
//! Cursor keeps its own sign-in (`agent login`, `CURSOR_API_KEY`, `CURSOR_AUTH_TOKEN`);
//! PandaCode never opens a browser, shows a login URL, or stores a key. A program is
//! Cursor only when an ACP `initialize` handshake says so (`cursor_login` in
//! `authMethods`); a file whose stem is `cursor` is the editor and never a candidate.
//!
//! Evidence: the scrubbed recordings in `cursor_frames/` (see its `NOTES.md`). Frames
//! built from `cursor_frames/extension-schemas.md` are labelled schema-derived in the
//! tests that use them; they were never recorded.
use super::*;

pub(crate) const LOGIN_METHOD: &str = "cursor_login";
/// The task-run `authenticate` request id; `acp.rs` writes it, `AuthWatch` follows it.
pub(crate) const AUTHENTICATE_ID: u64 = 7;
pub(crate) const SIGN_IN_ERROR: &str = "The Cursor CLI is not signed in. Run `agent login` in a terminal (or set CURSOR_API_KEY or CURSOR_AUTH_TOKEN in Settings → Providers), then send again.";
/// Matched case-insensitively against stderr with ANSI escapes removed.
pub(crate) const SIGN_IN_MARKERS: [&str; 4] = ["authentication required", "api key is invalid", "not logged in", "agent login"];
pub(crate) const MODELS_UNRECOGNISED: &str = "PandaCode could not read the Cursor CLI's model list. Run `agent models` in a terminal to see what it prints.";
const MODEL_REJECTED_PREFIX: &str = "The Cursor CLI refused the model “";
const MODEL_REJECTED_CAUSE: &str = "Cannot use this model:";
pub(crate) const PLAN_UNAVAILABLE: &str = "The Cursor CLI did not offer Plan mode for this session, so it runs in the mode the session reports. PandaCode declines the non-read-only tool requests Cursor asks about.";
/// Prefix of the status line for a question PandaCode could not show; the short reason follows.
pub(crate) const QUESTION_REFUSED: &str = "Cursor CLI question declined: ";
const PLAN_REFUSED: &str = "Cursor CLI plan declined: ";
const TASK_NOTE: &str = "The Cursor CLI asked to start a sub-task; PandaCode did not run it.";
/// Both bounds are the existing CLI startup bound; `cursor.rs` owns no timer of its own.
pub(crate) const AUTH_DEADLINE: Duration = super::acp::DISCOVERY_STARTUP;
pub(crate) const ONCE_ALLOW: &str = "allow_once";
pub(crate) const ONCE_REJECT: &str = "reject_once";
pub(crate) const EXTENSION_PREFIX: &str = "cursor/";
pub(crate) const PLAN_READ_ONLY: [&str; 4] = ["read", "search", "fetch", "think"];
pub(crate) const PLAN_FLAG: &str = "--mode=plan";
const AGENT_NAME: &str = "agent";
const NON_AGENT_IDS: [&str; 3] = ["ask", "plan", "debug"];
const PLAN_ID: &str = "plan";
const MAX_QUESTIONS: usize = 3;
const MAX_OPTIONS: usize = 20;
const TODO_LIMIT: usize = 100;
const ASK_QUESTION: &str = "cursor/ask_question";
const CREATE_PLAN: &str = "cursor/create_plan";
const UPDATE_TODOS: &str = "cursor/update_todos";
const TASK: &str = "cursor/task";
const GENERATE_IMAGE: &str = "cursor/generate_image";
/// Marks the program that prints a browser login URL instead of opening a browser.
pub(crate) const NO_OPEN_BROWSER: &str = "NO_OPEN_BROWSER";

#[cfg(test)]
tokio::task_local! {
    pub(crate) static TEST_AUTH_DEADLINE: Duration;
}

/// The `authenticate` wait, shortened under test.
pub(crate) fn auth_deadline() -> Duration {
    #[cfg(test)]
    if let Ok(deadline) = TEST_AUTH_DEADLINE.try_with(|deadline| *deadline) {
        return deadline;
    }
    AUTH_DEADLINE
}

pub(crate) fn model_rejected(model: &str) -> String {
    format!("{MODEL_REJECTED_PREFIX}{}”. Choose another model from the list.", model_text(Some(model), "", 120))
}

pub(crate) fn model_not_confirmed(chosen: &str, current: &str) -> String {
    format!(
        "The Cursor CLI did not confirm the model “{}” for this session and is running “{}”.",
        model_text(Some(chosen), "", 120),
        model_text(Some(current), "its default", 120)
    )
}

/// An error this module wrote itself, whose text must not gain the CLI's stderr
/// (which can carry a login URL or the account's model list).
pub(crate) fn is_own_error(error: &str) -> bool {
    error == SIGN_IN_ERROR || error.starts_with(MODEL_REJECTED_PREFIX)
}

/// A run that failed only because nobody is signed in. The provider stays installed, so this
/// is not the model's failure and is never recorded as one.
pub(crate) fn is_sign_in_error(error: &str) -> bool {
    error == SIGN_IN_ERROR
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.peek() == Some(&'\\')) {
                            if c != '\u{7}' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            '\u{200b}'..='\u{200d}' | '\u{feff}' => {}
            c => out.push(c),
        }
    }
    out
}

/// The error for a child exit while `authenticate` is pending: an unknown `--model` slug
/// makes the CLI exit about 8 s after `authenticate` with no reply (recorded in
/// `model-unknown-load.jsonl`); anything else is taken for a missing sign-in.
pub(crate) fn auth_exit(stderr: &str) -> String {
    let clean = strip_ansi(stderr);
    match clean.split_once(MODEL_REJECTED_CAUSE) {
        Some((_, rest)) => {
            let slug = rest.split_whitespace().next().unwrap_or_default().trim_end_matches('.');
            model_rejected(slug)
        }
        None => SIGN_IN_ERROR.into(),
    }
}

// ---------------------------------------------------------------------------------
// The `authenticate` deadline: a pure arm / disarm decision. `providers.rs` owns the timer.

#[derive(Default)]
pub(crate) struct AuthWatch {
    enabled: bool,
    deadline: Option<tokio::time::Instant>,
}

impl AuthWatch {
    pub(crate) fn for_provider(provider: &str) -> Self {
        Self { enabled: provider == "cursor", deadline: None }
    }
    pub(crate) fn armed(&self) -> bool {
        self.deadline.is_some()
    }
    pub(crate) fn deadline(&self) -> Option<tokio::time::Instant> {
        self.deadline
    }
    /// Armed by a written `authenticate` request.
    pub(crate) fn written(&mut self, frames: &[Value], now: tokio::time::Instant) {
        if self.enabled && self.deadline.is_none() && frames.iter().any(|frame| frame["method"] == "authenticate") {
            self.deadline = Some(now + auth_deadline());
        }
    }
    /// Disarmed only by the response: id 7, a `result` or `error`, and no `method`. The CLI
    /// numbers its own requests from 0, so a request that happens to use id 7 leaves it armed.
    pub(crate) fn received(&mut self, frame: &Value) {
        if self.deadline.is_some() && frame["id"] == AUTHENTICATE_ID && frame.get("method").is_none() && (frame.get("result").is_some() || frame.get("error").is_some()) {
            self.deadline = None;
        }
    }
}

// ---------------------------------------------------------------------------------
// Identity and the probe.

pub(crate) fn identifies_cursor(initialize_result: &Value) -> bool {
    initialize_result["authMethods"].as_array().is_some_and(|methods| methods.iter().any(|method| method["id"] == LOGIN_METHOD))
}

/// The editor's launcher is called `cursor`; it is never the CLI, however it was supplied.
pub(crate) fn rejected_name(path: &Path) -> bool {
    path.file_stem().is_some_and(|stem| stem.to_string_lossy().eq_ignore_ascii_case("cursor"))
}

#[derive(Clone)]
enum Outcome {
    Pending,
    Done(Option<PathBuf>),
}

#[derive(Default)]
struct State {
    /// `None`: no verdict yet. `Some(None)`: probed, not installed. `Some(Some(path))`: installed.
    verdict: Option<Option<PathBuf>>,
    accepted: HashSet<PathBuf>,
    version: Option<String>,
    generation: u64,
    inflight: Option<(u64, tokio::sync::watch::Receiver<Outcome>)>,
}

fn state() -> std::sync::MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Mutex::default).lock().unwrap_or_else(|error| error.into_inner())
}

type Callback = Arc<dyn Fn() + Send + Sync>;

fn callback() -> std::sync::MutexGuard<'static, Option<Callback>> {
    static CALLBACK: OnceLock<Mutex<Option<Callback>>> = OnceLock::new();
    CALLBACK.get_or_init(Mutex::default).lock().unwrap_or_else(|error| error.into_inner())
}

/// Registers the one callback the probe calls when a stored verdict differs from the last
/// (the installed bit or the path). Set once at startup.
pub(crate) fn on_settled(notify: impl Fn() + Send + Sync + 'static) {
    *callback() = Some(Arc::new(notify));
}

/// Reads the verdict only; never spawns. `Some(path)` is installed.
pub(crate) fn cached_installed() -> Option<PathBuf> {
    state().verdict.clone().flatten()
}

/// Starts one background probe when there is no verdict and none is in flight, and returns at
/// once. Scheduled on the Tauri runtime, which exists on `discover`'s blocking threads.
pub(crate) fn ensure_probe() {
    let mut state = state();
    if state.verdict.is_none() && state.inflight.is_none() {
        let _ = start(&mut state);
    }
}

/// Resolves with the accepted program. With no verdict it starts a probe (or joins the one
/// in flight) and waits; a cached negative verdict is not trusted, so a CLI installed after
/// launch is found by the next run, list, start or console. `None`: the probe settled as
/// not installed.
pub(crate) async fn accepted_executable() -> Option<PathBuf> {
    let receiver = {
        let mut state = state();
        if let Some(Some(path)) = &state.verdict {
            return Some(path.clone());
        }
        match &state.inflight {
            Some((_, receiver)) => receiver.clone(),
            None => start(&mut state),
        }
    };
    wait(receiver).await
}

/// A fresh probe even when a verdict exists. The old verdict stays readable until the new
/// one replaces it. A probe already running may have started before the call, so it is
/// superseded (a new generation, a new probe) rather than joined: the forced list must
/// return the verdict read now.
pub(crate) async fn refresh() -> Option<PathBuf> {
    let receiver = {
        let mut state = state();
        state.version = None;
        state.generation = state.generation.wrapping_add(1);
        state.inflight = None;
        start(&mut state)
    };
    wait(receiver).await
}

/// Drops the verdict and the accepted-path set, then probes again: the candidate set changed
/// (an override was saved) or a cached path vanished.
pub(crate) fn invalidate_handshake() {
    let mut state = state();
    state.verdict = None;
    state.accepted.clear();
    state.version = None;
    state.generation = state.generation.wrapping_add(1);
    state.inflight = None;
    let _ = start(&mut state);
}

/// Remembers one exact program (a named connection's binary) once its handshake says it is
/// Cursor. It does not replace the discovered default, and a failure caches nothing. The
/// connection's own environment is used, so a binary that needs its `PATH` or config home to
/// start is verified as the run would start it.
pub(crate) async fn accept_path(path: &Path, env: &[(String, String)]) -> Result<(), String> {
    if rejected_name(path) {
        return Err("This program is the Cursor editor's launcher, not the Cursor CLI. Choose the `agent` program from the Cursor CLI install.".into());
    }
    {
        let state = state();
        if state.accepted.contains(path) || state.verdict.as_ref().and_then(Option::as_ref).is_some_and(|accepted| accepted == path) {
            return Ok(());
        }
    }
    let result = handshake(path, env).await.map_err(|error| format!("The Cursor CLI program could not be verified: {error}"))?;
    if !identifies_cursor(&result) {
        return Err("This program did not identify itself as the Cursor CLI. Choose the `agent` program from the Cursor CLI install.".into());
    }
    state().accepted.insert(path.to_owned());
    Ok(())
}

/// `--version` of the accepted program, run once by `read` and kept for the process
/// (`refresh` and `invalidate_handshake` drop it). An empty read is kept too, so a failed
/// `--version` is not spawned again on every provider list.
pub(crate) fn version(path: &Path, read: impl FnOnce(&Path) -> Option<String>) -> String {
    if let Some(version) = state().version.clone() {
        return version;
    }
    let version = read(path).unwrap_or_default();
    state().version = Some(version.clone());
    version
}

fn start(state: &mut State) -> tokio::sync::watch::Receiver<Outcome> {
    let generation = state.generation;
    let (sender, receiver) = tokio::sync::watch::channel(Outcome::Pending);
    state.inflight = Some((generation, receiver.clone()));
    tauri::async_runtime::spawn(async move {
        let mut finish = Finish { generation, sender: Some(sender) };
        let found = scan().await;
        finish.complete(found);
    });
    receiver
}

async fn wait(mut receiver: tokio::sync::watch::Receiver<Outcome>) -> Option<PathBuf> {
    loop {
        if let Outcome::Done(path) = &*receiver.borrow() {
            return path.clone();
        }
        if receiver.changed().await.is_err() {
            return None;
        }
    }
}

/// Stores the verdict when its probe finishes; a probe that dies leaves the verdict unset
/// rather than recording "not installed".
struct Finish {
    generation: u64,
    sender: Option<tokio::sync::watch::Sender<Outcome>>,
}

impl Finish {
    fn complete(&mut self, found: Option<PathBuf>) {
        let changed = {
            let mut state = state();
            let current = state.generation == self.generation;
            if current {
                let before = state.verdict.clone().flatten();
                state.verdict = Some(found.clone());
                state.inflight = None;
                before != found
            } else {
                false
            }
        };
        if changed {
            if let Some(notify) = callback().clone() {
                notify();
            }
        }
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(Outcome::Done(found));
        }
    }
}

impl Drop for Finish {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let mut state = state();
            if state.generation == self.generation {
                state.inflight = None;
            }
            drop(state);
            let _ = sender.send(Outcome::Done(None));
        }
    }
}

async fn scan() -> Option<PathBuf> {
    for path in candidates() {
        if rejected_name(&path) {
            continue;
        }
        if let Ok(result) = handshake(&path, &[]).await {
            if identifies_cursor(&result) {
                return Some(path);
            }
        }
    }
    None
}

/// The saved Settings override alone, else `agent` then `cursor-agent` on PATH. A candidate
/// is used only after the handshake identifies Cursor.
fn candidates() -> Vec<PathBuf> {
    #[cfg(test)]
    if let Some(hook) = hook().as_ref() {
        return hook.candidates.clone();
    }
    if let Some(path) = crate::provider_settings::executable_override("cursor") {
        return vec![path];
    }
    ["agent", "cursor-agent"].into_iter().filter_map(super::launch::find).collect()
}

/// `initialize` alone, on the program as the run would launch it (so a Windows `agent.cmd`
/// becomes `node.exe index.js`), with the connection's environment when there is one.
/// Never `authenticate`, never a session.
async fn handshake(path: &Path, env: &[(String, String)]) -> Result<Value, String> {
    #[cfg(test)]
    {
        let hooked = hook().as_ref().map(|hook| hook.handshake.clone());
        if let Some(handshake) = hooked {
            return handshake(path);
        }
    }
    let command = handshake_command(path, env)?;
    super::acp::handshake(command, &std::env::temp_dir()).await
}

fn handshake_command(path: &Path, env: &[(String, String)]) -> Result<Command, String> {
    let mut command = super::provider_command("cursor", path)?;
    command.envs(env.iter().map(|(key, value)| (key, value)));
    command.env(NO_OPEN_BROWSER, "1").arg("acp");
    Ok(command)
}

// ---------------------------------------------------------------------------------
// Test hooks.

#[cfg(test)]
type Handshake = Arc<dyn Fn(&Path) -> Result<Value, String> + Send + Sync>;

#[cfg(test)]
struct Hook {
    candidates: Vec<PathBuf>,
    handshake: Handshake,
}

#[cfg(test)]
fn hook() -> std::sync::MutexGuard<'static, Option<Hook>> {
    static HOOK: OnceLock<Mutex<Option<Hook>>> = OnceLock::new();
    HOOK.get_or_init(Mutex::default).lock().unwrap_or_else(|error| error.into_inner())
}

/// Replaces the candidate list and the handshake and clears the verdict, so cold-start tests
/// in other files run a probe without a binary. Hold `probe_test_guard()` for the whole test.
#[cfg(test)]
pub(crate) fn set_probe_for_test(candidates: Vec<PathBuf>, handshake: impl Fn(&Path) -> Result<Value, String> + Send + Sync + 'static) {
    *hook() = Some(Hook { candidates, handshake: Arc::new(handshake) });
    *callback() = None;
    let mut state = state();
    *state = State { generation: state.generation.wrapping_add(1), ..State::default() };
}

/// A process-wide lock: parallel tests must not share the probe hook.
#[cfg(test)]
pub(crate) fn probe_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: Mutex<()> = Mutex::new(());
    GUARD.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
type ModelsRun = Arc<dyn Fn(&Path) -> Result<Captured, String> + Send + Sync>;

#[cfg(test)]
fn models_hook() -> std::sync::MutexGuard<'static, Option<ModelsRun>> {
    static HOOK: OnceLock<Mutex<Option<ModelsRun>>> = OnceLock::new();
    HOOK.get_or_init(Mutex::default).lock().unwrap_or_else(|error| error.into_inner())
}

/// Replaces `agent models` for one test; the guard clears it on drop. Hold
/// `probe_test_guard()` as well.
#[cfg(test)]
struct ModelsGuard;

#[cfg(test)]
impl Drop for ModelsGuard {
    fn drop(&mut self) {
        *models_hook() = None;
    }
}

#[cfg(test)]
fn set_models_for_test(run: impl Fn(&Path) -> Result<Captured, String> + Send + Sync + 'static) -> ModelsGuard {
    *models_hook() = Some(Arc::new(run));
    ModelsGuard
}

// ---------------------------------------------------------------------------------
// Launch.

/// Flags and environment for the program the run started: `--model <id>` for a chosen model,
/// then `--mode=plan` in Plan (also on a resume: the flag goes on the one process). `acp` is
/// not added here; `run_inner` appends allowed connection arguments first and `acp` last.
/// Never `--mode=ask`, `--force`, `--yolo`, `--auto-review`, `--approve-mcps` or `--trust`.
pub(crate) fn configure(command: &mut Command, config: &RunConfig) {
    command.env(NO_OPEN_BROWSER, "1");
    command.args(flags(config));
}

fn flags(config: &RunConfig) -> Vec<String> {
    let mut argv = Vec::new();
    if !config.model.is_empty() && config.model != "default" {
        argv.extend(["--model".to_owned(), config.model.clone()]);
    }
    if config.mode == "plan" {
        argv.push(PLAN_FLAG.to_owned());
    }
    argv
}

/// The full argv for a task run: `--model`, the Plan flag, the connection's allowed
/// arguments once, then `acp` last.
pub(crate) fn spawn_arguments(config: &RunConfig, extra: &[String]) -> Result<Vec<String>, String> {
    let mut argv = flags(config);
    argv.extend(connection_args(extra)?);
    argv.push("acp".to_owned());
    Ok(argv)
}

/// Flags a named connection may never pass: each would widen what the CLI does without the
/// user's PandaCode mode agreeing, or change what PandaCode itself selects.
const REFUSED_FLAGS: [&str; 12] = [
    "--force", "-f", "--yolo", "--auto-review", "--approve-mcps", "--trust", "--model", "--mode", "--plan", "--print", "-p", "--resume",
];

/// A named connection's extra arguments, or the error that names the flag it may not pass.
pub(crate) fn connection_args(args: &[String]) -> Result<Vec<String>, String> {
    for arg in args {
        let lower = arg.to_ascii_lowercase();
        if let Some(flag) = REFUSED_FLAGS.iter().find(|flag| lower == **flag || lower.starts_with(*flag)) {
            return Err(format!(
                "A Cursor CLI connection cannot pass `{flag}`: PandaCode's access mode, model and session choices decide it. Remove it from the connection."
            ));
        }
    }
    Ok(args.to_vec())
}

// ---------------------------------------------------------------------------------
// Mode and model.

#[derive(Debug, PartialEq)]
pub(crate) enum ModeAction {
    /// Send nothing; the turn goes on to the model step and the prompt.
    Continue,
    /// A Plan task whose session lists no Plan id: like `Continue`, plus the
    /// `PLAN_UNAVAILABLE` status line once before the prompt.
    ContinueWithoutPlan,
    /// One `session/set_mode` (JSON-RPC id 4).
    SetMode(Value),
}

/// `(id, display name)` of each advertised mode and the current id, from the live session
/// result only: `modes.availableModes` / `currentModeId`, else the `mode` select option.
fn advertised_modes(session: &Value) -> (Vec<(String, String)>, Option<String>) {
    if let Some(modes) = session["modes"]["availableModes"].as_array() {
        let list = modes
            .iter()
            .filter_map(|mode| Some((mode["id"].as_str()?.to_owned(), mode["name"].as_str().unwrap_or_default().to_owned())))
            .collect();
        return (list, session["modes"]["currentModeId"].as_str().map(str::to_owned));
    }
    if let Some(option) = super::acp::options(session, "mode") {
        let list = super::acp::choices(option)
            .into_iter()
            .filter_map(|choice| Some((choice["value"].as_str()?.to_owned(), choice["name"].as_str().unwrap_or_default().to_owned())))
            .collect();
        return (list, option["currentValue"].as_str().map(str::to_owned));
    }
    (Vec::new(), None)
}

fn set_mode(remote_id: &str, mode_id: &str) -> Value {
    json!({"jsonrpc":"2.0","id":4,"method":"session/set_mode","params":{"sessionId":remote_id,"modeId":mode_id}})
}

/// Decides the one mode request, if any. Never fails a turn and never selects Cursor's
/// read-only Ask. Standard, Accept edits and Full are Cursor Agent; Plan is Cursor Plan.
pub(crate) fn mode_request(session: &Value, mode: &str, remote_id: &str) -> ModeAction {
    let (modes, current) = advertised_modes(session);
    let current = current.map(|id| id.to_ascii_lowercase());
    if mode == "plan" {
        let Some((plan, _)) = modes.iter().find(|(id, _)| id.eq_ignore_ascii_case(PLAN_ID)) else {
            return ModeAction::ContinueWithoutPlan;
        };
        return if current.as_deref() == Some(PLAN_ID) { ModeAction::Continue } else { ModeAction::SetMode(set_mode(remote_id, plan)) };
    }
    let agent = modes
        .iter()
        .find(|(id, name)| !NON_AGENT_IDS.contains(&id.to_ascii_lowercase().as_str()) && (id.eq_ignore_ascii_case(AGENT_NAME) || name.eq_ignore_ascii_case(AGENT_NAME)));
    match (current, agent) {
        (Some(current), Some((agent, _))) if NON_AGENT_IDS.contains(&current.as_str()) => ModeAction::SetMode(set_mode(remote_id, agent)),
        _ => ModeAction::Continue,
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum ModelAction {
    /// Send nothing: no model was chosen, or the session already runs it.
    Applied,
    /// One `session/set_config_option` (JSON-RPC id 5) with a choice's own bracketed value.
    SetModel(Value),
    /// The CLI did not confirm the model; the current choice's name is returned for the
    /// `MODEL_NOT_CONFIRMED` status line.
    Unconfirmed(String),
}

/// `(value, name)` of each model choice and the current value, from the `model` option or
/// the legacy `models` block.
fn model_choices(session: &Value) -> (Vec<(String, String)>, String) {
    if let Some(option) = super::acp::options(session, "model") {
        let list = super::acp::choices(option)
            .into_iter()
            .filter_map(|choice| Some((choice["value"].as_str()?.to_owned(), choice["name"].as_str().unwrap_or_default().to_owned())))
            .collect();
        return (list, option["currentValue"].as_str().unwrap_or_default().to_owned());
    }
    let list = session["models"]["availableModels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| Some((model["modelId"].as_str()?.to_owned(), model["name"].as_str().unwrap_or_default().to_owned())))
        .collect();
    (list, session["models"]["currentModelId"].as_str().unwrap_or_default().to_owned())
}

/// `--model <slug>` on the spawn is the selector; this checks what the session reports.
/// Applied: the current choice's name is the slug, or the slug is that name plus an effort
/// suffix (`grok-4.7-high-fast` against `grok-4.7[…]`). SetModel: a choice named like the
/// slug exists but is not current. Otherwise Unconfirmed. Never a bare slug as a value (the
/// CLI answers `-32602 Invalid model value`).
pub(crate) fn model_request(session: &Value, model: &str, remote_id: &str) -> ModelAction {
    if model.is_empty() || model == "default" {
        return ModelAction::Applied;
    }
    let slug = model.to_ascii_lowercase();
    let (choices, current) = model_choices(session);
    let current_name = choices
        .iter()
        .find(|(value, _)| *value == current)
        .map(|(_, name)| name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| current.split('[').next().unwrap_or_default().to_owned());
    let name = current_name.to_ascii_lowercase();
    if !name.is_empty() && (slug == name || slug.starts_with(&format!("{name}-"))) {
        return ModelAction::Applied;
    }
    if let (Some(option), Some((value, _))) = (
        super::acp::options(session, "model"),
        choices.iter().find(|(_, name)| name.eq_ignore_ascii_case(model)),
    ) {
        return ModelAction::SetModel(
            json!({"jsonrpc":"2.0","id":5,"method":"session/set_config_option","params":{"sessionId":remote_id,"configId":option["id"],"value":value}}),
        );
    }
    ModelAction::Unconfirmed(current_name)
}

// ---------------------------------------------------------------------------------
// Permissions.

pub(crate) struct PlanChoice {
    pub(crate) option_id: Value,
    /// A once-reject: `acp.rs` also shows the "Plan mode declined" status line.
    pub(crate) reject: bool,
}

/// Plan only. A read-only tool kind selects the once-allow option, any other kind the
/// once-reject option. `None` when that option is absent, so the request falls through to
/// the approval card. Never `allow_always`.
pub(crate) fn plan_permission(kind: &str, options: Option<&Vec<Value>>) -> Option<PlanChoice> {
    let reject = !PLAN_READ_ONLY.contains(&kind);
    let wanted = if reject { ONCE_REJECT } else { ONCE_ALLOW };
    let option = options?.iter().find(|option| option["kind"] == wanted)?;
    Some(PlanChoice { option_id: option["optionId"].clone(), reject })
}

/// A JSON-RPC response selecting an offered option.
pub(crate) fn selected_reply(request_id: &Value, option_id: &Value) -> Value {
    json!({"jsonrpc":"2.0","id":request_id,"result":{"outcome":{"outcome":"selected","optionId":option_id}}})
}

/// The reply to a pending request when the user decides: Approve selects once-allow, Deny
/// once-reject, never an `always` option, and `cancelled` (granting nothing) when the wanted
/// option is absent. A pending plan goes to `plan_reply`.
pub(crate) fn approve_reply(request: &Value, approved: bool) -> Value {
    if request["method"] == CREATE_PLAN {
        return plan_reply(request, approved);
    }
    let wanted = if approved { ONCE_ALLOW } else { ONCE_REJECT };
    let option = request["params"]["options"].as_array().and_then(|options| options.iter().find(|option| option["kind"] == wanted));
    match option {
        Some(option) => selected_reply(&request["id"], &option["optionId"]),
        None => json!({"jsonrpc":"2.0","id":request["id"],"result":{"outcome":{"outcome":"cancelled"}}}),
    }
}

// ---------------------------------------------------------------------------------
// Questions, plans and todos.

pub(crate) struct QuestionInput {
    /// The input-request data `src/agent-input.ts` renders.
    pub(crate) data: Value,
    /// What `Protocol.inputs` keeps to validate the answer: ids, labels and `allowMultiple`.
    pub(crate) stored: Value,
}

fn bounded(value: &Value, limit: usize) -> Option<String> {
    let text = value.as_str()?.trim();
    (!text.is_empty()).then(|| text.chars().filter(|c| !c.is_control() || *c == '\n').take(limit).collect())
}

/// Normalizes a `cursor/ask_question` request (schema-derived; never recorded). One count
/// budget, the same counts as `host_inputs.rs`: 1 to 3 questions, at most 20 options each.
/// The refusal reason is short, static, and never carries frame content.
pub(crate) fn question_input(request: &Value) -> Result<QuestionInput, &'static str> {
    let params = &request["params"];
    let questions = params["questions"].as_array().ok_or("no questions")?;
    if questions.is_empty() {
        return Err("no questions");
    }
    if questions.len() > MAX_QUESTIONS {
        return Err("more than 3 questions");
    }
    let title = bounded(&params["title"], 128).unwrap_or_default();
    let (mut data, mut stored) = (Vec::new(), Vec::new());
    let mut seen_questions = HashSet::new();
    for (index, question) in questions.iter().enumerate() {
        let id = bounded(&question["id"], 256).ok_or("a question has no id")?;
        if !seen_questions.insert(id.clone()) {
            return Err("duplicate question ids");
        }
        let prompt = bounded(&question["prompt"], 4096).ok_or("a question has no prompt")?;
        let options = question["options"].as_array().ok_or("a question has no options")?;
        if options.is_empty() {
            return Err("a question has no options");
        }
        if options.len() > MAX_OPTIONS {
            return Err("more than 20 options");
        }
        let (mut labels, mut ids) = (HashSet::new(), HashSet::new());
        let mut shown = Vec::new();
        let mut kept = Vec::new();
        for option in options {
            let option_id = bounded(&option["id"], 256).ok_or("an option has no id")?;
            let label = bounded(&option["label"], 256).ok_or("an option has no label")?;
            if !labels.insert(label.clone()) {
                return Err("duplicate option labels");
            }
            if !ids.insert(option_id.clone()) {
                return Err("duplicate option ids");
            }
            shown.push(json!({"label":label}));
            kept.push(json!({"id":option_id,"label":label}));
        }
        let multiple = question["allowMultiple"] == true;
        data.push(json!({"id":id,"header":if index == 0 { title.as_str() } else { "" },"question":prompt,"options":shown,"multiSelect":multiple,"isOther":false,"isSecret":false}));
        stored.push(json!({"id":id,"allowMultiple":multiple,"options":kept}));
    }
    Ok(QuestionInput {
        data: json!({"kind":"questions","questions":data}),
        stored: json!({"id":request["id"],"method":ASK_QUESTION,"params":{"questions":stored}}),
    })
}

/// The reply to a stored question for the UI's `{answers:{<questionId>:[label…]}}`. Every
/// question must be answered with offered labels, and several only when `allowMultiple`. An
/// empty `answers` (the card's Cancel) is `cancelled`. An invalid answer is an `Err` and the
/// request stays pending.
pub(crate) fn question_reply(request: &Value, response: &Value) -> Result<Value, String> {
    let answers = response
        .as_object()
        .filter(|object| object.len() == 1)
        .and_then(|object| object.get("answers")?.as_object())
        .ok_or("Invalid answer format")?;
    if answers.is_empty() {
        return Ok(cancel_reply(request));
    }
    let questions = request["params"]["questions"].as_array().cloned().unwrap_or_default();
    if answers.len() != questions.len() {
        return Err("Answer every question before continuing".into());
    }
    let mut selected = Vec::with_capacity(questions.len());
    for (question_id, labels) in answers {
        let question = questions.iter().find(|question| question["id"] == question_id.as_str()).ok_or("Response contains an unknown question")?;
        let labels: Vec<&str> = labels
            .as_array()
            .ok_or("Invalid answer format")?
            .iter()
            .map(|label| label.as_str().ok_or("Invalid answer format"))
            .collect::<Result<_, _>>()?;
        if labels.is_empty() {
            return Err("Choose an answer for every question".into());
        }
        if labels.len() > 1 && question["allowMultiple"] != true {
            return Err("This question takes one answer".into());
        }
        let mut ids = Vec::with_capacity(labels.len());
        for label in labels {
            let option = question["options"]
                .as_array()
                .and_then(|options| options.iter().find(|option| option["label"] == label))
                .ok_or("An answer was not one of the offered choices")?;
            if !ids.contains(&option["id"]) {
                ids.push(option["id"].clone());
            }
        }
        selected.push(json!({"questionId":question_id,"selectedOptionIds":ids}));
    }
    Ok(json!({"jsonrpc":"2.0","id":request["id"],"result":{"outcome":{"outcome":"answered","answers":selected}}}))
}

/// A pending `cursor/ask_question` answered `cancelled` when the run stops. (A pending plan
/// is declined with `plan_reply(request, false)` instead.)
pub(crate) fn cancel_reply(request: &Value) -> Value {
    json!({"jsonrpc":"2.0","id":request["id"],"result":{"outcome":{"outcome":"cancelled"}}})
}

pub(crate) struct PlanCard {
    /// The approval card's question, with the plan's name when it has one.
    pub(crate) label: String,
    /// In Plan: acceptance does not change the mode, so the card says how to implement.
    pub(crate) detail: Option<String>,
    pub(crate) markdown: String,
    pub(crate) todos: Vec<Value>,
}

impl PlanCard {
    /// The single line the approval event carries.
    pub(crate) fn text(&self) -> String {
        match &self.detail {
            Some(detail) => format!("{} {detail}", self.label),
            None => self.label.clone(),
        }
    }
}

/// Normalizes a `cursor/create_plan` request (recorded in `plan-load.jsonl`).
pub(crate) fn plan_card(request: &Value, mode: &str) -> Result<PlanCard, &'static str> {
    let params = &request["params"];
    let markdown = params["plan"].as_str().map(str::trim).filter(|plan| !plan.is_empty()).ok_or("the plan is empty")?;
    let (question, detail) = if mode == "plan" {
        ("Accept this plan?", Some("To implement it, switch this task to Standard, Accept edits or Full access.".to_owned()))
    } else {
        ("Implement this plan?", None)
    };
    let label = match bounded(&params["name"], 120) {
        Some(name) => format!("{question} “{name}”"),
        None => question.into(),
    };
    let todos = params["todos"].as_array().into_iter().flatten().take(TODO_LIMIT).filter_map(clean_todo).collect();
    Ok(PlanCard { label, detail, markdown: markdown.chars().take(60_000).collect(), todos })
}

/// Accept is the body recorded in `plan-load.jsonl`; reject is the one recorded in
/// `plan-deny.jsonl` (no `reason` field). No other body was captured, so none is sent.
pub(crate) fn plan_reply(request: &Value, approved: bool) -> Value {
    json!({"jsonrpc":"2.0","id":request["id"],"result":{"outcome":{"outcome":if approved { "accepted" } else { "rejected" }}}})
}

fn clean_todo(todo: &Value) -> Option<Value> {
    let id = bounded(&todo["id"], 128)?;
    let content = bounded(&todo["content"], 1000)?;
    let status = bounded(&todo["status"], 32).unwrap_or_else(|| "pending".into());
    Some(json!({"id":id,"content":content,"status":status}))
}

/// The list after a `cursor/update_todos` request: `merge: true` merges by `id` into
/// `previous`, otherwise the request's list replaces it. Every item keeps `id`, `content`
/// and `status`.
pub(crate) fn todo_input(params: &Value, previous: &Value) -> Value {
    let incoming = params["todos"].as_array().into_iter().flatten().filter_map(clean_todo);
    let mut list: Vec<Value> = if params["merge"] == true { previous.as_array().cloned().unwrap_or_default() } else { Vec::new() };
    for todo in incoming {
        match list.iter_mut().find(|old| old["id"] == todo["id"]) {
            Some(old) => *old = todo,
            None => list.push(todo),
        }
    }
    list.truncate(TODO_LIMIT);
    Value::Array(list)
}

/// The trace's checklist input: the list without ids.
fn todo_projection(list: &Value) -> String {
    let todos: Vec<Value> = list.as_array().into_iter().flatten().map(|todo| json!({"content":todo["content"],"status":todo["status"]})).collect();
    json!({"todos":todos}).to_string()
}

fn success(id: &Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

fn failure(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

impl Protocol {
    /// Handles every `cursor/*` frame the CLI sends. Blocking methods are requests the CLI
    /// waits on; each ends in a reply, a visible refusal, or (on Stop) a cancel.
    pub(super) fn cursor_extension(&mut self, value: &Value, step: &mut Step) {
        let id = value.get("id").filter(|id| !id.is_null()).cloned();
        match field(value, "method") {
            ASK_QUESTION => self.cursor_question(value, id, step),
            CREATE_PLAN => self.cursor_plan(value, id, step),
            UPDATE_TODOS => {
                let list = todo_input(&value["params"], &self.cursor_todos);
                self.cursor_todos = list.clone();
                let tool = format!("{}-todos", limited(field(&value["params"], "toolCallId"), 200));
                self.tool_update(&tool, Some("TodoWrite"), Some("completed"), Some(todo_projection(&list)), None, step);
                if let Some(id) = id {
                    // The recorded request carried an id (the published page calls it a
                    // notification). This body is the one the recording driver sent; the CLI
                    // continued after it. It is not a CLI-defined shape.
                    step.write.push(success(&id, json!({"outcome":{"outcome":"accepted","todos":list}})));
                }
            }
            method @ (TASK | GENERATE_IMAGE) => {
                if method == TASK {
                    step.events.push(self.event("status", TASK_NOTE, None));
                }
                // Not taken from a fixture: a bare success. It starts no agent and writes no file.
                if let Some(id) = id {
                    step.write.push(success(&id, json!({})));
                }
            }
            _ => {
                if let Some(id) = id {
                    step.write.push(failure(&id, -32601, "Method not found"));
                }
            }
        }
    }

    fn refuse_question(&self, id: Option<&Value>, reason: &str, step: &mut Step) {
        if let Some(id) = id {
            step.write.push(failure(id, -32602, "Invalid params"));
        }
        step.events.push(self.event("status", format!("{QUESTION_REFUSED}{reason}"), None));
    }

    fn cursor_question(&mut self, value: &Value, id: Option<Value>, step: &mut Step) {
        let Some(rpc_id) = id.as_ref() else {
            return self.refuse_question(None, "the request had no id to answer", step);
        };
        if self.inputs.len() + self.pending.len() >= 64 {
            return self.refuse_question(id.as_ref(), "too many pending requests", step);
        }
        if self.inputs.values().any(|old| old["id"] == *rpc_id) {
            return self.refuse_question(id.as_ref(), "a request with that id is already pending", step);
        }
        match question_input(value) {
            Ok(input) => {
                let request_id = uuid::Uuid::new_v4().to_string();
                let mut event = self.event("input_request", "", None);
                event.request_id = Some(request_id.clone());
                event.data = Some(input.data);
                self.inputs.insert(request_id, input.stored);
                step.events.push(event);
            }
            Err(reason) => self.refuse_question(id.as_ref(), reason, step),
        }
    }

    fn cursor_plan(&mut self, value: &Value, id: Option<Value>, step: &mut Step) {
        let card = match plan_card(value, &self.config.mode) {
            Ok(card) => card,
            Err(reason) => {
                if let Some(id) = &id {
                    step.write.push(failure(id, -32602, "Invalid params"));
                }
                step.events.push(self.event("status", format!("{PLAN_REFUSED}{reason}"), None));
                return;
            }
        };
        let message = self.next_text_id();
        self.text_delta(&message, None, &card.markdown, step);
        let tool_call = limited(field(&value["params"], "toolCallId"), 200);
        if !card.todos.is_empty() {
            self.tool_update(&format!("{tool_call}-todos"), Some("TodoWrite"), Some("completed"), Some(todo_projection(&Value::Array(card.todos.clone()))), None, step);
        }
        let Some(id) = id else { return };
        let stored = json!({"jsonrpc":"2.0","id":id,"method":CREATE_PLAN,"toolCallId":tool_call});
        if let Err(error) = self.approval_capped(stored, card.text(), step) {
            step.write.push(failure(&id, -32602, "Invalid params"));
            step.events.push(self.event("status", format!("{PLAN_REFUSED}{error}"), None));
        }
    }

    /// The Cursor twin of `host_tools.rs` `user_input`: validates the UI's answer, removes the
    /// stored question only once it validates, and writes the reply.
    pub(super) fn cursor_user_input(&mut self, id: &str, response: Value) -> Result<Step, String> {
        let request = self.inputs.get(id).ok_or("This input request is no longer active")?;
        let reply = question_reply(request, &response)?;
        self.inputs.remove(id);
        let mut event = self.event("input_cancelled", "", None);
        event.request_id = Some(id.into());
        Ok(Step { write: vec![reply], events: vec![event], ..Step::default() })
    }
}

// ---------------------------------------------------------------------------------
// Models.

struct Captured {
    success: bool,
    stdout: String,
    stderr: String,
}

/// The `agent models` output: a header, `<id> - <label>` rows, a footer. A line is a model
/// only when the whole line has that shape, so prose can never become an id. Each slug is
/// its own model; the command reports no per-model effort.
pub(crate) fn parse_models(output: &str) -> Vec<ModelInfo> {
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for line in strip_ansi(output).lines() {
        let Some((id, label)) = line.trim().split_once(" - ") else { continue };
        let valid = id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
            && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            && valid_model_id(id);
        let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
        let label = ["(current, default)", "(current)", "(default)"].iter().fold(label, |label, marker| match label.strip_suffix(marker) {
            Some(rest) => rest.trim_end().to_owned(),
            None => label,
        });
        if !valid || label.is_empty() || !seen.insert(id.to_owned()) {
            continue;
        }
        models.push(ModelInfo {
            id: id.into(),
            name: model_text(Some(&label), id, 160),
            description: String::new(),
            reasoning_efforts: Vec::new(),
            default_reasoning_effort: None,
            context_window: None,
        });
        if models.len() >= MODEL_CATALOG_LIMIT {
            break;
        }
    }
    models
}

/// A command that failed because nobody is signed in (recorded: exit 1, empty stdout, stderr
/// naming `Authentication required` or an invalid key) is a sign-in error; every other
/// failure, a timeout and an unreadable list are `MODELS_UNRECOGNISED`. Neither clears installed.
fn classify_models(run: Result<Captured, String>) -> Result<Vec<ModelInfo>, String> {
    let output = run.map_err(|_| MODELS_UNRECOGNISED.to_owned())?;
    if !output.success {
        let stderr = strip_ansi(&output.stderr).to_ascii_lowercase();
        return Err(if SIGN_IN_MARKERS.iter().any(|marker| stderr.contains(marker)) { SIGN_IN_ERROR } else { MODELS_UNRECOGNISED }.into());
    }
    let mut models = parse_models(&output.stdout);
    if models.is_empty() {
        return Err(MODELS_UNRECOGNISED.into());
    }
    models.insert(0, ModelInfo::default_choice());
    Ok(models)
}

fn models_bound() -> Duration {
    #[cfg(test)]
    if let Ok(bound) = TEST_AUTH_DEADLINE.try_with(|bound| *bound) {
        return bound;
    }
    super::acp::DISCOVERY_STARTUP
}

/// `agent models` on the exact program discovery accepted, bounded by the discovery startup
/// bound. Cached in `acp_catalog` (5 minutes; errors are never cached).
pub(super) async fn models(path: &Path) -> Result<Vec<ModelInfo>, String> {
    let bound = models_bound();
    models_via(path, move |path| run_models(path, bound)).await
}

async fn models_via(path: &Path, run: impl FnOnce(PathBuf) -> Result<Captured, String> + Send + 'static) -> Result<Vec<ModelInfo>, String> {
    if let Some(models) = super::acp_catalog::get("cursor", path) {
        return Ok(models);
    }
    let _refresh = super::acp_catalog::refresh_lock("cursor", path).await;
    if let Some(models) = super::acp_catalog::get("cursor", path) {
        return Ok(models);
    }
    let discovery = super::acp_catalog::begin("cursor", path);
    let owned = path.to_owned();
    let output = tokio::task::spawn_blocking(move || run(owned)).await.map_err(|_| MODELS_UNRECOGNISED.to_owned());
    let models = classify_models(output.and_then(|run| run))?;
    super::acp_catalog::store_models(discovery, models.clone());
    Ok(models)
}

fn run_models(path: PathBuf, bound: Duration) -> Result<Captured, String> {
    #[cfg(test)]
    if let Some(run) = models_hook().as_ref().cloned() {
        return run(&path);
    }
    let mut command = super::launch::std_command(&path)?;
    crate::provider_settings::apply_environment("cursor", &mut command);
    command.arg("models").env(NO_OPEN_BROWSER, "1").current_dir(std::env::temp_dir());
    let output = crate::workspace::run_process(command, "Cursor CLI", 1024 * 1024, bound)?;
    Ok(Captured {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout.bytes).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr.bytes).into_owned(),
    })
}

// ---------------------------------------------------------------------------------
// Tests.

#[cfg(test)]
// The probe tests hold `probe_test_guard()` (a std mutex) for the whole test on purpose.
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    const INITIALIZE: &str = include_str!("cursor_frames/initialize.jsonl");
    const SESSION: &str = include_str!("cursor_frames/session.jsonl");
    const RESUME: &str = include_str!("cursor_frames/resume.jsonl");
    const MODEL_NEW: &str = include_str!("cursor_frames/model-new.jsonl");
    const MODEL_LOAD: &str = include_str!("cursor_frames/model-load.jsonl");
    const PLAN_LOAD: &str = include_str!("cursor_frames/plan-load.jsonl");
    const PLAN_DENY: &str = include_str!("cursor_frames/plan-deny.jsonl");
    const MCP_LOAD: &str = include_str!("cursor_frames/mcp-load.jsonl");
    const MODELS_TXT: &str = include_str!("cursor_frames/models.txt");
    const BADKEY_ACP: &str = include_str!("cursor_frames/signedout-badkey-acp.jsonl");
    const NOKEY_ACP: &str = include_str!("cursor_frames/signedout-nokey-acp.jsonl");
    const BADKEY_MODELS: &str = include_str!("cursor_frames/signedout-badkey-models.jsonl");
    const NOKEY_MODELS: &str = include_str!("cursor_frames/signedout-nokey-models.jsonl");
    const UNKNOWN_MODEL: &str = include_str!("cursor_frames/model-unknown-load.jsonl");

    fn entries(file: &str) -> Vec<Value> {
        file.lines().filter(|line| !line.trim().is_empty()).map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    /// The `in` frames of a recording.
    fn received(file: &str) -> Vec<Value> {
        entries(file).into_iter().filter(|entry| entry["dir"] == "in").map(|entry| entry["frame"].clone()).collect()
    }

    fn result_for(file: &str, id: u64) -> Value {
        received(file).into_iter().find(|frame| frame["id"] == id && frame.get("result").is_some()).unwrap()["result"].clone()
    }

    fn error_for(file: &str, id: u64) -> Value {
        received(file).into_iter().find(|frame| frame["id"] == id && frame.get("error").is_some()).unwrap()
    }

    fn requests(file: &str, method: &str) -> Vec<Value> {
        received(file).into_iter().filter(|frame| frame["method"] == method).collect()
    }

    fn stderr_of(file: &str) -> String {
        entries(file).iter().filter(|entry| entry["dir"] == "stderr").map(|entry| entry["text"].as_str().unwrap().to_owned()).collect()
    }

    fn config(mode: &str, model: &str) -> RunConfig {
        let mut config = super::super::tests::config("cursor");
        config.mode = mode.into();
        config.model = model.into();
        config
    }

    fn protocol(mode: &str) -> Protocol {
        Protocol::new(config(mode, "default"))
    }

    fn args(command: &Command) -> Vec<String> {
        command.as_std().get_args().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    // --- identity -------------------------------------------------------------------

    #[test]
    fn only_the_cursor_login_method_identifies_cursor() {
        assert!(identifies_cursor(&result_for(INITIALIZE, 1)));
        assert!(identifies_cursor(&result_for(BADKEY_ACP, 1)), "initialize does not depend on sign-in");
        assert!(!identifies_cursor(&json!({"protocolVersion":1,"authMethods":[{"id":"cached_token"}]})));
        assert!(!identifies_cursor(&json!({"protocolVersion":1})));
    }

    #[test]
    fn the_editor_launcher_is_never_a_candidate() {
        for name in ["cursor", "cursor.cmd", "Cursor.exe", "/usr/bin/CURSOR"] {
            assert!(rejected_name(Path::new(name)), "{name}");
        }
        for name in ["cursor-agent", "agent", "agent.cmd", "cursor-agent.cmd", "/opt/cursor/agent"] {
            assert!(!rejected_name(Path::new(name)), "{name}");
        }
    }

    fn accepting() -> impl Fn(&Path) -> Result<Value, String> + Send + Sync + 'static {
        |_| Ok(result_for(INITIALIZE, 1))
    }

    #[tokio::test]
    async fn a_candidate_is_accepted_only_after_a_handshake_that_identifies_cursor() {
        let _guard = probe_test_guard();
        // The path does not contain `cursor-agent`: the handshake alone decides.
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        set_probe_for_test(vec![PathBuf::from("/bin/cursor"), PathBuf::from("/opt/tool/agent")], move |path| {
            seen.fetch_add(1, Ordering::SeqCst);
            assert_ne!(path, Path::new("/bin/cursor"), "a `cursor` stem is skipped before any spawn");
            Ok(result_for(INITIALIZE, 1))
        });
        assert_eq!(cached_installed(), None, "no verdict yet");
        assert_eq!(accepted_executable().await, Some(PathBuf::from("/opt/tool/agent")));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(cached_installed(), Some(PathBuf::from("/opt/tool/agent")), "the PATH entry, not a canonical target");

        set_probe_for_test(vec![PathBuf::from("/opt/tool/agent")], |_| Ok(json!({"protocolVersion":1,"authMethods":[{"id":"other"}]})));
        assert_eq!(accepted_executable().await, None, "a handshake that is not Cursor");
    }

    #[tokio::test]
    async fn a_failed_agent_handshake_still_tries_cursor_agent() {
        let _guard = probe_test_guard();
        set_probe_for_test(vec![PathBuf::from("/a/agent"), PathBuf::from("/b/cursor-agent")], |path| {
            if path == Path::new("/a/agent") { Err("not an ACP server".into()) } else { Ok(result_for(INITIALIZE, 1)) }
        });
        assert_eq!(accepted_executable().await, Some(PathBuf::from("/b/cursor-agent")));
    }

    #[tokio::test]
    async fn concurrent_callers_share_one_probe_and_a_cold_start_waits_for_it() {
        let _guard = probe_test_guard();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        set_probe_for_test(vec![PathBuf::from("/x/agent")], move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(50));
            Ok(result_for(INITIALIZE, 1))
        });
        let (first, second) = tokio::join!(accepted_executable(), accepted_executable());
        assert_eq!((first, second), (Some("/x/agent".into()), Some("/x/agent".into())));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(accepted_executable().await, Some("/x/agent".into()));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "a positive verdict is not probed again");
    }

    #[tokio::test]
    async fn a_cached_negative_verdict_is_reprobed_by_runs_but_not_by_display_reads() {
        let _guard = probe_test_guard();
        let installed = Arc::new(AtomicBool::new(false));
        let flag = installed.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        set_probe_for_test(vec![PathBuf::from("/x/agent")], move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            if flag.load(Ordering::SeqCst) { Ok(result_for(INITIALIZE, 1)) } else { Err("missing".into()) }
        });
        assert_eq!(accepted_executable().await, None);
        assert_eq!(cached_installed(), None);
        ensure_probe();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "ensure_probe and display reads leave a negative verdict alone");
        installed.store(true, Ordering::SeqCst);
        assert_eq!(accepted_executable().await, Some("/x/agent".into()), "installed after launch");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "exactly one fresh probe");
    }

    #[tokio::test]
    async fn refresh_keeps_the_old_verdict_until_the_new_one_lands_and_settles_only_on_change() {
        let _guard = probe_test_guard();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        set_probe_for_test(vec![PathBuf::from("/x/agent")], move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(80));
            Ok(result_for(INITIALIZE, 1))
        });
        let settled = Arc::new(AtomicUsize::new(0));
        let counter = settled.clone();
        on_settled(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(accepted_executable().await, Some("/x/agent".into()));
        assert_eq!(settled.load(Ordering::SeqCst), 1, "unset to installed");
        let pending = tokio::spawn(refresh());
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(cached_installed(), Some("/x/agent".into()), "readable while the probe runs");
        assert_eq!(pending.await.unwrap(), Some("/x/agent".into()));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(settled.load(Ordering::SeqCst), 1, "the same verdict is not a change");
    }

    #[tokio::test]
    async fn a_forced_refresh_supersedes_a_probe_that_started_before_the_call() {
        let _guard = probe_test_guard();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        // The first (background) probe is slow and does not identify Cursor; the probe the
        // forced list starts is quick and does. Joining the first would report "not installed".
        set_probe_for_test(vec![PathBuf::from("/x/agent")], move |_| {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                std::thread::sleep(Duration::from_millis(120));
                Ok(json!({"protocolVersion":1}))
            } else {
                Ok(result_for(INITIALIZE, 1))
            }
        });
        let stale = tokio::spawn(accepted_executable());
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(refresh().await, Some("/x/agent".into()), "the forced list returns the new probe's verdict");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "it starts its own probe instead of joining");
        assert_eq!(stale.await.unwrap(), None, "the superseded probe keeps its own verdict");
        assert_eq!(cached_installed(), Some("/x/agent".into()), "and stores nothing");
    }

    #[tokio::test]
    async fn invalidating_drops_the_verdict_and_the_accepted_paths() {
        let _guard = probe_test_guard();
        set_probe_for_test(vec![PathBuf::from("/x/agent")], accepting());
        assert!(accepted_executable().await.is_some());
        assert!(accept_path(Path::new("/y/other"), &[]).await.is_ok());
        assert!(state().accepted.contains(Path::new("/y/other")));
        invalidate_handshake();
        assert!(state().accepted.is_empty());
        assert!(state().verdict.is_none());
        assert_eq!(accepted_executable().await, Some("/x/agent".into()));
    }

    #[tokio::test]
    async fn a_connection_binary_is_used_only_after_its_handshake() {
        let _guard = probe_test_guard();
        set_probe_for_test(vec![], |path| {
            if path == Path::new("/c/agent") { Ok(result_for(INITIALIZE, 1)) } else { Ok(json!({"protocolVersion":1})) }
        });
        assert!(accept_path(Path::new("/c/agent"), &[]).await.is_ok());
        assert!(accept_path(Path::new("/c/other"), &[]).await.is_err(), "not Cursor");
        assert!(accept_path(Path::new("/c/cursor.cmd"), &[]).await.is_err(), "the editor launcher, however supplied");
        assert!(!state().accepted.contains(Path::new("/c/other")), "a failure caches nothing");
        assert_eq!(cached_installed(), None, "the default is not replaced");
    }

    #[test]
    fn the_version_is_read_once_per_verdict() {
        let _guard = probe_test_guard();
        set_probe_for_test(vec![], accepting());
        let reads = AtomicUsize::new(0);
        let read = |_: &Path| {
            reads.fetch_add(1, Ordering::SeqCst);
            Some("2026.09.15".to_owned())
        };
        assert_eq!(version(Path::new("/x/agent"), read), "2026.09.15");
        assert_eq!(version(Path::new("/x/agent"), read), "2026.09.15");
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        state().version = None;
        assert_eq!(version(Path::new("/x/agent"), read), "2026.09.15");
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        // An empty `--version` read is kept too, so a failed read is not spawned every list.
        let empty_reads = AtomicUsize::new(0);
        let empty = |_: &Path| {
            empty_reads.fetch_add(1, Ordering::SeqCst);
            None
        };
        state().version = None;
        assert_eq!(version(Path::new("/x/agent"), empty), "");
        assert_eq!(version(Path::new("/x/agent"), empty), "");
        assert_eq!(empty_reads.load(Ordering::SeqCst), 1);
    }

    // --- bounds and the authenticate watch ------------------------------------------

    #[test]
    fn both_bounds_are_the_discovery_startup_bound() {
        assert_eq!(AUTH_DEADLINE, super::super::acp::DISCOVERY_STARTUP);
        assert_eq!(AUTH_DEADLINE, Duration::from_secs(60));
        assert_eq!(auth_deadline(), AUTH_DEADLINE);
        assert_eq!(models_bound(), AUTH_DEADLINE);
    }

    #[tokio::test]
    async fn the_deadline_override_shortens_both_bounds() {
        TEST_AUTH_DEADLINE
            .scope(Duration::from_millis(5), async {
                assert_eq!(auth_deadline(), Duration::from_millis(5));
                assert_eq!(models_bound(), Duration::from_millis(5));
            })
            .await;
    }

    fn authenticate() -> Value {
        json!({"jsonrpc":"2.0","id":7,"method":"authenticate","params":{"methodId":LOGIN_METHOD}})
    }

    #[test]
    fn only_a_written_authenticate_arms_and_only_its_response_disarms() {
        let now = tokio::time::Instant::now();
        let mut watch = AuthWatch::for_provider("cursor");
        watch.written(&[json!({"jsonrpc":"2.0","id":2,"method":"session/new"})], now);
        assert!(!watch.armed());
        watch.written(&[authenticate()], now);
        assert_eq!(watch.deadline(), Some(now + AUTH_DEADLINE));
        // The CLI numbers its own requests from 0, so a request with id 7 is not the answer.
        watch.received(&json!({"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{}}));
        assert!(watch.armed());
        watch.received(&json!({"jsonrpc":"2.0","method":"session/update","params":{}}));
        watch.received(&json!({"jsonrpc":"2.0","id":2,"result":{}}));
        assert!(watch.armed());
        for answer in [received(include_str!("cursor_frames/authenticate.jsonl")).pop().unwrap(), error_for(BADKEY_ACP, 7)] {
            let mut watch = AuthWatch::for_provider("cursor");
            watch.written(&[authenticate()], now);
            watch.received(&answer);
            assert!(!watch.armed(), "{answer}");
        }
        let mut other = AuthWatch::for_provider("grok");
        other.written(&[authenticate()], now);
        assert!(!other.armed(), "only a Cursor run is watched");
    }

    #[test]
    fn a_child_exit_while_armed_is_a_model_refusal_or_a_sign_in_error() {
        assert_eq!(auth_exit(&stderr_of(UNKNOWN_MODEL)), model_rejected("not-a-real-model-xyz"));
        assert!(model_rejected("not-a-real-model-xyz").contains("not-a-real-model-xyz"));
        assert_eq!(auth_exit(""), SIGN_IN_ERROR);
        assert_eq!(auth_exit("\u{1b}[31mboom\u{1b}[0m"), SIGN_IN_ERROR);
        assert!(!auth_exit(&stderr_of(UNKNOWN_MODEL)).contains("Available models"), "the account's model list is not shown");
        assert!(is_own_error(SIGN_IN_ERROR) && is_own_error(&model_rejected("x")) && !is_own_error("Agent exited"));
        assert!(is_sign_in_error(SIGN_IN_ERROR), "a signed-out run is not a model failure");
        assert!(!is_sign_in_error(&model_rejected("x")) && !is_sign_in_error("Agent exited"));
    }

    #[test]
    fn a_signed_out_authenticate_error_is_recorded_with_a_url_the_sign_in_error_never_shows() {
        for file in [BADKEY_ACP, NOKEY_ACP] {
            let error = error_for(file, 7);
            assert_eq!(error["error"]["code"], -32602, "recorded; the earlier -32000 guess is dropped");
            assert!(error["error"]["data"]["message"].as_str().unwrap().contains("<login-url>"));
        }
        assert!(!SIGN_IN_ERROR.contains("http") && !SIGN_IN_ERROR.contains("<login-url>"));
    }

    // --- launch ---------------------------------------------------------------------

    #[test]
    fn configure_adds_only_the_model_and_plan_flags_and_never_acp() {
        let run = |mode: &str, model: &str, resume: Option<&str>| {
            let mut command = Command::new("agent");
            let mut config = config(mode, model);
            config.resume_id = resume.map(str::to_owned);
            configure(&mut command, &config);
            command
        };
        assert!(args(&run("ask", "default", None)).is_empty());
        assert_eq!(args(&run("ask", "composer-2.5", None)), ["--model", "composer-2.5"]);
        assert_eq!(args(&run("plan", "default", None)), ["--mode=plan"]);
        assert_eq!(args(&run("plan", "grok-4.7-high-fast", Some("11111111-1111-4111-8111-111111111111"))), ["--model", "grok-4.7-high-fast", "--mode=plan"], "a resume uses the same arguments");
        for mode in ["ask", "auto", "full", "plan"] {
            let all = args(&run(mode, "composer-2.5", Some("id"))).join(" ");
            for forbidden in ["acp", "--mode=ask", "--force", "--yolo", "--auto-review", "--approve-mcps", "--trust", "--resume", "-p"] {
                assert!(!all.split(' ').any(|arg| arg == forbidden), "{mode}: {forbidden}");
            }
        }
        let command = run("ask", "default", None);
        assert!(command.as_std().get_envs().any(|(name, value)| name == NO_OPEN_BROWSER && value.is_some_and(|value| value == "1")));
        assert!(!command.as_std().get_envs().any(|(name, _)| name == "ACP_BACKEND"), "that strip is Devin-only");
    }

    #[test]
    fn a_run_puts_the_model_then_plan_then_connection_arguments_then_acp() {
        assert_eq!(spawn_arguments(&config("ask", "default"), &[]).unwrap(), ["acp"]);
        assert_eq!(spawn_arguments(&config("plan", "composer-2.5"), &[]).unwrap(), ["--model", "composer-2.5", PLAN_FLAG, "acp"]);
        let extra = vec!["--sandbox".to_owned(), "enabled".to_owned()];
        assert_eq!(spawn_arguments(&config("ask", "default"), &extra).unwrap(), ["--sandbox", "enabled", "acp"], "a passed argument appears once, before `acp`");
        let argv = spawn_arguments(&config("plan", "grok-4.7-high-fast"), &extra).unwrap();
        assert_eq!(argv.iter().filter(|arg| *arg == "acp").count(), 1, "never twice");
        assert_eq!(argv.last().map(String::as_str), Some("acp"), "`acp` is the last token");
        assert!(spawn_arguments(&config("ask", "default"), &["--force".into()]).is_err(), "the refusal reaches the spawn");
        assert!(spawn_arguments(&config("ask", "default"), &["--mode=ask".into()]).is_err());
    }

    #[test]
    fn the_handshake_command_carries_the_connections_environment_and_acp_last() {
        let env = vec![("PATH".to_owned(), "/opt/cursor/bin".to_owned()), ("CURSOR_CONFIG_HOME".to_owned(), "/tmp/cursor-home".to_owned())];
        let command = handshake_command(Path::new("/opt/cursor/agent"), &env).unwrap();
        let vars: Vec<_> = command
            .as_std()
            .get_envs()
            .filter_map(|(name, value)| Some((name.to_string_lossy().into_owned(), value?.to_string_lossy().into_owned())))
            .collect();
        assert!(vars.contains(&("PATH".to_owned(), "/opt/cursor/bin".to_owned())), "{vars:?}");
        assert!(vars.contains(&("CURSOR_CONFIG_HOME".to_owned(), "/tmp/cursor-home".to_owned())), "{vars:?}");
        assert!(vars.iter().any(|(name, value)| name == NO_OPEN_BROWSER && value == "1"));
        assert_eq!(args(&command), ["acp"]);
    }

    #[test]
    fn a_connection_cannot_widen_access_or_override_pandacodes_choices() {
        for flag in ["--force", "-f", "--yolo", "--auto-review", "--approve-mcps", "--trust", "--mode=ask", "--mode", "--plan", "--model", "--model=composer-2.5", "--print", "-p", "--resume", "--resume=abc", "--YOLO"] {
            let error = connection_args(&["--sandbox".into(), flag.into()]).unwrap_err();
            assert!(error.contains(&format!("`{}`", REFUSED_FLAGS.iter().find(|f| flag.to_ascii_lowercase().starts_with(*f)).unwrap())), "{flag}: {error}");
        }
        let kept = vec!["--sandbox".to_owned(), "enabled".to_owned(), "--workspace".to_owned(), "/tmp/work".to_owned()];
        assert_eq!(connection_args(&kept).unwrap(), kept, "other arguments pass unchanged, in order");
        assert!(connection_args(&[]).unwrap().is_empty());
    }

    // --- the mode rule ---------------------------------------------------------------

    fn session_with(current: &str, available: &[(&str, &str)]) -> Value {
        let modes: Vec<Value> = available.iter().map(|(id, name)| json!({"id":id,"name":name})).collect();
        json!({"modes":{"currentModeId":current,"availableModes":modes}})
    }

    const SID: &str = "11111111-1111-4111-8111-111111111111";

    #[test]
    fn plan_selects_the_advertised_plan_id_with_client_id_four_as_recorded() {
        let loaded = result_for(PLAN_LOAD, 2);
        assert_eq!(loaded["modes"]["currentModeId"], "agent", "--mode=plan left the loaded chat in Agent");
        let recorded = entries(PLAN_LOAD).into_iter().find(|e| e["dir"] == "out" && e["frame"]["method"] == "session/set_mode").unwrap()["frame"].clone();
        let ModeAction::SetMode(request) = mode_request(&loaded, "plan", SID) else { panic!("expected SetMode") };
        assert_eq!(request["id"], 4, "the driver numbered it 5; PandaCode's id 5 is the model step");
        assert_eq!(request["method"], recorded["method"]);
        assert_eq!(request["params"], recorded["params"]);
        // A session already in Plan sends nothing (plan-deny.jsonl loaded such a chat).
        assert_eq!(result_for(PLAN_DENY, 2)["modes"]["currentModeId"], "plan");
        assert_eq!(mode_request(&result_for(PLAN_DENY, 2), "plan", SID), ModeAction::Continue);
    }

    #[test]
    fn plan_without_a_plan_id_continues_and_says_so() {
        for session in [session_with("agent", &[("agent", "Agent"), ("ask", "Ask")]), json!({}), session_with("agent", &[])] {
            assert_eq!(mode_request(&session, "plan", SID), ModeAction::ContinueWithoutPlan);
        }
        // The mode option alone is read too.
        let option = json!({"configOptions":[{"id":"mode","type":"select","currentValue":"agent","options":[{"value":"agent","name":"Agent"},{"value":"plan","name":"Plan"}]}]});
        assert!(matches!(mode_request(&option, "plan", SID), ModeAction::SetMode(request) if request["params"]["modeId"] == "plan"));
    }

    #[test]
    fn standard_accept_edits_and_full_are_agent_and_never_select_ask() {
        let recorded = result_for(SESSION, 2);
        for mode in ["ask", "auto", "full"] {
            assert_eq!(mode_request(&recorded, mode, SID), ModeAction::Continue, "{mode}: current agent sends nothing");
            // A resumed chat left in ask, plan or debug goes back to the Agent id.
            for current in ["ask", "plan", "debug"] {
                let session = session_with(current, &[("agent", "Agent"), ("plan", "Plan"), ("ask", "Ask")]);
                let ModeAction::SetMode(request) = mode_request(&session, mode, SID) else { panic!("{mode} {current}") };
                assert_eq!(request["params"], json!({"sessionId":SID,"modeId":"agent"}));
                assert_eq!(request["id"], 4);
            }
            // The Agent id is found by its display name when its id is something else.
            let named = session_with("ask", &[("code", "Agent"), ("ask", "Ask")]);
            assert!(matches!(mode_request(&named, mode, SID), ModeAction::SetMode(r) if r["params"]["modeId"] == "code"));
            // Only ask and plan advertised: leave the CLI on its default, never select ask.
            assert_eq!(mode_request(&session_with("ask", &[("ask", "Ask"), ("plan", "Plan")]), mode, SID), ModeAction::Continue);
            assert_eq!(mode_request(&session_with("custom", &[("agent", "Agent"), ("custom", "Custom")]), mode, SID), ModeAction::Continue, "an unknown id");
            assert_eq!(mode_request(&json!({"modes":{"availableModes":[{"id":"agent","name":"Agent"}]}}), mode, SID), ModeAction::Continue, "no current mode");
            assert_eq!(mode_request(&json!({}), mode, SID), ModeAction::Continue, "no modes");
        }
    }

    // --- the model step -----------------------------------------------------------------

    #[test]
    fn a_chosen_model_is_applied_when_the_session_reports_its_name_or_effort_family() {
        let loaded = result_for(MODEL_LOAD, 2);
        assert_eq!(model_request(&loaded, "composer-2.5", SID), ModelAction::Applied, "choice name equals the slug");
        let denied = result_for(PLAN_DENY, 2);
        assert!(denied["models"]["currentModelId"].as_str().unwrap().starts_with("grok-4.7["));
        assert_eq!(model_request(&denied, "grok-4.7-high-fast", SID), ModelAction::Applied, "effort family, recorded on session/load");
        let created = result_for(MODEL_NEW, 2);
        assert_eq!(model_request(&created, "grok-4.7-high-fast", SID), ModelAction::Applied, "and on session/new");
        assert_eq!(model_request(&created, "default", SID), ModelAction::Applied);
        assert_eq!(model_request(&created, "", SID), ModelAction::Applied);
        let auto = json!({"models":{"currentModelId":"default[]","availableModels":[{"modelId":"default[]","name":"Auto"}]}});
        assert_eq!(model_request(&auto, "auto", SID), ModelAction::Applied);
    }

    #[test]
    fn a_listed_but_not_current_model_is_set_with_its_bracketed_value_as_recorded() {
        let created = result_for(MODEL_NEW, 2);
        let recorded = entries(MODEL_NEW).into_iter().find(|e| e["dir"] == "out" && e["frame"]["method"] == "session/set_config_option").unwrap()["frame"].clone();
        let ModelAction::SetModel(request) = model_request(&created, "composer-2.5", "22222222-2222-4222-8222-222222222222") else { panic!() };
        assert_eq!(request["id"], 5);
        assert_eq!(request["params"], recorded["params"], "{{configId: model, value: composer-2.5[fast=true]}}");
        assert_eq!(request["params"]["value"], "composer-2.5[fast=true]");
    }

    #[test]
    fn an_unconfirmed_model_is_reported_and_a_bare_slug_is_never_sent() {
        let created = result_for(MODEL_NEW, 2);
        assert_eq!(model_request(&created, "gpt-5.3-codex-high", SID), ModelAction::Unconfirmed("grok-4.7".into()));
        assert_eq!(error_for(MODEL_NEW, 6)["error"]["data"]["message"], "Invalid model value: gpt-5.3-codex-high", "what a bare slug gets");
        assert_eq!(model_request(&json!({"models":{"currentModelId":"a[]","availableModels":[{"modelId":"a[]","name":"a"}]}}), "b", SID), ModelAction::Unconfirmed("a".into()), "no model option to set");
        assert!(model_not_confirmed("gpt-5.3-codex-high", "grok-4.7").contains("gpt-5.3-codex-high"));
    }

    // --- permissions -----------------------------------------------------------------

    fn permission() -> Value {
        requests(RESUME, "session/request_permission").remove(0)
    }

    #[test]
    fn the_once_pair_is_the_recorded_kinds_and_replies_carry_the_offered_option_id() {
        assert_eq!((ONCE_ALLOW, ONCE_REJECT), ("allow_once", "reject_once"));
        let request = permission();
        assert_eq!(request["id"], 0, "the CLI numbers its own requests from 0");
        let kinds: Vec<_> = request["params"]["options"].as_array().unwrap().iter().map(|o| (o["optionId"].as_str().unwrap(), o["kind"].as_str().unwrap())).collect();
        assert_eq!(kinds, [("allow-once", "allow_once"), ("allow-always", "allow_always"), ("reject-once", "reject_once")]);
        let allow = approve_reply(&request, true);
        let reject = approve_reply(&request, false);
        assert_eq!(allow, json!({"jsonrpc":"2.0","id":0,"result":{"outcome":{"outcome":"selected","optionId":"allow-once"}}}));
        assert_eq!(reject, json!({"jsonrpc":"2.0","id":0,"result":{"outcome":{"outcome":"selected","optionId":"reject-once"}}}));
        for reply in [&allow, &reject] {
            assert!(!reply.to_string().contains("always"));
        }
    }

    #[test]
    fn an_absent_once_option_grants_nothing() {
        let only_always = json!({"id":3,"method":"session/request_permission","params":{"options":[{"optionId":"a","kind":"allow_always"},{"optionId":"r","kind":"reject_always"}]}});
        for approved in [true, false] {
            assert_eq!(approve_reply(&only_always, approved), json!({"jsonrpc":"2.0","id":3,"result":{"outcome":{"outcome":"cancelled"}}}));
        }
    }

    #[test]
    fn plan_allows_read_only_kinds_declines_the_rest_and_falls_through_when_the_option_is_missing() {
        let options = permission()["params"]["options"].as_array().unwrap().clone();
        for kind in PLAN_READ_ONLY {
            let choice = plan_permission(kind, Some(&options)).unwrap();
            assert_eq!((choice.option_id, choice.reject), (json!("allow-once"), false), "{kind}");
        }
        for kind in ["edit", "execute", "delete", "move", "other", ""] {
            let choice = plan_permission(kind, Some(&options)).unwrap();
            assert_eq!((choice.option_id, choice.reject), (json!("reject-once"), true), "{kind}");
        }
        let without_reject: Vec<Value> = options.iter().filter(|o| o["kind"] != "reject_once").cloned().collect();
        assert!(plan_permission("execute", Some(&without_reject)).is_none());
        assert!(plan_permission("read", Some(&vec![])).is_none());
        assert!(plan_permission("read", None).is_none());
        let always = vec![json!({"optionId":"x","kind":"allow_always"})];
        assert!(plan_permission("read", Some(&always)).is_none(), "never allow_always");
    }

    #[test]
    fn approving_through_protocol_never_writes_always_or_a_claude_response() {
        let mut p = protocol("ask");
        p.pending.insert("req".into(), permission());
        let reply = p.approve("req", true).unwrap();
        assert_eq!(reply["result"]["outcome"]["optionId"], "allow-once");
        assert!(reply.get("type").is_none(), "not a control_response");
        assert!(p.pending.is_empty());
    }

    // --- questions (schema-derived, never recorded) -----------------------------------
    //
    // `cursor/ask_question` was never raised by the CLI in Plan or Agent mode across two
    // recording attempts (cursor_frames/NOTES.md). Every frame below is built from the
    // published schema in cursor_frames/extension-schemas.md and is NOT a recording.

    fn ask(questions: Value) -> Value {
        json!({"jsonrpc":"2.0","id":4,"method":ASK_QUESTION,"params":{"toolCallId":"call-1","title":"Greeting","questions":questions}})
    }

    fn one_question() -> Value {
        ask(json!([{"id":"greeting","prompt":"Which word?","options":[{"id":"hi","label":"Hi"},{"id":"hello","label":"Hello"}]}]))
    }

    #[test]
    fn a_valid_question_becomes_the_existing_question_card() {
        let mut p = protocol("ask");
        let mut step = Step::default();
        p.cursor_extension(&one_question(), &mut step);
        assert!(step.write.is_empty());
        let event = &step.events[0];
        assert_eq!(event.kind, "input_request");
        let id = event.request_id.clone().unwrap();
        assert_eq!(
            event.data.as_ref().unwrap(),
            &json!({"kind":"questions","questions":[{"id":"greeting","header":"Greeting","question":"Which word?","options":[{"label":"Hi"},{"label":"Hello"}],"multiSelect":false,"isOther":false,"isSecret":false}]})
        );
        assert_eq!(p.inputs.len(), 1);
        // Answering by label writes a JSON-RPC 2.0 response with the option ids.
        let step = p.cursor_user_input(&id, json!({"answers":{"greeting":["Hello"]}})).unwrap();
        assert_eq!(step.write, vec![json!({"jsonrpc":"2.0","id":4,"result":{"outcome":{"outcome":"answered","answers":[{"questionId":"greeting","selectedOptionIds":["hello"]}]}}})]);
        assert_eq!(step.events[0].kind, "input_cancelled");
        assert!(p.inputs.is_empty());
    }

    #[test]
    fn allow_multiple_and_cancel_follow_the_schema() {
        let mut p = protocol("ask");
        let mut step = Step::default();
        p.cursor_extension(&ask(json!([{"id":"q","prompt":"Pick","allowMultiple":true,"options":[{"id":"a","label":"A"},{"id":"b","label":"B"}]}])), &mut step);
        assert_eq!(step.events[0].data.as_ref().unwrap()["questions"][0]["multiSelect"], true);
        let id = step.events[0].request_id.clone().unwrap();
        let reply = p.cursor_user_input(&id, json!({"answers":{"q":["A","B"]}})).unwrap();
        assert_eq!(reply.write[0]["result"]["outcome"]["answers"][0]["selectedOptionIds"], json!(["a","b"]));
        let mut step = Step::default();
        p.cursor_extension(&one_question(), &mut step);
        let id = step.events[0].request_id.clone().unwrap();
        let cancelled = p.cursor_user_input(&id, json!({"answers":{}})).unwrap();
        assert_eq!(cancelled.write, vec![json!({"jsonrpc":"2.0","id":4,"result":{"outcome":{"outcome":"cancelled"}}})]);
    }

    #[test]
    fn an_answer_the_card_cannot_collect_is_refused_and_the_question_stays_pending() {
        let mut p = protocol("ask");
        let mut step = Step::default();
        p.cursor_extension(&one_question(), &mut step);
        let id = step.events[0].request_id.clone().unwrap();
        for bad in [
            json!({"answers":{"greeting":["Unlisted"]}}),
            json!({"answers":{"nope":["Hi"]}}),
            json!({"answers":{"greeting":["Hi","Hello"]}}),
            json!({"answers":{"greeting":[]}}),
            json!({"answers":{"greeting":[3]}}),
            json!({"answers":"x"}),
            json!({"other":1}),
            json!({"answers":{"greeting":["Hi"]},"extra":1}),
        ] {
            assert!(p.cursor_user_input(&id, bad.clone()).is_err(), "{bad}");
            assert_eq!(p.inputs.len(), 1, "{bad}");
        }
        assert!(p.cursor_user_input("unknown", json!({"answers":{}})).is_err());
        // Every question must be answered.
        let two = ask(json!([{"id":"a","prompt":"A","options":[{"id":"1","label":"One"}]},{"id":"b","prompt":"B","options":[{"id":"2","label":"Two"}]}]));
        let mut two = two;
        two["id"] = json!(5);
        let mut step = Step::default();
        p.cursor_extension(&two, &mut step);
        let id = step.events[0].request_id.clone().unwrap();
        assert!(p.cursor_user_input(&id, json!({"answers":{"a":["One"]}})).is_err());
        assert!(p.inputs.contains_key(&id));
    }

    #[test]
    fn cursor_malformed_question_fails_visibly() {
        let option = |id: &str, label: &str| json!({"id":id,"label":label});
        let many: Vec<Value> = (0..21).map(|n| option(&format!("o{n}"), &format!("L{n}"))).collect();
        let cases = [
            ("no questions field", json!({"jsonrpc":"2.0","id":4,"method":ASK_QUESTION,"params":{"toolCallId":"c"}})),
            ("empty questions", ask(json!([]))),
            ("four questions", ask(json!((0..4).map(|n| json!({"id":format!("q{n}"),"prompt":"P","options":[option("a","A")]})).collect::<Vec<_>>()))),
            ("no options", ask(json!([{"id":"q","prompt":"P"}]))),
            ("empty options", ask(json!([{"id":"q","prompt":"P","options":[]}]))),
            ("21 options", ask(json!([{"id":"q","prompt":"P","options":many}]))),
            ("question without id", ask(json!([{"prompt":"P","options":[option("a","A")]}]))),
            ("question without prompt", ask(json!([{"id":"q","options":[option("a","A")]}]))),
            ("option without id", ask(json!([{"id":"q","prompt":"P","options":[{"label":"A"}]}]))),
            ("empty label", ask(json!([{"id":"q","prompt":"P","options":[option("a","  ")]}]))),
            ("duplicate labels", ask(json!([{"id":"q","prompt":"P","options":[option("a","Same"),option("b","Same")]}]))),
            ("duplicate question ids", ask(json!([{"id":"q","prompt":"P","options":[option("a","A")]},{"id":"q","prompt":"P","options":[option("a","A")]}]))),
        ];
        for (name, request) in cases {
            let mut p = protocol("ask");
            let mut step = Step::default();
            p.cursor_extension(&request, &mut step);
            assert_eq!(step.write.len(), 1, "{name}: exactly one written frame");
            let frame = &step.write[0];
            assert_eq!((frame["jsonrpc"].clone(), frame["id"].clone(), frame["error"]["code"].clone()), (json!("2.0"), json!(4), json!(-32602)), "{name}");
            let statuses: Vec<_> = step.events.iter().filter(|e| e.kind == "status").collect();
            assert_eq!(statuses.len(), 1, "{name}: exactly one status event");
            assert!(statuses[0].text.starts_with(QUESTION_REFUSED), "{name}");
            let leaked = ["Which", "greeting", "Same", "L1", "call", "Greeting"];
            assert!(leaked.iter().all(|word| !statuses[0].text.contains(word)), "{name}: {}", statuses[0].text);
            assert!(step.events.iter().all(|e| e.kind != "input_request"), "{name}");
            assert!(p.inputs.is_empty(), "{name}: nothing left pending");
        }
        // A request with no id cannot be answered: no frame, still visible.
        let mut p = protocol("ask");
        let mut step = Step::default();
        let mut request = one_question();
        request.as_object_mut().unwrap().remove("id");
        p.cursor_extension(&request, &mut step);
        assert!(step.write.is_empty() && step.events.len() == 1 && step.events[0].text.starts_with(QUESTION_REFUSED));
    }

    #[test]
    fn stopping_answers_a_pending_question_cancelled() {
        let mut p = protocol("ask");
        let mut step = Step::default();
        p.cursor_extension(&one_question(), &mut step);
        let request = p.inputs.values().next().unwrap().clone();
        assert_eq!(cancel_reply(&request), json!({"jsonrpc":"2.0","id":4,"result":{"outcome":{"outcome":"cancelled"}}}));
    }

    // --- plans and todos (recorded) -----------------------------------------------------

    #[test]
    fn a_recorded_plan_shows_its_markdown_and_checklist_and_raises_one_approval() {
        let request = requests(PLAN_LOAD, "cursor/create_plan").remove(0);
        for (mode, label, detail) in [("plan", "Accept this plan?", true), ("ask", "Implement this plan?", false), ("full", "Implement this plan?", false)] {
            let mut p = protocol(mode);
            let mut step = Step::default();
            p.cursor_extension(&request, &mut step);
            assert!(step.write.is_empty(), "{mode}");
            let kinds: Vec<_> = step.events.iter().map(|e| e.kind.as_str()).collect();
            assert!(kinds.contains(&"text") && kinds.contains(&"tool") && kinds.last() == Some(&"approval"), "{kinds:?}");
            let text: String = step.events.iter().filter(|e| e.kind == "text").map(|e| e.text.clone()).collect();
            assert!(text.contains(request["params"]["plan"].as_str().unwrap().trim()), "{mode}");
            let tool = step.events.iter().find_map(|e| e.tool.as_ref()).unwrap();
            assert_eq!(tool.name, "TodoWrite");
            let input: Value = serde_json::from_str(&tool.input).unwrap();
            assert!(input["todos"][0].get("id").is_none() && input["todos"][0]["content"].is_string(), "the trace needs only content and status");
            let approval = step.events.last().unwrap();
            assert!(approval.text.starts_with(label), "{mode}: {}", approval.text);
            assert_eq!(approval.text.contains("switch this task to Standard, Accept edits or Full access"), detail, "{mode}");
            assert_eq!(p.pending.len(), 1);
        }
    }

    #[test]
    fn plan_approve_and_deny_send_the_recorded_bodies_with_no_reason() {
        let request = requests(PLAN_LOAD, "cursor/create_plan").remove(0);
        let mut p = protocol("plan");
        let mut step = Step::default();
        p.cursor_extension(&request, &mut step);
        let approval = step.events.last().unwrap().request_id.clone().unwrap();
        let accepted = p.approve(&approval, true).unwrap();
        assert_eq!(accepted, json!({"jsonrpc":"2.0","id":request["id"],"result":{"outcome":{"outcome":"accepted"}}}));
        let recorded_accept = entries(PLAN_LOAD).into_iter().find(|e| e["dir"] == "out" && e["frame"]["id"] == request["id"] && e["frame"].get("result").is_some()).unwrap()["frame"]["result"].clone();
        assert_eq!(accepted["result"], recorded_accept);

        let deny = requests(PLAN_DENY, "cursor/create_plan").remove(0);
        let mut p = protocol("plan");
        let mut step = Step::default();
        p.cursor_extension(&deny, &mut step);
        let approval = step.events.last().unwrap().request_id.clone().unwrap();
        let rejected = p.approve(&approval, false).unwrap();
        let recorded_reject = entries(PLAN_DENY).into_iter().find(|e| e["dir"] == "out" && e["frame"]["id"] == deny["id"] && e["frame"].get("result").is_some()).unwrap()["frame"]["result"].clone();
        assert_eq!(rejected["result"], recorded_reject);
        assert_eq!(rejected["result"], json!({"outcome":{"outcome":"rejected"}}));
        // Stop declines a pending plan with the same body, never `cancelled`.
        assert_eq!(plan_reply(&deny, false), rejected);
    }

    #[test]
    fn an_empty_plan_is_refused_visibly() {
        let mut p = protocol("plan");
        let mut step = Step::default();
        p.cursor_extension(&json!({"jsonrpc":"2.0","id":9,"method":CREATE_PLAN,"params":{"plan":"  ","todos":[]}}), &mut step);
        assert_eq!(step.write[0]["error"]["code"], -32602);
        assert!(step.events.iter().any(|e| e.kind == "status" && e.text.starts_with(PLAN_REFUSED)));
        assert!(p.pending.is_empty());
    }

    #[test]
    fn recorded_update_todos_replaces_and_is_answered_with_the_driver_sent_body() {
        let request = requests(PLAN_LOAD, "cursor/update_todos").remove(0);
        assert_eq!(request["params"]["merge"], false);
        let mut p = protocol("plan");
        let mut step = Step::default();
        p.cursor_extension(&request, &mut step);
        // Driver-sent, not CLI-defined: the recording driver echoed the request's todos and the
        // CLI continued after it.
        let expected = json!({"jsonrpc":"2.0","id":request["id"],"result":{"outcome":{"outcome":"accepted","todos":[{"id":"record-shapes","content":"record shapes","status":"pending"}]}}});
        assert_eq!(step.write, vec![expected]);
        let tool = step.events.iter().find_map(|e| e.tool.as_ref()).unwrap();
        assert_eq!(tool.name, "TodoWrite");
        assert_eq!(serde_json::from_str::<Value>(&tool.input).unwrap(), json!({"todos":[{"content":"record shapes","status":"pending"}]}));
    }

    #[test]
    fn update_todos_merges_by_id_and_keeps_every_field_in_the_reply() {
        let mut p = protocol("ask");
        let frame = |merge: bool, todos: Value| json!({"jsonrpc":"2.0","id":2,"method":UPDATE_TODOS,"params":{"toolCallId":"t","merge":merge,"todos":todos}});
        let mut step = Step::default();
        p.cursor_extension(&frame(false, json!([{"id":"a","content":"first","status":"pending"},{"id":"b","content":"second","status":"pending"}])), &mut step);
        let mut step = Step::default();
        p.cursor_extension(&frame(true, json!([{"id":"b","content":"second","status":"completed"},{"id":"c","content":"third","status":"in_progress"}])), &mut step);
        assert_eq!(
            step.write[0]["result"]["outcome"]["todos"],
            json!([{"id":"a","content":"first","status":"pending"},{"id":"b","content":"second","status":"completed"},{"id":"c","content":"third","status":"in_progress"}])
        );
        assert_eq!(p.cursor_todos.as_array().unwrap().len(), 3);
        let mut step = Step::default();
        p.cursor_extension(&frame(false, json!([{"id":"z","content":"only","status":"pending"}])), &mut step);
        assert_eq!(p.cursor_todos, json!([{"id":"z","content":"only","status":"pending"}]));
        // The published page calls it a notification: no id, no reply.
        let mut step = Step::default();
        let mut notification = frame(true, json!([]));
        notification.as_object_mut().unwrap().remove("id");
        p.cursor_extension(&notification, &mut step);
        assert!(step.write.is_empty());
    }

    // `cursor/task` and `cursor/generate_image` were never raised either; these frames are
    // built from cursor_frames/extension-schemas.md and are not recordings. The success body
    // `{}` is not taken from any fixture.
    #[test]
    fn task_and_generate_image_get_a_bare_success_and_start_nothing() {
        let task = json!({"jsonrpc":"2.0","id":11,"method":TASK,"params":{"toolCallId":"t","description":"d","prompt":"p","subagentType":"explore"}});
        let image = json!({"jsonrpc":"2.0","id":12,"method":GENERATE_IMAGE,"params":{"toolCallId":"t","description":"a cat"}});
        let mut p = protocol("ask");
        for (frame, id) in [(task.clone(), 11), (image.clone(), 12)] {
            let mut step = Step::default();
            p.cursor_extension(&frame, &mut step);
            assert_eq!(step.write, vec![json!({"jsonrpc":"2.0","id":id,"result":{}})]);
            assert!(step.calls.is_empty() && step.failure.is_none() && !step.done);
            assert!(step.write[0].get("error").is_none() && !step.write[0].to_string().contains("cancel"));
        }
        let mut step = Step::default();
        p.cursor_extension(&task, &mut step);
        assert_eq!(step.events.len(), 1, "one status line, no new agent");
        for frame in [task, image] {
            let mut step = Step::default();
            let mut notification = frame;
            notification.as_object_mut().unwrap().remove("id");
            p.cursor_extension(&notification, &mut step);
            assert!(step.write.is_empty(), "no id, no JSON-RPC write");
        }
    }

    #[test]
    fn an_unknown_cursor_request_gets_method_not_found() {
        let mut p = protocol("ask");
        let mut step = Step::default();
        p.cursor_extension(&json!({"jsonrpc":"2.0","id":5,"method":"cursor/mystery","params":{}}), &mut step);
        assert_eq!(step.write, vec![json!({"jsonrpc":"2.0","id":5,"error":{"code":-32601,"message":"Method not found"}})]);
        let mut step = Step::default();
        p.cursor_extension(&json!({"jsonrpc":"2.0","method":"cursor/mystery"}), &mut step);
        assert!(step.write.is_empty());
    }

    // --- models --------------------------------------------------------------------

    #[test]
    fn the_recorded_model_list_parses_without_markers_efforts_or_free_text() {
        let models = parse_models(MODELS_TXT);
        assert!(models.len() > 20);
        assert_eq!(models[0].id, "auto");
        assert_eq!(models[0].name, "Auto", "the (current, default) marker is dropped");
        assert!(models.iter().all(|m| m.reasoning_efforts.is_empty() && m.default_reasoning_effort.is_none()));
        for model in &models {
            assert!(!model.name.contains("(current") && !model.name.contains("(default)"), "{}", model.name);
            assert!(!model.name.contains("  ") && !model.name.chars().any(|c| matches!(c, '\u{200b}'..='\u{200d}')), "{}", model.name);
            assert!(model.id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')));
        }
        let grok = models.iter().find(|m| m.id == "grok-4.7-high-fast").unwrap();
        assert_eq!(grok.name, "Grok 4.7 High Fast", "double spaces and U+200B collapse");
        let fable = models.iter().find(|m| m.id == "claude-fable-5-1-max").unwrap();
        assert!(fable.name.ends_with("(NO ZDR)"), "other markers are kept");
        let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids.len(), ids.iter().collect::<HashSet<_>>().len(), "duplicates keep the first");
        assert!(!ids.contains(&"Available") && !ids.contains(&"Tip:"));
    }

    #[test]
    fn model_lines_must_match_the_whole_row_shape() {
        let output = "Available models\n\nTip: use --model - it is fast\nSome prose with a - dash\nok-id - Fine\n  spaced-id - Spaced  Label \nbad id - x\n- - x\n_lead - x\nnolabel - \nws-id -x\nnever\u{1b}[31m - Red\u{1b}[0m\nauto - Auto (current, default)\nauto - Duplicate\n";
        let ids: Vec<_> = parse_models(output).into_iter().map(|m| (m.id, m.name)).collect();
        assert_eq!(
            ids,
            [("ok-id".to_owned(), "Fine".to_owned()), ("spaced-id".to_owned(), "Spaced Label".to_owned()), ("never".to_owned(), "Red".to_owned()), ("auto".to_owned(), "Auto".to_owned())]
        );
        assert!(parse_models("").is_empty() && parse_models("\n\n").is_empty());
    }

    fn captured(success: bool, stdout: &str, stderr: &str) -> Result<Captured, String> {
        Ok(Captured { success, stdout: stdout.into(), stderr: stderr.into() })
    }

    #[test]
    fn signed_out_lists_are_a_sign_in_error_and_other_failures_are_not() {
        for file in [BADKEY_MODELS, NOKEY_MODELS] {
            let exit = entries(file).into_iter().find(|e| e["dir"] == "exit").unwrap();
            assert_eq!(exit["code"], 1);
            assert_eq!(classify_models(captured(false, "", &stderr_of(file))).unwrap_err(), SIGN_IN_ERROR, "recorded: {file:.20}");
        }
        assert_eq!(classify_models(captured(false, "", "Not logged in")).unwrap_err(), SIGN_IN_ERROR);
        assert_eq!(classify_models(captured(false, "", "please run agent login")).unwrap_err(), SIGN_IN_ERROR);
        for other in [
            classify_models(captured(false, "", "segfault")),
            classify_models(captured(false, "", "")),
            classify_models(captured(true, "   \n", "")),
            classify_models(captured(true, "Models:\n| a | b |\n", "")),
            classify_models(Err("Cursor CLI operation timed out.".into())),
        ] {
            assert_eq!(other.unwrap_err(), MODELS_UNRECOGNISED);
        }
        assert_ne!(SIGN_IN_ERROR, MODELS_UNRECOGNISED);
        let ok = classify_models(captured(true, MODELS_TXT, "")).unwrap();
        assert_eq!(ok[0].id, "default", "the picker keeps its default row");
        assert!(ok.len() > 20);
    }

    #[tokio::test]
    async fn the_model_list_is_cached_for_the_ttl_and_errors_are_not() {
        let _guard = probe_test_guard();
        let dir = std::env::temp_dir().join(format!("pandacode-cursor-models-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("agent");
        std::fs::write(&program, b"stand-in").unwrap();
        let runs = Arc::new(AtomicUsize::new(0));
        let run = |runs: &Arc<AtomicUsize>, result: Result<Captured, String>| {
            let runs = runs.clone();
            move |_path: PathBuf| {
                runs.fetch_add(1, Ordering::SeqCst);
                result
            }
        };
        let bad = models_via(&program, run(&runs, captured(false, "", "Error: Authentication required."))).await;
        assert_eq!(bad.unwrap_err(), SIGN_IN_ERROR);
        let again = models_via(&program, run(&runs, captured(false, "", ""))).await;
        assert_eq!(again.unwrap_err(), MODELS_UNRECOGNISED);
        assert_eq!(runs.load(Ordering::SeqCst), 2, "neither error is cached");
        let first = models_via(&program, run(&runs, captured(true, MODELS_TXT, ""))).await.unwrap();
        let second = models_via(&program, run(&runs, captured(false, "", ""))).await.unwrap();
        assert_eq!(runs.load(Ordering::SeqCst), 3, "a hit within the TTL runs no process");
        assert_eq!(first.len(), second.len());
        super::super::acp_catalog::invalidate("cursor");
        models_via(&program, run(&runs, captured(true, MODELS_TXT, ""))).await.unwrap();
        assert_eq!(runs.load(Ordering::SeqCst), 4, "Refresh runs it again");
        super::super::acp_catalog::invalidate("cursor");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_model_catalog_waits_for_the_probe_and_runs_models_on_the_accepted_path() {
        let _guard = probe_test_guard();
        // A path of its own: `models_via` caches per accepted program, and another test's
        // entry for the same path would answer this one without running `agent models`.
        let directory = std::env::temp_dir().join(format!("pandacode-cursor-catalog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let accepted = directory.join("agent");
        let probes = Arc::new(AtomicUsize::new(0));
        let seen = probes.clone();
        let candidate = accepted.clone();
        set_probe_for_test(vec![candidate.clone()], move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(result_for(INITIALIZE, 1))
        });
        let asked = Arc::new(Mutex::new(Vec::new()));
        let recorded = asked.clone();
        let _models = set_models_for_test(move |path| {
            recorded.lock().unwrap().push(path.to_owned());
            Ok(Captured { success: true, stdout: MODELS_TXT.into(), stderr: String::new() })
        });
        let models = super::super::list_model_catalog("cursor").await.unwrap();
        assert_eq!(probes.load(Ordering::SeqCst), 1, "the catalog waited for one probe");
        assert_eq!(*asked.lock().unwrap(), vec![accepted], "`agent models` ran on the accepted path");
        assert_eq!(models[0].id, "default");
        assert!(models.len() > 20);
        let _ = std::fs::remove_dir_all(&directory);
    }

    // --- recorded frames the shared path handles ----------------------------------------

    #[test]
    fn the_recorded_capabilities_match_the_plan() {
        let capabilities = &result_for(INITIALIZE, 1)["agentCapabilities"];
        assert_eq!(capabilities["loadSession"], true);
        assert_eq!(capabilities["mcpCapabilities"]["http"], true);
        assert_eq!(capabilities["promptCapabilities"]["image"], true);
        assert!(capabilities["sessionCapabilities"].get("resume").is_none(), "session/resume is never sent");
        let mcp = entries(MCP_LOAD).into_iter().find(|e| e["dir"] == "out" && e["frame"]["method"] == "session/load").unwrap();
        assert_eq!(mcp["frame"]["params"]["mcpServers"][0]["type"], "http");
        assert!(requests(MCP_LOAD, "session/request_permission").is_empty(), "no permission request for a client HTTP server");
    }

    /// Live check against the developer's CLI: PandaCode's own probe and model list. It sends
    /// no session request, so it creates no chat (the CLI cannot delete one).
    #[tokio::test]
    #[ignore = "requires an installed Cursor CLI that is signed in"]
    async fn installed_cursor_is_discovered_and_lists_models() {
        let path = refresh().await.expect("Install the Cursor CLI first (`agent` or `cursor-agent`)");
        assert_eq!(accepted_executable().await, Some(path.clone()));
        assert!(!rejected_name(&path));
        let models = models(&path).await.expect("`agent models` should list models (is the CLI signed in?)");
        eprintln!("{} models via {}", models.len(), path.display());
        assert!(models.len() > 1);
        assert!(models.iter().all(|model| model.reasoning_efforts.is_empty()));
    }
}
