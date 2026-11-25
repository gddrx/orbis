//! A long-lived yt-dlp: orbis's plugin (`orbis-serve:`, see
//! `orbis_plugin.py`) answers requests for links over stdin and stdout.
//! Python and yt-dlp then start once instead of for every track, and the
//! connections to YouTube stay open: about 1.3 s a link instead of 2.2 s.
//!
//! [`Workers`] keeps at most one such process. It starts on the first
//! request while allowed (the window is shown), and stops when no longer
//! allowed, after [`IDLE`] without requests, or when a request times out.
//! A worker that cannot start, or that dies [`CRASHES`] times, is not
//! started again in the run: links then come from one-shot yt-dlp runs.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Semaphore, oneshot};

use crate::Urgency;

/// How long a new worker may take to say it is ready.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// A worker without requests this long stops.
pub const IDLE: Duration = Duration::from_secs(5 * 60);
/// Deaths after which no worker is started again in the run.
const CRASHES: usize = 3;
/// Lines of the worker's stderr kept for the message when it dies.
const STDERR_LINES: usize = 5;

/// Why the worker gave no link.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Failure {
    /// There is no worker (not allowed, or given up for the run): resolve
    /// another way, nothing to report.
    Off,
    /// The worker could not answer: it did not start or it died. Resolve
    /// another way.
    Worker(String),
    /// yt-dlp ran and failed, such as for a removed track: the request's
    /// warnings and error, as on yt-dlp's stderr.
    Ytdlp(String),
    /// No answer in time; the worker was stopped.
    Timeout,
}

type Answer = Result<Value, String>;

/// What the worker's reader shares with the requests.
#[derive(Default)]
struct Shared {
    /// Requests waiting for their answer, by number.
    pending: Mutex<HashMap<u64, oneshot::Sender<Answer>>>,
    /// The last lines of stderr.
    stderr: Mutex<VecDeque<String>>,
    /// Stdout is still open.
    alive: AtomicBool,
}

impl Shared {
    fn stderr_tail(&self) -> String {
        let lines = self.stderr.lock().expect("stderr lock");
        lines.back().cloned().unwrap_or_else(|| "no message".into())
    }
}

/// One running `yt-dlp … orbis-serve:`.
struct Worker {
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    shared: Arc<Shared>,
    next: AtomicU64,
    in_flight: AtomicUsize,
    last_used: Mutex<Instant>,
    pid: Option<u32>,
    /// Stopped on purpose: requests it drops are not its fault.
    stopped: AtomicBool,
}

