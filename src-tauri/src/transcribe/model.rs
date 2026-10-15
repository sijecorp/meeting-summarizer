//! Model files (architecture 5.1, ONB-01, ONB-02): the chosen Whisper model and the Silero VAD
//! model, downloaded with resume and sha256 verification, or imported from a file the recorder
//! picked. Files land in `{local app data}/models/`; a download grows as `{file}.part` until
//! its hash checks out.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

pub struct ModelFile {
    pub repo: &'static str,
    pub file: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// `settings.model` → file. Hashes checked on Hugging Face on 2026-10-13 (architecture 5.1).
/// `tiny` is for development only (try the pipeline without an 874 MB download); the settings
/// screen never offers it.
const WHISPER: [(&str, ModelFile); 4] = [
    (
        "large-v3-turbo-q8_0",
        ModelFile {
            repo: "ggerganov/whisper.cpp",
            file: "ggml-large-v3-turbo-q8_0.bin",
            size: 874_188_075,
            sha256: "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1",
        },
    ),
    (
        "large-v3-turbo",
        ModelFile {
            repo: "ggerganov/whisper.cpp",
            file: "ggml-large-v3-turbo.bin",
            size: 1_624_555_275,
            sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        },
    ),
    (
        "medium-q5_0",
        ModelFile {
            repo: "ggerganov/whisper.cpp",
            file: "ggml-medium-q5_0.bin",
            size: 539_212_467,
            sha256: "19fea4b380c3a618ec4723c3eef2eb785ffba0d0538cf43f8f235e7b3b34220f",
        },
    ),
    (
        "tiny",
        ModelFile {
            repo: "ggerganov/whisper.cpp",
            file: "ggml-tiny.bin",
            size: 77_691_713,
            sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        },
    ),
];
pub const VAD: ModelFile = ModelFile {
    repo: "ggml-org/whisper-vad",
    file: "ggml-silero-v6.2.0.bin",
    size: 885_098,
    sha256: "2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987",
};
/// D12: give up on a connection after 10 s, and on a download that sends nothing for 30 s.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Attempts per file. Each one after the first resumes where the last stopped.
const ATTEMPTS: u32 = 3;

/// The two files transcription needs: the chosen Whisper model and the VAD model.
pub fn needed(model: &str) -> Result<[&'static ModelFile; 2], String> {
    let (_, whisper) = WHISPER
        .iter()
        .find(|(name, _)| *name == model)
        .ok_or_else(|| format!("알 수 없는 모델입니다: {model}"))?;
    Ok([whisper, &VAD])
}

/// `model://download`, about once per percent.
#[derive(Serialize, Clone, Debug)]
pub struct DownloadProgress {
    pub file: &'static str,
    pub received: u64,
    pub total: u64,
}

#[derive(Serialize, Debug)]
pub struct FileState {
    pub file: &'static str,
    pub size: u64,
    pub installed: bool,
    /// Bytes in `{file}.part`, kept for resuming.
    pub received: u64,
}

/// For the settings card and the transcription tab.
#[derive(Serialize, Debug)]
pub struct ModelState {
    pub files: Vec<FileState>,
    pub downloading: bool,
}

/// One download or import at a time.
pub struct Models {
    pub dir: PathBuf,
    busy: Arc<AtomicBool>,
    /// The download in flight, for `cancel`.
    current: Mutex<Option<Current>>,
}

/// One download's cancel mark. Aborting its task stops the network side at once, but a blocking
/// step (hashing 874 MB) runs on to its end. Such a step renames or deletes only under this lock
/// and only while the mark is clear, and `cancel` sets it under the same lock: once `cancel`
/// returns, that download changes nothing on disk.
type CancelMark = Arc<Mutex<bool>>;

struct Current {
    mark: CancelMark,
    abort: Box<dyn Fn() + Send>,
}

/// How a model file looked when hashed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Check {
    Intact,
    /// The wrong bytes; deleted (unless a download is in flight).
    Damaged,
    /// Missing or unreadable.
    Missing,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `act`, unless the mark says cancelled. See `CancelMark`.
fn unless_cancelled<T>(mark: &Mutex<bool>, act: impl FnOnce() -> T) -> Option<T> {
    let cancelled = lock(mark);
    (!*cancelled).then(act)
}

/// Holds `busy` for one download or import and releases it on drop, however that ends. A
/// cancelled download drops it at once, so an import can start right away.
struct Busy(Arc<AtomicBool>);

impl Busy {
    fn take(flag: &Arc<AtomicBool>, taken: &str) -> Result<Self, String> {
        if flag.swap(true, Ordering::SeqCst) {
            return Err(taken.into());
        }
        Ok(Self(flag.clone()))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

fn part_of(path: &Path) -> PathBuf {
    suffixed(path, ".part")
}

fn sha256_hex(path: &Path) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    std::io::copy(&mut File::open(path)?, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// The finished `.part` becomes the model file only if its hash matches; otherwise it is deleted
/// so the next attempt starts clean. Neither happens once `mark` is set.
fn install(part: &Path, dest: &Path, f: &ModelFile, mark: &Mutex<bool>) -> Result<(), String> {
    let hash = sha256_hex(part).map_err(|e| format!("{}을(를) 읽을 수 없습니다: {e}", f.file))?;
    unless_cancelled(mark, || {
        if hash != f.sha256 {
            let _ = fs::remove_file(part);
            return Err(format!(
                "{} 파일이 손상됐습니다(sha256 불일치). 다시 받아 주세요",
                f.file
            ));
        }
        fs::rename(part, dest).map_err(|e| format!("{} 설치 실패: {e}", f.file))
    })
    .unwrap_or_else(|| Err(CANCELLED.into()))
}

/// Hashes an installed file. A damaged one is deleted (unless `mark` is set), so the next download
/// fetches it again instead of trusting its size. Blocking: hashing a model takes seconds.
fn check(path: &Path, f: &ModelFile, mark: &Mutex<bool>) -> Check {
    match sha256_hex(path) {
        Ok(hash) if hash == f.sha256 => Check::Intact,
        Ok(_) => {
            unless_cancelled(mark, || fs::remove_file(path));
            Check::Damaged
        }
        Err(_) => Check::Missing,
    }
}

const CANCELLED: &str = "모델 다운로드를 취소했습니다";

enum Fetch {
    /// Network trouble: the next attempt resumes.
    Retry(String),
    Stop(String),
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(STALL_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
}

/// One file into `dest`: attempts that resume each other, then the hash check.
async fn fetch(
    client: &reqwest::Client,
    base_url: &str,
    dest: &Path,
    f: &'static ModelFile,
    mark: &CancelMark,
    on_progress: &impl Fn(DownloadProgress),
) -> Result<(), String> {
    let url = format!("{base_url}/{}/resolve/main/{}", f.repo, f.file);
    let part = part_of(dest);
    let mut attempt = 1;
    loop {
        match fetch_once(client, &url, &part, f, on_progress).await {
            Ok(()) => break,
            Err(Fetch::Retry(e)) if attempt < ATTEMPTS => {
                eprintln!("모델 다운로드 재시도 {attempt} ({}): {e}", f.file);
                attempt += 1;
            }
            Err(Fetch::Retry(e) | Fetch::Stop(e)) => return Err(e),
        }
    }
    let (dest, mark) = (dest.to_path_buf(), mark.clone());
    blocking(move || install(&part, &dest, f, &mark)).await?
}

/// One attempt: resumes `part` from where it ends.
async fn fetch_once(
    client: &reqwest::Client,
    url: &str,
    part: &Path,
    f: &'static ModelFile,
    on_progress: &impl Fn(DownloadProgress),
) -> Result<(), Fetch> {
    let retry = |e: reqwest::Error| Fetch::Retry(format!("모델을 받지 못했습니다: {e}"));
    let io = |e: std::io::Error| Fetch::Stop(format!("모델 파일을 쓸 수 없습니다: {e}"));
    let mut received = fs::metadata(part).map_or(0, |m| m.len());
    if received == f.size {
        // Complete; only the hash check is left.
        return Ok(());
    }
    if received > f.size {
        fs::remove_file(part).map_err(io)?;
        received = 0;
    }
    let mut request = client.get(url);
    if received > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={received}-"));
    }
    let mut response = request.send().await.map_err(retry)?;
    let status = response.status();
    let mut file = if status == reqwest::StatusCode::PARTIAL_CONTENT {
        // Appended only where the server says the range starts: `bytes {start}-{end}/{size}`.
        let start = response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("bytes "))
            .and_then(|v| v.split('-').next())
            .and_then(|v| v.parse::<u64>().ok());
        if start != Some(received) {
            fs::remove_file(part).map_err(io)?;
            return Err(Fetch::Retry(format!(
                "이어받기 위치가 맞지 않아 처음부터 다시 받습니다 ({start:?} / {received})"
            )));
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(part)
            .map_err(io)?
    } else if status.is_success() {
        // The server ignored the range and sends the whole file. Only replace what we have with
        // something of the model's size.
        if received > 0 && response.content_length() != Some(f.size) {
            return Err(Fetch::Stop(format!(
                "서버가 이어받기를 지원하지 않고, 받은 크기({:?})도 모델과 다릅니다 ({url})",
                response.content_length()
            )));
        }
        received = 0;
        File::create(part).map_err(io)?
    } else {
        return Err(Fetch::Stop(format!(
            "모델을 받지 못했습니다: HTTP {status} ({url})"
        )));
    };
    let mut shown = u64::MAX;
    while let Some(bytes) = response.chunk().await.map_err(retry)? {
        file.write_all(&bytes).map_err(io)?;
        received += bytes.len() as u64;
        if received * 100 / f.size != shown {
            shown = received * 100 / f.size;
            on_progress(DownloadProgress {
                file: f.file,
                received,
                total: f.size,
            });
        }
    }
    // Only an fsync: it changes nothing a later download could trip over if it runs on after a
    // cancel.
    blocking(move || file.sync_all())
        .await
        .map_err(Fetch::Stop)?
        .map_err(io)?;
    if received != f.size {
        return Err(Fetch::Retry(format!(
            "연결이 끊겼습니다 ({received} / {} 바이트)",
            f.size
        )));
    }
    Ok(())
}

impl Models {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            busy: Arc::new(AtomicBool::new(false)),
            current: Mutex::new(None),
        }
    }

    fn path(&self, f: &ModelFile) -> PathBuf {
        self.dir.join(f.file)
    }

    /// By size only; the hash was checked when the file was installed, and again by `download`
    /// and `verify`.
    fn installed(&self, f: &ModelFile) -> bool {
        fs::metadata(self.path(f)).is_ok_and(|m| m.len() == f.size)
    }

    /// Paths of the Whisper and VAD models when both are installed.
    pub fn ready(&self, model: &str) -> Option<(PathBuf, PathBuf)> {
        let [whisper, vad] = needed(model).ok()?;
        (self.installed(whisper) && self.installed(vad))
            .then(|| (self.path(whisper), self.path(vad)))
    }

    /// After a worker could not load the model: hashes both files and deletes a damaged one.
    /// `Intact` means the cause lies elsewhere, such as memory. Blocking.
    pub fn verify(&self, model: &str) -> Check {
        needed(model).map_or(Check::Missing, |files| self.check(&files))
    }

    /// The worst of the files' checks. Every file is hashed, so each damaged one goes. While a
    /// download or import runs, nothing is deleted: it may be installing that very file.
    fn check(&self, files: &[&ModelFile]) -> Check {
        let busy = Busy::take(&self.busy, "").ok();
        let mark = Mutex::new(busy.is_none());
        let checks: Vec<Check> = files
            .iter()
            .map(|f| check(&self.path(f), f, &mark))
            .collect();
        [Check::Damaged, Check::Missing]
            .into_iter()
            .find(|worse| checks.contains(worse))
            .unwrap_or(Check::Intact)
    }

    pub fn state(&self, model: &str) -> Result<ModelState, String> {
        let files = needed(model)?
            .into_iter()
            .map(|f| FileState {
                file: f.file,
                size: f.size,
                installed: self.installed(f),
                received: fs::metadata(part_of(&self.path(f))).map_or(0, |m| m.len()),
            })
            .collect();
        Ok(ModelState {
            files,
            downloading: self.busy.load(Ordering::SeqCst),
        })
    }

    /// Stops the download at once. What arrived stays in the `.part` for resuming, and nothing is
    /// installed or deleted afterwards (`CancelMark`).
    pub fn cancel(&self) {
        if let Some(current) = lock(&self.current).as_ref() {
            *lock(&current.mark) = true;
            (current.abort)();
        }
    }

    /// ONB-01: downloads what is missing or damaged from `{base}/{repo}/resolve/main/{file}`.
    /// Runs as its own task so `cancel` can drop it wherever it waits.
    pub async fn download(
        &self,
        base_url: &str,
        files: [&'static ModelFile; 2],
        on_progress: impl Fn(DownloadProgress) + Send + Sync + 'static,
    ) -> Result<(), String> {
        let busy = Busy::take(&self.busy, "이미 모델을 받는 중입니다")?;
        let dir = self.dir.clone();
        let base_url = base_url.trim().trim_end_matches('/').to_string();
        let mark = CancelMark::default();
        let task_mark = mark.clone();
        let task = {
            // Spawned and stored under one lock: a `cancel` lands before both (and is not for
            // this download) or after both (and reaches it).
            let mut current = lock(&self.current);
            let task = tauri::async_runtime::spawn(async move {
                let _busy = busy;
                fs::create_dir_all(&dir).map_err(|e| format!("모델 폴더 생성 실패: {e}"))?;
                let client = client()?;
                for f in files {
                    let dest = dir.join(f.file);
                    // The right size can still be the wrong bytes: the hash decides (deleted if
                    // not).
                    if fs::metadata(&dest).is_ok_and(|m| m.len() == f.size) {
                        let (path, mark) = (dest.clone(), task_mark.clone());
                        if blocking(move || check(&path, f, &mark)).await? == Check::Intact {
                            continue;
                        }
                    }
                    fetch(&client, &base_url, &dest, f, &task_mark, &on_progress).await?;
                }
                Ok(())
            });
            let abort = task.inner().abort_handle();
            *current = Some(Current {
                mark: mark.clone(),
                abort: Box::new(move || abort.abort()),
            });
            task
        };
        let result = task.await;
        {
            // Only ours: a download started after a cancel may have stored its own by now.
            let mut current = lock(&self.current);
            if current
                .as_ref()
                .is_some_and(|c| Arc::ptr_eq(&c.mark, &mark))
            {
                *current = None;
            }
        }
        match result {
            Ok(result) => result,
            Err(tauri::Error::JoinError(e)) if e.is_cancelled() => Err(CANCELLED.into()),
            Err(e) => Err(format!("모델 다운로드가 비정상 종료했습니다: {e}")),
        }
    }

    /// ONB-02: installs a file the recorder picked, when it is one of `candidates` by size and
    /// sha256. Blocking: hashing a model takes seconds.
    pub fn import(&self, src: &Path, candidates: &[&ModelFile]) -> Result<(), String> {
        let _busy = Busy::take(&self.busy, "모델을 받는 중에는 가져올 수 없습니다")?;
        let size = fs::metadata(src)
            .map_err(|e| format!("고른 파일을 읽을 수 없습니다: {e}"))?
            .len();
        let f = candidates
            .iter()
            .find(|f| f.size == size)
            .ok_or("고른 파일은 이 앱이 쓰는 모델 파일이 아닙니다(크기가 다릅니다)")?;
        fs::create_dir_all(&self.dir).map_err(|e| format!("모델 폴더 생성 실패: {e}"))?;
        let dest = self.path(f);
        // Not the download's `.part`, which stays for resuming.
        let staged = suffixed(&dest, ".import");
        let copied = fs::copy(src, &staged)
            .and_then(|_| OpenOptions::new().write(true).open(&staged)?.sync_all());
        if let Err(e) = copied {
            let _ = fs::remove_file(&staged);
            return Err(format!("모델 파일 복사 실패: {e}"));
        }
        install(&staged, &dest, f, &Mutex::new(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// sha256("hello").
    static HELLO: ModelFile = ModelFile {
        repo: "test/repo",
        file: "hello.bin",
        size: 5,
        sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
    };

    #[test]
    fn hash_mismatch_deletes_the_part_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join(HELLO.file);
        let part = part_of(&dest);
        let clear = Mutex::new(false);
        fs::write(&part, b"hellp").unwrap();
        assert!(
            install(&part, &dest, &HELLO, &clear)
                .unwrap_err()
                .contains("sha256")
        );
        assert!(!part.exists() && !dest.exists());

        fs::write(&part, b"hello").unwrap();
        install(&part, &dest, &HELLO, &clear).unwrap();
        assert!(!part.exists());
        assert_eq!(fs::read(&dest).unwrap(), b"hello");
    }

    #[test]
    fn a_cancelled_install_neither_renames_nor_deletes() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join(HELLO.file);
        let part = part_of(&dest);
        let cancelled = Mutex::new(true);
        for bytes in [b"hello", b"hellp"] {
            fs::write(&part, bytes).unwrap();
            assert_eq!(
                install(&part, &dest, &HELLO, &cancelled).unwrap_err(),
                CANCELLED
            );
            assert!(part.exists() && !dest.exists());
        }
    }

    /// The blocker from the M3 review: the hash of a finished file runs on after `cancel` (a
    /// blocking task cannot be aborted), and must not install the file when it ends.
    #[test]
    fn cancel_while_hashing_installs_nothing_afterwards() {
        let tmp = tempfile::tempdir().unwrap();
        let models = Models::new(tmp.path().to_path_buf());
        // A finished `.part` big enough that hashing it takes a while (sparse: no disk used).
        let size = 1 << 28;
        let part = part_of(&tmp.path().join("big.bin"));
        File::create(&part).unwrap().set_len(size).unwrap();
        let started = Instant::now();
        let sha256 = sha256_hex(&part).unwrap();
        let hashing = started.elapsed();
        let big: &'static ModelFile = Box::leak(Box::new(ModelFile {
            repo: "test/repo",
            file: "big.bin",
            size,
            sha256: sha256.leak(),
        }));

        let result = std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(hashing / 4);
                models.cancel();
            });
            // No request is made: the `.part` is complete, only the hash check is left.
            tauri::async_runtime::block_on(models.download(
                "http://127.0.0.1:9",
                [big, big],
                |_| {},
            ))
        });
        assert_eq!(result.unwrap_err(), CANCELLED);
        assert!(!models.busy.load(Ordering::SeqCst));
        // Let the orphaned hash finish: the file stays a `.part`.
        std::thread::sleep(hashing * 2 + Duration::from_millis(200));
        assert!(part.exists());
        assert!(!models.path(big).exists());
    }

    #[test]
    fn check_tells_intact_damaged_and_missing_apart() {
        let tmp = tempfile::tempdir().unwrap();
        let models = Models::new(tmp.path().to_path_buf());
        let path = models.path(&HELLO);
        assert_eq!(models.check(&[&HELLO]), Check::Missing);
        fs::write(&path, b"hello").unwrap();
        assert_eq!(models.check(&[&HELLO]), Check::Intact);

        fs::write(&path, b"hellp").unwrap();
        // While a download or import runs, a damaged file is reported but left alone.
        let busy = Busy::take(&models.busy, "").unwrap();
        assert_eq!(models.check(&[&HELLO]), Check::Damaged);
        assert!(path.exists());
        drop(busy);
        assert_eq!(models.check(&[&HELLO]), Check::Damaged);
        assert!(!path.exists());
        assert_eq!(models.verify("no-such-model"), Check::Missing);
    }

    #[test]
    fn import_installs_only_a_matching_file() {
        let tmp = tempfile::tempdir().unwrap();
        let models = Models::new(tmp.path().join("models"));
        let src = tmp.path().join("picked.bin");

        fs::write(&src, b"hello, world").unwrap();
        assert!(models.import(&src, &[&HELLO]).unwrap_err().contains("크기"));
        fs::write(&src, b"hellp").unwrap();
        assert!(
            models
                .import(&src, &[&HELLO])
                .unwrap_err()
                .contains("sha256")
        );
        assert!(!models.installed(&HELLO));

        fs::write(&src, b"hello").unwrap();
        models.import(&src, &[&HELLO]).unwrap();
        assert!(models.installed(&HELLO));
        // The busy flag is released after every outcome.
        assert!(!models.busy.load(Ordering::SeqCst));
    }

    #[test]
    fn every_setting_model_has_a_file() {
        for (name, f) in &WHISPER {
            assert_eq!(needed(name).unwrap()[0].file, f.file);
            assert_eq!(f.sha256.len(), 64);
        }
        assert!(needed("large-v3").is_err());
        let tmp = tempfile::tempdir().unwrap();
        let models = Models::new(tmp.path().to_path_buf());
        assert!(models.ready("large-v3-turbo-q8_0").is_none());
        let state = models.state("large-v3-turbo-q8_0").unwrap();
        assert_eq!(state.files.len(), 2);
        assert!(state.files.iter().all(|f| !f.installed && f.received == 0));
    }

    use std::io::Read;
    use std::time::{Duration, Instant};

    fn reply(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    /// A local HTTP server: answers each connection with the next canned bytes, then holds the
    /// connection for `hold`. Returns the base URL and, on join, the request heads it saw.
    fn serve(
        responses: Vec<Vec<u8>>,
        hold: Duration,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let mut heads = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut head = Vec::new();
                let mut buf = [0; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    head.extend_from_slice(&buf[..n]);
                }
                heads.push(String::from_utf8_lossy(&head).to_lowercase());
                stream.write_all(&response).unwrap();
                std::thread::sleep(hold);
            }
            heads
        });
        (url, thread)
    }

    /// One `fetch_once` of HELLO into a `.part` that starts as `part`.
    fn fetch_into(part: &[u8], response: Vec<u8>) -> (Result<(), Fetch>, Option<Vec<u8>>, String) {
        let tmp = tempfile::tempdir().unwrap();
        let path = part_of(&tmp.path().join(HELLO.file));
        if !part.is_empty() {
            fs::write(&path, part).unwrap();
        }
        let (url, server) = serve(vec![response], Duration::ZERO);
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let result =
            tauri::async_runtime::block_on(fetch_once(&client, &url, &path, &HELLO, &|_| {}));
        (
            result,
            fs::read(&path).ok(),
            server.join().unwrap().remove(0),
        )
    }

    #[test]
    fn resumes_only_where_the_server_says_it_resumes() {
        let (result, part, head) = fetch_into(
            b"he",
            reply(
                "206 Partial Content",
                "Content-Range: bytes 2-4/5\r\n",
                b"llo",
            ),
        );
        assert!(result.is_ok());
        assert!(head.contains("range: bytes=2-"), "{head}");
        assert_eq!(part.unwrap(), b"hello");

        // A range starting elsewhere would splice the wrong bytes: start over instead.
        let (result, part, _) = fetch_into(
            b"he",
            reply(
                "206 Partial Content",
                "Content-Range: bytes 0-4/5\r\n",
                b"hello",
            ),
        );
        assert!(matches!(result, Err(Fetch::Retry(_))));
        assert_eq!(part, None);
    }

    #[test]
    fn a_whole_file_answer_to_a_range_replaces_the_part_only_at_the_model_size() {
        let (result, part, _) = fetch_into(b"he", reply("200 OK", "", b"hello"));
        assert!(result.is_ok());
        assert_eq!(part.unwrap(), b"hello");

        let (result, part, _) = fetch_into(b"he", reply("200 OK", "", b"hello, world"));
        assert!(matches!(result, Err(Fetch::Stop(_))));
        assert_eq!(part.unwrap(), b"he");
    }

    #[test]
    fn an_oversized_part_starts_over() {
        let (result, part, head) = fetch_into(b"hello, world", reply("200 OK", "", b"hello"));
        assert!(result.is_ok());
        assert!(!head.contains("range:"), "{head}");
        assert_eq!(part.unwrap(), b"hello");
    }

    #[test]
    fn download_replaces_an_installed_file_whose_hash_is_wrong() {
        let tmp = tempfile::tempdir().unwrap();
        let models = Models::new(tmp.path().to_path_buf());
        // The right size, the wrong bytes: size alone would call it installed.
        fs::write(models.path(&HELLO), b"hellp").unwrap();
        let (url, server) = serve(vec![reply("200 OK", "", b"hello")], Duration::ZERO);
        tauri::async_runtime::block_on(models.download(&url, [&HELLO, &HELLO], |_| {})).unwrap();
        assert_eq!(fs::read(models.path(&HELLO)).unwrap(), b"hello");
        assert_eq!(server.join().unwrap().len(), 1);
    }

    #[test]
    fn cancel_stops_a_stalled_download_at_once_and_frees_import() {
        let tmp = tempfile::tempdir().unwrap();
        let models = Models::new(tmp.path().to_path_buf());
        // Two of five bytes, then nothing for 5 s (the stall timeout is 30 s).
        let mut stalled = reply("200 OK", "", b"hello");
        stalled.truncate(stalled.len() - 3);
        let (url, _server) = serve(vec![stalled], Duration::from_secs(5));
        let (progress, arrived) = std::sync::mpsc::channel();
        let (result, cancelled) = std::thread::scope(|s| {
            let canceller = s.spawn(|| {
                // Once the two bytes are in the `.part`, however long a loaded machine took, the
                // download only waits. A fixed sleep could cancel before the `.part` existed.
                let _ = arrived.into_iter().any(|received| received == 2);
                models.cancel();
                Instant::now()
            });
            let result =
                tauri::async_runtime::block_on(models.download(&url, [&HELLO, &HELLO], move |p| {
                    let _ = progress.send(p.received);
                }));
            (result, canceller.join().unwrap())
        });
        assert_eq!(result.unwrap_err(), CANCELLED);
        assert!(
            cancelled.elapsed() < Duration::from_secs(2),
            "{:?}",
            cancelled.elapsed()
        );
        assert!(!models.busy.load(Ordering::SeqCst));
        assert_eq!(fs::read(part_of(&models.path(&HELLO))).unwrap(), b"he");
    }

    /// Hits the network: `cargo test -- --ignored downloads_and_resumes`. Downloads the 885 KB VAD
    /// model, cuts it in half, and checks the second download resumes with a Range request.
    #[test]
    #[ignore]
    fn downloads_and_resumes_the_vad_model() {
        let tmp = tempfile::tempdir().unwrap();
        let models = Models::new(tmp.path().to_path_buf());
        let get = |models: &Models| {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let into = seen.clone();
            tauri::async_runtime::block_on(models.download(
                "https://huggingface.co/",
                [&VAD, &VAD],
                move |p| into.lock().unwrap().push(p.received),
            ))
            .unwrap();
            std::mem::take(&mut *seen.lock().unwrap())
        };
        let first = get(&models);
        assert_eq!(first.last(), Some(&VAD.size));
        assert!(models.installed(&VAD));

        let dest = models.path(&VAD);
        let half = fs::read(&dest).unwrap()[..400_000].to_vec();
        fs::remove_file(&dest).unwrap();
        fs::write(part_of(&dest), half).unwrap();
        let resumed = get(&models);
        assert!(resumed[0] > 400_000, "resumed from {}", resumed[0]);
        assert!(models.installed(&VAD));
    }
}
