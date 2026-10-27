//! How a command ends, as far as telemetry is concerned: the payload is
//! recorded when the decision is on, and the question is asked when nobody has
//! decided and a person can answer. A recorded payload is also sent, once, to
//! the collector (`send`); "recorded" is `telemetry-last.json` (what
//! `bddkit telemetry show` prints) and, if `BDDKIT_TELEMETRY_LOG` names a file,
//! one line appended to it.
//!
//! Every failure in here is silent: the exit code is the command's, whatever
//! happens to a file or a prompt.

use super::payload::Payload;
use super::{Decision, ask_yes_no, last_payload_path, should_ask, store_answer, write_atomically};
use crate::dirs::Layer;
use crate::privacy::process_env;
use dialoguer::console::Term;
use std::future::Future;
use std::io::Write as _;
use std::time::Duration;

pub struct Ending<'a> {
    pub payload: &'a Payload,
    pub decision: Decision,
    pub layers: &'a [Layer],
    /// `run` and `doctor` ask; the other commands follow whatever is decided.
    pub ask: bool,
}

/// Records the payload, or asks. A signal during the question leaves the
/// prompt with the terminal as it was and stores nothing; the exit is the
/// command's.
pub async fn end(ending: Ending<'_>, interrupts: &mut crate::Interrupts) {
    let can_prompt = ending.ask && crate::report::progress::can_prompt();
    let exit = ending.payload.exit;
    end_with(ending, can_prompt, || prompt(interrupts, exit)).await;
}

/// `end` with the terminal check and the question handed in, which is how the
/// answers are tested without a terminal.
async fn end_with<F, Fut>(ending: Ending<'_>, can_prompt: bool, ask: F)
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Option<bool>>,
{
    if ending.decision.on {
        record(ending.payload, ending.layers).await;
        return;
    }
    if !should_ask(&ending.decision, can_prompt) {
        return;
    }
    // `None`: Esc, Ctrl-C or end of input. Nothing is stored, so the question
    // comes back on the next interactive run.
    let Some(answer) = ask().await else { return };
    if let Ok(path) = super::answer_path(ending.layers, None) {
        let _ = store_answer(&path, answer);
    }
    if answer {
        record(ending.payload, ending.layers).await;
    }
}

/// The text and the list, raced with the signals `run` has registered:
/// `console` raises SIGINT for Ctrl-C, and a handler that is installed would
/// otherwise swallow it.
async fn prompt(interrupts: &mut crate::Interrupts, exit: i32) -> Option<bool> {
    eprintln!("\n{}", super::QUESTION_TEXT);
    let saved = saved_terminal();
    let question =
        tokio::task::spawn_blocking(|| ask_yes_no(&Term::stderr(), super::PROMPT, ["Yes", "No"]));
    tokio::select! {
        answer = question => answer.ok().and_then(Result::ok).flatten(),
        _ = interrupts.wait() => {
            let _ = Term::stderr().show_cursor();
            restore_terminal(saved.as_deref());
            std::process::exit(exit);
        }
    }
}

/// The terminal's settings as `stty -g` prints them. `console` has the
/// terminal raw while it waits for a key and that read cannot be cancelled, so
/// a signal that ends the process under the prompt has to put them back itself
/// (Ctrl-C does not: `console` restores them before it raises SIGINT).
#[cfg(unix)]
fn saved_terminal() -> Option<String> {
    let tty = std::fs::File::open("/dev/tty").ok()?;
    let out = std::process::Command::new("stty")
        .arg("-g")
        .stdin(tty)
        .output()
        .ok()?;
    let settings = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !settings.is_empty()).then_some(settings)
}

#[cfg(unix)]
fn restore_terminal(saved: Option<&str>) {
    if let (Some(saved), Ok(tty)) = (saved, std::fs::File::open("/dev/tty")) {
        let _ = std::process::Command::new("stty")
            .arg(saved)
            .stdin(tty)
            .status();
    }
}

#[cfg(not(unix))]
fn saved_terminal() -> Option<String> {
    None
}

#[cfg(not(unix))]
fn restore_terminal(_: Option<&str>) {}

/// `telemetry-last.json` in the user layer (or the override directory), and
/// the log file if one is named, then the request. The first is replaced
/// whole, so two commands ending together cannot leave half a payload; the
/// second is one `write`.
pub async fn record(payload: &Payload, layers: &[Layer]) {
    write_local(payload, layers);
    send(payload).await;
}

/// Total time the request may take, DNS included: a network that drops
/// packets, or a resolver that never answers, costs a run this much and no
/// more. `main` leaves through `process::exit`, so a resolver thread still
/// stuck in `getaddrinfo` cannot hold the process either.
const TOTAL: Duration = Duration::from_millis(1500);
const CONNECT: Duration = Duration::from_millis(500);