impl Worker {
    /// Starts `program` with the plugin in `plugins` and waits until it says
    /// it is ready.
    async fn start(program: &Path, plugins: &Path, timeout: Duration) -> Result<Self, String> {
        let mut child = Command::new(program)
            .args([
                "--ignore-config",
                "--quiet",
                "--no-warnings",
                "--simulate",
                "--plugin-dirs",
            ])
            .arg(plugins)
            .arg("orbis-serve:")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", program.display()))?;
        let pid = child.id();
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err("no pipes to the worker".into());
        };
        let shared = Arc::new(Shared {
            alive: AtomicBool::new(true),
            ..Default::default()
        });
        let (ready, is_ready) = oneshot::channel();
        tokio::spawn(read_answers(stdout, Arc::clone(&shared), ready));
        tokio::spawn(read_stderr(stderr, Arc::clone(&shared)));
        match tokio::time::timeout(timeout, is_ready).await {
            Ok(Ok(())) => Ok(Self {
                stdin: tokio::sync::Mutex::new(Some(stdin)),
                child: Mutex::new(Some(child)),
                shared,
                next: AtomicU64::new(1),
                in_flight: AtomicUsize::new(0),
                last_used: Mutex::new(Instant::now()),
                pid,
                stopped: AtomicBool::new(false),
            }),
            Ok(Err(_)) => Err(format!("it exited: {}", shared.stderr_tail())),
            Err(_) => Err(format!("not ready in {} s", timeout.as_secs())),
        }
    }

    fn alive(&self) -> bool {
        self.shared.alive.load(Ordering::SeqCst)
    }

    fn idle_for(&self) -> Duration {
        self.last_used.lock().expect("worker lock").elapsed()
    }

    fn touch(&self) {
        *self.last_used.lock().expect("worker lock") = Instant::now();
    }

    /// Sends `request` (a JSON object) and waits for its answer.
    async fn ask(&self, mut request: Value, timeout: Duration) -> Result<Value, Failure> {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        request["n"] = n.into();
        let mut line = request.to_string();
        line.push('\n');
        let (sender, answer) = oneshot::channel();
        self.shared
            .pending
            .lock()
            .expect("pending lock")
            .insert(n, sender);
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        self.touch();
        let result = self.send_and_wait(n, &line, answer, timeout).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.touch();
        result
    }

    async fn send_and_wait(
        &self,
        n: u64,
        line: &str,
        answer: oneshot::Receiver<Answer>,
        timeout: Duration,
    ) -> Result<Value, Failure> {
        let sent = {
            let mut stdin = self.stdin.lock().await;
            match stdin.as_mut() {
                Some(stdin) => match stdin.write_all(line.as_bytes()).await {
                    Ok(()) => stdin.flush().await.map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                },
                None => Err("stopped".into()),
            }
        };
        if let Err(e) = sent {
            self.forget(n);
            return Err(Failure::Worker(format!("cannot ask it: {e}")));
        }
        match tokio::time::timeout(timeout, answer).await {
            Ok(Ok(Ok(info))) => Ok(info),
            Ok(Ok(Err(message))) => Err(Failure::Ytdlp(message)),
            Ok(Err(_)) => Err(Failure::Worker(format!(
                "it exited: {}",
                self.shared.stderr_tail()
            ))),
            Err(_) => {
                self.forget(n);
                Err(Failure::Timeout)
            }
        }
    }

    fn forget(&self, n: u64) {
        self.shared.pending.lock().expect("pending lock").remove(&n);
    }

    /// Closes the worker's stdin, which ends it, and kills the process: a
    /// standalone yt-dlp's Python is a child of the process started, and it
    /// goes on its own once stdin is closed.
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Ok(mut stdin) = self.stdin.try_lock() {
            stdin.take();
        }
        if let Some(mut child) = self.child.lock().expect("child lock").take() {
            let _ = child.start_kill();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Hands each answer on stdout to its request; when stdout closes, the
/// requests still waiting learn that the worker is gone.
async fn read_answers(
    stdout: tokio::process::ChildStdout,
    shared: Arc<Shared>,
    ready: oneshot::Sender<()>,
) {
    let mut ready = Some(ready);
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        // The line carries a signed link: it is never logged.
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value.get("ready").and_then(Value::as_bool) == Some(true) {
            if let Some(ready) = ready.take() {
                let _ = ready.send(());
            }
            continue;
        }
        let Some(n) = value.get("n").and_then(Value::as_u64) else {
            continue;
        };
        let Some(sender) = shared.pending.lock().expect("pending lock").remove(&n) else {
            continue;
        };
        let answer = match value.get("info") {
            Some(info) => Ok(info.clone()),
            None => Err(value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("yt-dlp failed")
                .to_owned()),
        };
        let _ = sender.send(answer);
    }
    shared.alive.store(false, Ordering::SeqCst);
    shared.pending.lock().expect("pending lock").clear();
}

async fn read_stderr(stderr: tokio::process::ChildStderr, shared: Arc<Shared>) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let mut kept = shared.stderr.lock().expect("stderr lock");
        if kept.len() == STDERR_LINES {
            kept.pop_front();
        }
        kept.push_back(line);
    }
}

type Slot = Arc<tokio::sync::Mutex<Option<Arc<Worker>>>>;

/// At most one worker, started on demand.
pub(crate) struct Workers {
    plugins: PathBuf,
    idle: Duration,
    allowed: AtomicBool,
    /// A worker failed to start or died too often: none in this run.
    given_up: AtomicBool,
    crashes: AtomicUsize,
    current: Slot,
    /// Requests made ahead (a track under the pointer, the visible rows)
    /// take at most one of the worker's threads, so a click never waits
    /// behind them.
    ahead: Semaphore,
}

impl std::fmt::Debug for Workers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workers")
            .field("plugins", &self.plugins)
            .field("allowed", &self.allowed.load(Ordering::Relaxed))
            .field("given_up", &self.given_up.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Workers {
    /// Workers with orbis's plugin in `plugins`, stopped after `idle`
    /// without requests. None runs until [`Workers::allow`].
    pub(crate) fn new(plugins: PathBuf, idle: Duration) -> Self {
        Self {
            plugins,
            idle,
            allowed: AtomicBool::new(false),
            given_up: AtomicBool::new(false),
            crashes: AtomicUsize::new(0),
            current: Arc::default(),
            ahead: Semaphore::new(1),
        }
    }

    /// Lets a worker run, or stops the one running.
    pub(crate) fn allow(&self, allowed: bool) {
        self.allowed.store(allowed, Ordering::SeqCst);
        if !allowed {
            // A worker being started now is stopped by its starter, which
            // checks again once it is ready.
            if let Ok(mut current) = self.current.try_lock()
                && let Some(worker) = current.take()
            {
                worker.stop();
            }
        }
    }

    pub(crate) fn allowed(&self) -> bool {
        self.allowed.load(Ordering::SeqCst)
    }

    /// The process ID of the running worker.
    pub(crate) fn pid(&self) -> Option<u32> {
        let current = self.current.try_lock().ok()?;
        current.as_ref().filter(|w| w.alive()).and_then(|w| w.pid)
    }

    /// Starts a worker of `program` now, when one may run, so the first
    /// link does not wait for it.
    pub(crate) async fn warm_up(&self, program: &Path) {
        let _ = self.worker(program).await;
    }

    /// Asks the worker for `request` (see `orbis_plugin.py`).
    pub(crate) async fn resolve(
        &self,
        program: &Path,
        request: Value,
        urgency: Urgency,
        timeout: Duration,
    ) -> Result<Value, Failure> {
        if !self.allowed() || self.given_up.load(Ordering::SeqCst) {
            return Err(Failure::Off);
        }
        let _ahead = match urgency {
            Urgency::Ahead => Some(self.ahead.acquire().await.map_err(|_| Failure::Off)?),
            Urgency::Now => None,
        };
        let worker = self.worker(program).await?;
        let result = match worker.ask(request, timeout).await {
            // Stopped meanwhile (the window went to the tray, another
            // request timed out): the link comes another way.
            Err(Failure::Worker(_)) if worker.stopped.load(Ordering::SeqCst) => Err(Failure::Off),
            other => other,
        };
        match &result {
            Err(Failure::Timeout) => self.drop_worker(&worker).await,
            Err(Failure::Worker(_)) => {
                if self.crashes.fetch_add(1, Ordering::SeqCst) + 1 >= CRASHES {
                    self.give_up("it died too often");
                }
                self.drop_worker(&worker).await;
            }
            // No longer allowed while it answered: `allow` could not stop
            // a worker that a request was taking.
            _ if !self.allowed() => self.drop_worker(&worker).await,
            _ => {}
        }
        result
    }

    /// The running worker, or a new one.
    async fn worker(&self, program: &Path) -> Result<Arc<Worker>, Failure> {
        let mut current = self.current.lock().await;
        if let Some(worker) = current.as_ref().filter(|w| w.alive()) {
            return Ok(Arc::clone(worker));
        }
        *current = None;
        if !self.allowed() || self.given_up.load(Ordering::SeqCst) {
            return Err(Failure::Off);
        }
        let worker = match Worker::start(program, &self.plugins, START_TIMEOUT).await {
            Ok(worker) => Arc::new(worker),
            Err(e) => {
                self.give_up(&e);
                return Err(Failure::Worker(e));
            }
        };
        if !self.allowed() {
            // Not allowed any more while it started.
            worker.stop();
            return Err(Failure::Off);
        }
        self.stop_when_idle(&worker);
        *current = Some(Arc::clone(&worker));
        Ok(worker)
    }

    fn give_up(&self, why: &str) {
        if !self.given_up.swap(true, Ordering::SeqCst) {
            eprintln!(
                "orbis: no yt-dlp worker in this run, links come one yt-dlp at a time: {why}"
            );
        }
    }

    /// Stops `worker` and forgets it, unless another one has replaced it.
    async fn drop_worker(&self, worker: &Arc<Worker>) {
        let mut current = self.current.lock().await;
        if current.as_ref().is_some_and(|c| Arc::ptr_eq(c, worker)) {
            *current = None;
        }
        worker.stop();
    }

    /// Stops `worker` once it has had no requests for [`Workers::idle`].
    fn stop_when_idle(&self, worker: &Arc<Worker>) {
        let idle = self.idle;
        let slot = Arc::downgrade(&self.current);
        let worker = Arc::downgrade(worker);
        let check = (idle / 4).clamp(Duration::from_millis(10), Duration::from_secs(30));
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(check).await;
                let Some(running) = worker.upgrade() else {
                    break;
                };
                if !running.alive() {
                    break;
                }
                if running.idle_for() < idle || running.in_flight.load(Ordering::SeqCst) > 0 {
                    continue;
                }
                if let Some(slot) = slot.upgrade() {
                    let mut current = slot.lock().await;
                    if current.as_ref().is_some_and(|c| Arc::ptr_eq(c, &running)) {
                        *current = None;
                    }
                }
                running.stop();
                break;
            }
        });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// A stand-in for yt-dlp that speaks the worker's protocol: `crash`
    /// makes it exit, `hang` never answers, `gone` fails like yt-dlp does
    /// for a removed track; any other ID gets a link. It starts by writing
    /// its process ID to the file `PIDS`; with `BROKEN` it fails at once.
    const FAKE: &str = r#"#!/usr/bin/env python3