/// One POST of the payload to the collector. No retry, no output, no change
/// to the exit code: every failure is dropped. A signal during the request
/// ends the process with the command's own code instead of waiting for it.
async fn send(payload: &Payload) {
    // The unit tests of this binary end commands through `record`; whatever
    // the build or the environment, none of them may reach a collector.
    if cfg!(test) {
        return;
    }
    let Some(url) = super::endpoint(&process_env) else {
        return;
    };
    let url = format!("{}/v1/run", url.trim_end_matches('/'));
    let Ok(client) = reqwest::Client::builder()
        .connect_timeout(CONNECT)
        .timeout(TOTAL)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return;
    };
    let mut interrupts = crate::Interrupts::install();
    // The outer timeout is the promise; reqwest's own is the same number.
    let request = tokio::time::timeout(TOTAL, client.post(url).json(payload).send());
    tokio::select! {
        _ = request => {}
        _ = interrupts.wait() => std::process::exit(payload.exit),
    }
}

fn write_local(payload: &Payload, layers: &[Layer]) {
    let Ok(line) = serde_json::to_string(payload) else {
        return;
    };
    if let Some(path) = last_payload_path(layers) {
        let _ = write_atomically(&path, &format!("{line}\n"));
    }
    if let Some(path) = process_env("BDDKIT_TELEMETRY_LOG")
        && let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        let _ = file.write_all(format!("{line}\n").as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::payload::Command;
    use crate::telemetry::{Source, Sources, decide, layer_answers};
    use std::path::PathBuf;
    use std::time::Duration;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bddkit-end-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn user_layer(dir: &std::path::Path) -> Vec<Layer> {
        vec![Layer {
            name: "user",
            dir: dir.to_path_buf(),
        }]
    }

    fn payload() -> Payload {
        Payload::new(Command::Run, 0, Duration::ZERO)
    }

    /// What `run` would decide from the layers alone, with a clean environment.
    fn undecided(layers: &[Layer]) -> Decision {
        decide(&Sources {
            flag: None,
            get: &|_| None,
            config: crate::telemetry::ConfigKey::Absent,
            layers: layer_answers(layers),
        })
    }

    async fn ended(layers: &[Layer], decision: Decision, can_prompt: bool, answer: Option<bool>) {
        let payload = payload();
        let ending = Ending {
            payload: &payload,
            decision,
            layers,
            ask: true,
        };
        end_with(ending, can_prompt, || async move { answer }).await;
    }

    fn recorded(dir: &std::path::Path) -> bool {
        dir.join("telemetry-last.json").is_file()
    }

    fn stored(dir: &std::path::Path) -> Option<String> {
        std::fs::read_to_string(dir.join("telemetry.yaml"))
            .ok()
            .map(|text| text.trim().to_string())
    }

    #[tokio::test]
    async fn a_yes_is_stored_in_the_user_layer_and_this_runs_payload_is_recorded() {
        let dir = temp("yes");
        let layers = user_layer(&dir);
        ended(&layers, undecided(&layers), true, Some(true)).await;
        assert_eq!(stored(&dir).as_deref(), Some("enabled: true"));
        assert!(recorded(&dir));
    }

    #[tokio::test]
    async fn a_no_is_stored_records_nothing_and_the_question_is_not_asked_again() {
        let dir = temp("no");
        let layers = user_layer(&dir);
        ended(&layers, undecided(&layers), true, Some(false)).await;
        assert_eq!(stored(&dir).as_deref(), Some("enabled: false"));
        assert!(!recorded(&dir));

        // The next run reads the answer: decided, so the asker is never called.
        let decision = undecided(&layers);
        assert_eq!(decision.by, Some(Source::Layer("user".into())));
        let payload = payload();
        let ending = Ending {
            payload: &payload,
            decision,
            layers: &layers,
            ask: true,
        };
        end_with(ending, true, || async { panic!("asked again") }).await;
    }

    #[tokio::test]
    async fn end_of_input_stores_nothing_so_the_question_comes_back() {
        let dir = temp("eof");
        let layers = user_layer(&dir);
        ended(&layers, undecided(&layers), true, None).await;
        assert_eq!(stored(&dir), None);
        assert!(!recorded(&dir));
        assert!(should_ask(&undecided(&layers), true));
    }

    #[tokio::test]
    async fn a_decided_run_or_one_without_a_terminal_never_asks() {
        let dir = temp("never");
        let layers = user_layer(&dir);
        let payload = payload();
        for (decision, can_prompt) in [
            // A config key decided, on or off.
            (
                Decision {
                    on: false,
                    by: Some(Source::Config),
                },
                true,
            ),
            (
                Decision {
                    on: true,
                    by: Some(Source::Config),
                },
                true,
            ),
            // Nobody decided, but nobody can answer.
            (undecided(&layers), false),
        ] {
            let ending = Ending {
                payload: &payload,
                decision,
                layers: &layers,
                ask: true,
            };
            end_with(ending, can_prompt, || async { panic!("asked") }).await;
        }
        // Only the decision that said on recorded anything, and nobody stored an answer.
        assert!(recorded(&dir));
        assert_eq!(stored(&dir), None);
    }
}