import json, os, sys
if sys.argv[1:] == ['--probe']:
    sys.exit(0)
with open('PIDS', 'a') as f:
    f.write(f'{os.getpid()}\n')
if BROKEN:
    sys.stderr.write('ImportError: broken plugin\n')
    sys.exit(1)
print('a line that is not an answer', flush=True)
print(json.dumps({'ready': True, 'version': 'fake'}), flush=True)
for line in sys.stdin:
    r = json.loads(line)
    vid = r['video_id']
    if vid == 'crash':
        sys.stderr.write('Traceback: boom\n'); sys.stderr.flush()
        os._exit(3)
    if vid == 'hang':
        continue
    if vid == 'gone':
        out = {'n': r['n'], 'error': 'ERROR: [youtube] gone: Video unavailable'}
    else:
        out = {'n': r['n'], 'info': {'url': 'https://v.example.invalid/?expire=9', 'format_id': '774',
                                     'acodec': 'opus', 'ext': 'webm', 'quick': r.get('quick')}}
    print(json.dumps(out), flush=True)
"#;

    struct Fake {
        _dir: tempfile::TempDir,
        program: PathBuf,
        pids: PathBuf,
    }

    /// The fake yt-dlp, or `None` without a Python to run it. Each test has
    /// its own, with what it needs written into it: the tests run at once.
    fn fake(broken: bool) -> Option<Fake> {
        use std::os::unix::fs::PermissionsExt;
        std::process::Command::new("python3")
            .arg("--version")
            .output()
            .ok()?;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("yt-dlp");
        let pids = dir.path().join("pids");
        let script = FAKE
            .replace("PIDS", pids.to_str().unwrap())
            .replace("BROKEN", if broken { "True" } else { "False" });
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        wait_until_runnable(&program);
        Some(Fake {
            _dir: dir,
            program,
            pids,
        })
    }

    /// Waits until `program`, just written, can run. A process another
    /// test starts at the same moment may hold the file open for writing
    /// until it runs its own program, and running ours then fails with
    /// "Text file busy".
    pub(crate) fn wait_until_runnable(program: &Path) {
        for _ in 0..100 {
            match std::process::Command::new(program).arg("--probe").output() {
                Err(e) if e.raw_os_error() == Some(26) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => return,
            }
        }
    }

    fn starts(fake: &Fake) -> usize {
        std::fs::read_to_string(&fake.pids)
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    fn request(video_id: &str) -> Value {
        json!({"video_id": video_id, "quick": true})
    }

    const SECOND: Duration = Duration::from_secs(1);

    #[tokio::test]
    async fn one_worker_answers_many_requests() {
        let Some(fake) = fake(false) else { return };
        let workers = Workers::new("/plugins".into(), IDLE);
        let off = workers
            .resolve(&fake.program, request("a"), Urgency::Now, SECOND * 5)
            .await;
        assert_eq!(off, Err(Failure::Off), "not allowed yet");

        workers.allow(true);
        let (a, b) = tokio::join!(
            workers.resolve(&fake.program, request("a"), Urgency::Now, SECOND * 5),
            workers.resolve(&fake.program, request("b"), Urgency::Ahead, SECOND * 5),
        );
        assert_eq!(a.unwrap()["format_id"], "774");
        assert_eq!(b.unwrap()["quick"], true);
        let gone = workers
            .resolve(&fake.program, request("gone"), Urgency::Now, SECOND * 5)
            .await;
        assert_eq!(
            gone,
            Err(Failure::Ytdlp(
                "ERROR: [youtube] gone: Video unavailable".into()
            ))
        );
        assert_eq!(starts(&fake), 1, "one process for all");
        assert!(workers.pid().is_some());

        workers.allow(false);
        assert_eq!(workers.pid(), None, "stopped when no longer allowed");
    }

    #[tokio::test]
    async fn a_dead_worker_is_replaced_and_then_given_up() {
        let Some(fake) = fake(false) else { return };
        let workers = Workers::new("/plugins".into(), IDLE);
        workers.allow(true);
        for round in 1..=CRASHES {
            let crash = workers
                .resolve(&fake.program, request("crash"), Urgency::Now, SECOND * 5)
                .await;
            assert_eq!(
                crash,
                Err(Failure::Worker("it exited: Traceback: boom".into()))
            );
            if round < CRASHES {
                let next = workers
                    .resolve(&fake.program, request("a"), Urgency::Now, SECOND * 5)
                    .await;
                assert!(next.is_ok(), "a new worker after crash {round}");
            }
        }
        let after = workers
            .resolve(&fake.program, request("a"), Urgency::Now, SECOND * 5)
            .await;
        assert_eq!(after, Err(Failure::Off), "given up after {CRASHES} crashes");
        assert_eq!(starts(&fake), CRASHES);
    }

    #[tokio::test]
    async fn a_hung_request_stops_the_worker() {
        let Some(fake) = fake(false) else { return };
        let workers = Workers::new("/plugins".into(), IDLE);
        workers.allow(true);
        let hung = workers
            .resolve(
                &fake.program,
                request("hang"),
                Urgency::Now,
                Duration::from_millis(300),
            )
            .await;
        assert_eq!(hung, Err(Failure::Timeout));
        assert_eq!(workers.pid(), None);
        let next = workers
            .resolve(&fake.program, request("a"), Urgency::Now, SECOND * 5)
            .await;
        assert!(next.is_ok());
        assert_eq!(starts(&fake), 2, "started anew");
    }

    #[tokio::test]
    async fn stopping_on_purpose_is_not_a_crash() {
        let Some(fake) = fake(false) else { return };
        let workers = Arc::new(Workers::new("/plugins".into(), IDLE));
        for _ in 0..CRASHES + 1 {
            workers.allow(true);
            let asking = {
                let workers = Arc::clone(&workers);
                let program = fake.program.clone();
                tokio::spawn(async move {
                    workers
                        .resolve(&program, request("hang"), Urgency::Now, SECOND * 5)
                        .await
                })
            };
            while workers.pid().is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            workers.allow(false);
            assert_eq!(asking.await.unwrap(), Err(Failure::Off));
        }
        workers.allow(true);
        let after = workers
            .resolve(&fake.program, request("a"), Urgency::Now, SECOND * 5)
            .await;
        assert!(after.is_ok(), "not given up: {after:?}");
    }

    #[tokio::test]
    async fn an_idle_worker_stops() {
        let Some(fake) = fake(false) else { return };
        let workers = Workers::new("/plugins".into(), Duration::from_millis(200));
        workers.allow(true);
        workers.warm_up(&fake.program).await;
        let pid = workers.pid().expect("warmed up");
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(workers.pid(), None, "stopped after the idle time");
        let proc = PathBuf::from(format!("/proc/{pid}"));
        for _ in 0..50 {
            if !proc.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!proc.exists(), "the process is gone");
    }

    #[tokio::test]
    async fn a_worker_that_cannot_start_is_given_up() {
        let Some(fake) = fake(true) else { return };
        let workers = Workers::new("/plugins".into(), IDLE);
        workers.allow(true);
        let first = workers
            .resolve(&fake.program, request("a"), Urgency::Now, SECOND * 5)
            .await;
        assert_eq!(
            first,
            Err(Failure::Worker(
                "it exited: ImportError: broken plugin".into()
            ))
        );
        let second = workers
            .resolve(&fake.program, request("a"), Urgency::Now, SECOND * 5)
            .await;
        assert_eq!(second, Err(Failure::Off));
        assert_eq!(starts(&fake), 1);
    }
}
