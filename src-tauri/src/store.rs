use crate::settings::LlmConnection;
use crate::summarize::split_title;
use crate::transcribe::engine::Segment;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SCHEMA_VERSION: u32 = 1;
pub const NEWER_VERSION: &str = "새 버전 앱에서 만든 데이터입니다";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Recording,
    Recorded,
    Transcribing,
    Transcribed,
    Summarizing,
    Review,
    Sent,
    Archived,
    Paused,
    TranscribeFailed,
    SummarizeFailed,
    Deleting,
    /// A status from a newer app. Such meetings are read-only (architecture 8.2).
    #[serde(other)]
    Unknown,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Gap {
    pub at_ms: u64,
    pub len_ms: u64,
    pub reason: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Sent {
    pub channel: String,
    pub at: String,
    pub edit_count: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Meta {
    pub schema_version: u32,
    pub id: String,
    pub title: String,
    pub status: Status,
    pub created_at: String,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub gaps: Vec<Gap>,
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub chunks_done: u32,
    #[serde(default)]
    pub llm_connection: Option<String>,
    #[serde(default)]
    pub summarize_local: bool,
    /// SUM-10: too little speech to summarize, so no LLM was asked and there is nothing to send.
    #[serde(default)]
    pub no_speech: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub sent: Option<Sent>,
    /// The grounding check (plan 5.6): owners and deadlines it made "미정"
    /// (`grounding::Grounded`).
    #[serde(default)]
    pub grounding_replaced: u32,
    /// Whether it could read the action items. `None`: no summary checked (none yet, no speech,
    /// or made before the check existed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grounding_checked: Option<bool>,
    /// Why a meeting with a transcript waits in `transcribed` or `paused` instead of summarizing
    /// (plan 5.4). Written by the queue, gone once its summary starts or its connection is changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<Wait>,
}

/// Plan 5.4, ENG15: the meeting's route does not resolve yet.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Wait {
    /// The built-in route needs the summary model. Whether it is downloading is `model_status`'s.
    SummaryModel,
    /// The connection it was pinned to is gone. It is never sent elsewhere unasked.
    ConnectionDeleted,
}

impl Meta {
    /// Summarizing has started, so where it goes is settled (SUM-08): a connection was chosen, or
    /// the meeting is past the stages before summarizing.
    pub fn summary_started(&self) -> bool {
        use Status::*;
        self.llm_connection.is_some()
            || !matches!(
                self.status,
                Recording | Recorded | Transcribing | Transcribed | Paused | TranscribeFailed
            )
    }
}

/// Architecture 8.3. Moving to `Deleting` is allowed from any state.
pub fn can_transition(from: Status, to: Status) -> bool {
    use Status::*;
    if to == Deleting {
        return from != Deleting;
    }
    matches!(
        (from, to),
        (Recording, Recorded)
            | (Recorded, Transcribing)
            | (Transcribing, Transcribed | Paused | TranscribeFailed)
            | (TranscribeFailed, Transcribing)
            | (Transcribed, Summarizing)
            | (Summarizing, Review | Paused | SummarizeFailed)
            | (SummarizeFailed, Summarizing)
            | (Paused, Transcribing | Summarizing)
            | (Review, Sent | Archived)
    )
}

fn transition(meta: &mut Meta, to: Status) -> Result<(), String> {
    if !can_transition(meta.status, to) {
        return Err(format!(
            "{:?}에서 {:?}(으)로 바꿀 수 없습니다",
            meta.status, to
        ));
    }
    meta.status = to;
    Ok(())
}

#[derive(Debug, PartialEq)]
pub enum Recovery {
    /// Shown as "중단된 녹음"; the recorder chooses 처리 계속 / 삭제 (STO-03).
    Interrupted,
    FinishDelete,
    Requeue,
    Nothing,
}

/// Architecture 8.4: what to do with a meeting found at app start.
pub fn classify(status: Status) -> Recovery {
    use Status::*;
    match status {
        Recording => Recovery::Interrupted,
        Deleting => Recovery::FinishDelete,
        Transcribing | Paused | Recorded | Transcribed | Summarizing => Recovery::Requeue,
        Review | Sent | Archived | TranscribeFailed | SummarizeFailed | Unknown => {
            Recovery::Nothing
        }
    }
}

/// `2026-10-13T05:00:03Z`: how `meta.json` keeps times (8.2). UTC: std/time cannot read the local
/// offset safely; the UI formats it in local time.
pub fn utc(at: time::OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        at.year(),
        at.month() as u8,
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    )
}

/// A title as stored: one line, however it was typed or pasted. It heads the `.md` export and
/// names its file, and Slack's header (7.1).
pub fn one_line(title: &str) -> String {
    title
        .split(|c: char| c.is_control() || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `fs::rename`, tried again a few times on Windows: antivirus or the search indexer can hold a file
/// open for a moment.
fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut retries = 0;
    loop {
        match fs::rename(from, to) {
            // 32 is ERROR_SHARING_VIOLATION, which std reports as an uncategorized error.
            Err(e)
                if cfg!(windows)
                    && (e.kind() == ErrorKind::PermissionDenied
                        || e.raw_os_error() == Some(32))
                    && retries < 3 =>
            {
                retries += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            result => return result,
        }
    }
}

/// Write to a temp file, fsync, then rename over the target so a crash never leaves a torn file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    let replaced = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        // Windows cannot rename a file this process still holds open.
        drop(file);
        rename(&tmp, path)
    })();
    if replaced.is_err() {
        // A half-written temp file holds the disk space the low-disk guard is trying to keep.
        let _ = fs::remove_file(&tmp);
        return replaced;
    }
    // Unix: the rename itself is only durable once the directory entry is flushed.
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

/// Starts the name of an upload's temp file in `meetings/` (`Store::import_temp`).
const IMPORT_PREFIX: &str = ".import-";

/// A new meeting's `meta.json`. The id is `now` (UTC) and a few changing bits of it, so two never
/// meet; `created_at` is when the meeting began.
fn new_meta(
    now: time::OffsetDateTime,
    created_at: time::OffsetDateTime,
    title: &str,
    status: Status,
    summarize_local: bool,
) -> Meta {
    Meta {
        schema_version: SCHEMA_VERSION,
        id: format!(
            "{:04}-{:02}-{:02}T{:02}{:02}-{:04x}",
            now.year(),
            now.month() as u8,
            now.day(),
            now.hour(),
            now.minute(),
            (now.unix_timestamp_nanos() / 1000) as u16
        ),
        title: title.to_string(),
        status,
        created_at: utc(created_at),
        duration_ms: 0,
        gaps: Vec::new(),
        backend: None,
        chunks_done: 0,
        llm_connection: None,
        summarize_local,
        no_speech: false,
        error: None,
        sent: None,
        grounding_replaced: 0,
        grounding_checked: None,
        wait: None,
    }
}

pub struct Store {
    dir: PathBuf,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// `recording` meetings found at startup: nothing is recording them in this process.
    interrupted: Mutex<HashSet<String>>,
}

impl Store {
    pub fn new(dir: PathBuf) -> std::io::Result<Self> {
        fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            locks: Mutex::new(HashMap::new()),
            interrupted: Mutex::new(HashSet::new()),
        })
    }

    /// Every path into a meeting folder goes through here, so an id from the webview
    /// can never point outside `meetings/`.
    pub fn dir_for(&self, id: &str) -> Result<PathBuf, String> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(format!("잘못된 회의 id입니다: {id}"));
        }
        Ok(self.dir.join(id))
    }

    /// Validates the id first, so a bad id never gets a registry entry.
    fn lock_for(&self, id: &str) -> Result<Arc<Mutex<()>>, String> {
        self.dir_for(id)?;
        Ok(self
            .locks
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_default()
            .clone())
    }

    fn read(&self, id: &str) -> Result<Meta, String> {
        let path = self.dir_for(id)?.join("meta.json");
        let bytes = fs::read(&path).map_err(|_| format!("회의를 찾을 수 없습니다: {id}"))?;
        let corrupt = |e: serde_json::Error| format!("meta.json을 읽을 수 없습니다 ({id}): {e}");
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(corrupt)?;
        let newer = value["schema_version"]
            .as_u64()
            .is_some_and(|v| v > SCHEMA_VERSION.into());
        let mut meta: Meta = match serde_json::from_value(value.clone()) {
            Ok(meta) => meta,
            // A newer app may have changed a field's shape. Keep the meeting visible, read-only (8.2).
            Err(_) if newer => serde_json::from_value(serde_json::json!({
                "schema_version": value["schema_version"],
                "id": id,
                "title": value["title"].as_str().unwrap_or_default(),
                "status": "unknown",
                "created_at": value["created_at"].as_str().unwrap_or_default(),
            }))
            .map_err(corrupt)?,
            Err(e) => return Err(corrupt(e)),
        };
        // The folder name is the id: a copied or renamed folder must never write to another meeting.
        meta.id = id.to_string();
        Ok(meta)
    }

    fn write(&self, meta: &Meta) -> Result<(), String> {
        let path = self.dir_for(&meta.id)?.join("meta.json");
        let bytes = serde_json::to_vec_pretty(meta).map_err(|e| e.to_string())?;
        write_atomic(&path, &bytes).map_err(|e| format!("meta.json 저장 실패: {e}"))
    }

    /// `summarize_local`: "로컬로 요약" starts as the settings' default (SUM-08).
    pub fn create(&self, title: &str, summarize_local: bool) -> Result<Meta, String> {
        let now = time::OffsetDateTime::now_utc();
        let meta = new_meta(now, now, title, Status::Recording, summarize_local);
        // create_dir (not _all) fails on an id collision instead of reusing a folder.
        fs::create_dir(self.dir_for(&meta.id)?).map_err(|e| format!("회의 폴더 생성 실패: {e}"))?;
        self.write(&meta)?;
        Ok(meta)
    }

    /// Where an upload is decoded (REC-12) before it has a meeting: a file in `meetings/`, so moving
    /// it in is a rename on the same volume. Startup recovery removes one left by a crash.
    pub fn import_temp(&self) -> PathBuf {
        let nanos = time::OffsetDateTime::now_utc().unix_timestamp_nanos();
        self.dir.join(format!("{IMPORT_PREFIX}{nanos:x}.wav"))
    }

    /// An upload (REC-12) as a `recorded` meeting, `wav` (from `import_temp`) as its `mic.wav`.
    /// `meta.json` comes last, so nothing ever sees the meeting without its audio.
    pub fn import(
        &self,
        wav: &Path,
        created_at: time::OffsetDateTime,
        duration_ms: u64,
        summarize_local: bool,
    ) -> Result<Meta, String> {
        let now = time::OffsetDateTime::now_utc();
        let mut meta = new_meta(now, created_at, "", Status::Recorded, summarize_local);
        meta.duration_ms = duration_ms;
        let dir = self.dir_for(&meta.id)?;
        // create_dir (not _all): a folder already there is another meeting's, never cleaned up here.
        if let Err(e) = fs::create_dir(&dir) {
            let _ = fs::remove_file(wav);
            return Err(format!("회의 폴더 생성 실패: {e}"));
        }
        let placed = rename(wav, &dir.join("mic.wav"))
            .map_err(|e| format!("녹음 파일을 옮기지 못했습니다: {e}"))
            .and_then(|_| self.write(&meta));
        if let Err(e) = placed {
            let _ = fs::remove_file(wav);
            let _ = fs::remove_dir_all(&dir);
            return Err(e);
        }
        Ok(meta)
    }

    pub fn get(&self, id: &str) -> Result<Meta, String> {
        self.read(id)
    }

    /// The summary body: `summary.md` (the recorder's edit) if present, else
    /// `summary.generated.md` without its `# 제목` line (that is `meta.title`), else empty.
    /// Only a missing file falls through; any other read error is reported.
    pub fn summary(&self, id: &str) -> Result<String, String> {
        let dir = self.dir_for(id)?;
        for name in ["summary.md", "summary.generated.md"] {
            match fs::read_to_string(dir.join(name)) {
                Ok(text) if name == "summary.md" => return Ok(text),
                Ok(text) => return Ok(split_title(&text).1),
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{name}을 읽을 수 없습니다: {e}")),
            }
        }
        Ok(String::new())
    }

    /// `transcript.json` (architecture 5.2), for reading only. Written once by the queue.
    pub fn transcript(&self, id: &str) -> Result<Vec<Segment>, String> {
        #[derive(Deserialize)]
        struct Transcript {
            segments: Vec<Segment>,
        }
        let unreadable = |e: String| format!("transcript.json을 읽을 수 없습니다: {e}");
        let bytes = fs::read(self.dir_for(id)?.join("transcript.json"))
            .map_err(|e| unreadable(e.to_string()))?;
        serde_json::from_slice::<Transcript>(&bytes)
            .map(|t| t.segments)
            .map_err(|e| unreadable(e.to_string()))
    }

    /// Newest first. Unreadable meetings are skipped.
    pub fn list(&self) -> Vec<Meta> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut metas: Vec<Meta> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter_map(|id| match self.read(&id) {
                Ok(meta) => Some(meta),
                Err(e) => {
                    log::warn!("{e}");
                    None
                }
            })
            .collect();
        metas.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        metas
    }

    /// The single write path for meta.json: read-modify-write under the meeting's lock.
    /// Rejects missing, `deleting` and newer-schema meetings, so late results are dropped.
    pub fn update(
        &self,
        id: &str,
        f: impl FnOnce(&mut Meta) -> Result<(), String>,
    ) -> Result<Meta, String> {
        let lock = self.lock_for(id)?;
        let _guard = lock.lock().unwrap();
        let mut meta = self.read(id)?;
        if meta.schema_version > SCHEMA_VERSION {
            return Err(NEWER_VERSION.to_string());
        }
        if meta.status == Status::Deleting {
            return Err(format!("삭제 중인 회의입니다: {id}"));
        }
        let from = meta.status;
        f(&mut meta)?;
        self.write(&meta)?;
        // Plan 5.1-13: app.log's timestamps then tell how long each step took.
        if meta.status != from {
            log::info!("상태 ({id}): {from:?} -> {:?}", meta.status);
        }
        Ok(meta)
    }

    pub fn set_status(&self, id: &str, to: Status) -> Result<Meta, String> {
        self.update(id, |meta| transition(meta, to))
    }

    /// REV-01: the recorder's title and summary. `summary.generated.md` stays as the grounding check
    /// left it (SUM-05). The title is kept to one line (`one_line`).
    pub fn save_summary(&self, id: &str, title: &str, body: &str) -> Result<Meta, String> {
        let path = self.dir_for(id)?.join("summary.md");
        let title = one_line(title);
        self.update(id, |meta| {
            if meta.status != Status::Review {
                return Err("검토 중인 회의만 저장할 수 있습니다".into());
            }
            write_atomic(&path, body.as_bytes())
                .map_err(|e| format!("summary.md 저장 실패: {e}"))?;
            meta.title = title;
            Ok(())
        })
    }

    /// SUM-08 "로컬로 요약", until summarizing starts.
    pub fn set_summarize_local(&self, id: &str, local: bool) -> Result<Meta, String> {
        self.update(id, |meta| {
            if meta.summary_started() {
                return Err("요약이 이미 시작돼 바꿀 수 없습니다".into());
            }
            meta.summarize_local = local;
            Ok(())
        })
    }

    /// SUM-06 "요약 다시 실행" with the connection the recorder picked, and ENG15's change of
    /// connection for a meeting waiting with its transcript (`transcribed`, or `paused` after it).
    /// The caller queues it.
    pub fn retry_summary(&self, id: &str, connection: &LlmConnection) -> Result<Meta, String> {
        let transcript = self.dir_for(id)?.join("transcript.json");
        self.update(id, |meta| {
            let waiting = match meta.status {
                Status::SummarizeFailed | Status::Transcribed => true,
                Status::Paused => transcript.exists(),
                _ => false,
            };
            if !waiting {
                return Err("요약을 기다리거나 요약에 실패한 회의가 아닙니다".into());
            }
            meta.llm_connection = Some(connection.name.clone());
            meta.summarize_local = connection.is_local;
            meta.wait = None;
            Ok(())
        })
    }

    /// Architecture 8.3 delete procedure, for a meeting no work runs on.
    pub fn delete(&self, id: &str) -> Result<(), String> {
        self.delete_with(id, || {})
    }

    /// Architecture 8.3 delete procedure. `cancel` is steps 2–3: stop the meeting's work and
    /// return once it has stopped. It runs after `deleting` is written, so nothing restarts it.
    pub fn delete_with(&self, id: &str, cancel: impl FnOnce()) -> Result<(), String> {
        let lock = self.lock_for(id)?;
        {
            let _guard = lock.lock().unwrap();
            let mut meta = match self.read(id) {
                Ok(meta) => meta,
                // Already gone: a delete of the same meeting finished first.
                Err(_) if !self.dir_for(id)?.join("meta.json").exists() => {
                    self.locks.lock().unwrap().remove(id);
                    return Ok(());
                }
                Err(e) => return Err(e),
            };
            if meta.schema_version > SCHEMA_VERSION {
                return Err(NEWER_VERSION.to_string());
            }
            if meta.status != Status::Deleting {
                let from = meta.status;
                meta.status = Status::Deleting;
                self.write(&meta)?;
                log::info!("상태 ({id}): {from:?} -> Deleting");
            }
        }
        // Outside the lock: the work being stopped may itself be waiting to write meta.json.
        cancel();
        let _guard = lock.lock().unwrap();
        match remove_meeting_dir(&self.dir_for(id)?) {
            // A second delete of the same meeting: the first one has removed it meanwhile.
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            removed => removed.map_err(|e| format!("회의 폴더 삭제 실패: {e}"))?,
        }
        self.locks.lock().unwrap().remove(id);
        Ok(())
    }

    /// Startup scan (architecture 8.4). Run once, before anything else touches meetings.
    pub fn recover(&self) {
        // Delete step 4 can stop after meta.json is gone, leaving an empty folder. `remove_dir`
        // only removes an empty folder, so one that still holds files is never touched.
        // An upload's temp file is a crash's leftover too (REC-12).
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(IMPORT_PREFIX)
                {
                    let _ = fs::remove_file(entry.path());
                } else {
                    let _ = fs::remove_dir(entry.path());
                }
            }
        }
        for meta in self.list() {
            if meta.schema_version > SCHEMA_VERSION {
                continue;
            }
            let result = match classify(meta.status) {
                Recovery::Interrupted => {
                    self.interrupted.lock().unwrap().insert(meta.id.clone());
                    Ok(())
                }
                Recovery::FinishDelete => self.delete(&meta.id),
                // Write the `paused` that exit handling (8.5) would have written, so nothing on
                // disk claims work that is not running. The queue picks it up after this.
                Recovery::Requeue
                    if matches!(meta.status, Status::Transcribing | Status::Summarizing) =>
                {
                    self.set_status(&meta.id, Status::Paused).map(|_| ())
                }
                Recovery::Requeue | Recovery::Nothing => Ok(()),
            };
            if let Err(e) = result {
                log::error!("복구 실패 ({}): {e}", meta.id);
            }
        }
    }

    /// The recorder closed both WAV files: `recording` → `recorded`, with length and sleep gaps.
    pub fn finish_recording(
        &self,
        id: &str,
        duration_ms: u64,
        gaps: Vec<Gap>,
    ) -> Result<Meta, String> {
        self.update(id, |meta| {
            meta.duration_ms = duration_ms;
            meta.gaps = gaps;
            // Exit handling (8.5) may have written `recorded` already.
            if meta.status == Status::Recording {
                transition(meta, Status::Recorded)?;
            }
            Ok(())
        })
    }

    /// A `recording` meeting found at startup: nothing records it in this process (STO-03).
    pub fn is_interrupted(&self, id: &str) -> bool {
        self.interrupted.lock().unwrap().contains(id)
    }

    /// A recording whose end could not be written: treated like one found at startup.
    pub fn mark_interrupted(&self, id: &str) {
        self.interrupted.lock().unwrap().insert(id.to_string());
    }

    /// "처리 계속" for an interrupted recording (STO-03). The WAV files keep what their last
    /// 5 s flush wrote. The caller queues the now `recorded` meeting.
    pub fn continue_interrupted(&self, id: &str) -> Result<Meta, String> {
        if !self.is_interrupted(id) {
            return Err(format!("중단된 녹음이 아닙니다: {id}"));
        }
        let meta = self.set_status(id, Status::Recorded)?;
        self.interrupted.lock().unwrap().remove(id);
        Ok(meta)
    }

    /// Meetings with work running in this process (recording, transcribing, summarizing).
    pub fn busy_ids(&self) -> Vec<String> {
        let interrupted = self.interrupted.lock().unwrap();
        self.list()
            .into_iter()
            .filter(|m| {
                m.schema_version <= SCHEMA_VERSION
                    && matches!(
                        m.status,
                        Status::Recording | Status::Transcribing | Status::Summarizing
                    )
                    && !interrupted.contains(&m.id)
            })
            .map(|m| m.id)
            .collect()
    }

    /// Exit handling (architecture 8.5): recording → recorded, processing → paused.
    pub fn pause_busy(&self) {
        for id in self.busy_ids() {
            let result = self.update(&id, |meta| {
                let to = if meta.status == Status::Recording {
                    Status::Recorded
                } else {
                    Status::Paused
                };
                transition(meta, to)
            });
            if let Err(e) = result {
                log::error!("종료 처리 실패 ({id}): {e}");
            }
        }
    }
}

/// Delete step 4. `meta.json` (status `deleting`) goes last: if this stops midway, the meeting
/// still lists as deleting and startup recovery finishes the job (8.4). `remove_dir_all` gives
/// no order, so on NTFS it could drop meta.json first and strand the recordings.
fn remove_meeting_dir(dir: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name() == "meta.json" {
            continue;
        }
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    fs::remove_file(dir.join("meta.json"))?;
    // Without meta.json the meeting is gone; `recover` removes the empty folder next start.
    if let Err(e) = fs::remove_dir(dir) {
        log::warn!("빈 회의 폴더 삭제 실패 ({}): {e}", dir.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use Status::*;

    const ALL: [Status; 13] = [
        Recording,
        Recorded,
        Transcribing,
        Transcribed,
        Summarizing,
        Review,
        Sent,
        Archived,
        Paused,
        TranscribeFailed,
        SummarizeFailed,
        Deleting,
        Unknown,
    ];

    fn store() -> (tempfile::TempDir, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("meetings")).unwrap();
        (tmp, store)
    }

    /// Writes a meeting in the given state directly, bypassing transitions.
    fn put(store: &Store, status: Status) -> Meta {
        let meta = store.create("테스트 회의", false).unwrap();
        let meta = Meta { status, ..meta };
        store.write(&meta).unwrap();
        meta
    }

    fn meta_path(store: &Store, id: &str) -> PathBuf {
        store.dir.join(id).join("meta.json")
    }

    #[test]
    fn failed_atomic_write_leaves_no_temp_file() {
        let tmp = tempfile::tempdir().unwrap();
        // Renaming a file over a non-empty directory fails on every OS.
        let target = tmp.path().join("meta.json");
        fs::create_dir_all(target.join("blocker")).unwrap();
        assert!(write_atomic(&target, b"{}").is_err());
        assert!(!tmp.path().join("meta.json.tmp").exists());
    }

    #[test]
    fn transitions_follow_the_state_diagram() {
        let valid = [
            (Recording, Recorded),
            (Recorded, Transcribing),
            (Transcribing, Transcribed),
            (Transcribing, Paused),
            (Transcribing, TranscribeFailed),
            (TranscribeFailed, Transcribing),
            (Paused, Transcribing),
            (Transcribed, Summarizing),
            (Summarizing, Review),
            (Summarizing, Paused),
            (Summarizing, SummarizeFailed),
            (SummarizeFailed, Summarizing),
            (Paused, Summarizing),
            (Review, Sent),
            (Review, Archived),
        ];
        for from in ALL {
            for to in ALL {
                let expected = valid.contains(&(from, to)) || (to == Deleting && from != Deleting);
                assert_eq!(can_transition(from, to), expected, "{from:?} -> {to:?}");
            }
        }
    }

    #[test]
    fn set_status_rejects_invalid_transition_and_keeps_file() {
        let (_tmp, store) = store();
        let meta = put(&store, Recording);
        let before = fs::read(meta_path(&store, &meta.id)).unwrap();
        assert!(store.set_status(&meta.id, Review).is_err());
        assert_eq!(fs::read(meta_path(&store, &meta.id)).unwrap(), before);
        assert_eq!(
            store.set_status(&meta.id, Recorded).unwrap().status,
            Recorded
        );
        assert_eq!(store.get(&meta.id).unwrap().status, Recorded);
    }

    #[test]
    fn concurrent_updates_of_different_fields_both_persist() {
        let (_tmp, store) = store();
        let id = put(&store, Transcribing).id;
        std::thread::scope(|s| {
            s.spawn(|| {
                for _ in 0..50 {
                    store
                        .update(&id, |m| {
                            m.chunks_done += 1;
                            Ok(())
                        })
                        .unwrap();
                }
            });
            s.spawn(|| {
                for _ in 0..50 {
                    store
                        .update(&id, |m| {
                            m.duration_ms += 1;
                            Ok(())
                        })
                        .unwrap();
                }
            });
        });
        let meta = store.get(&id).unwrap();
        assert_eq!((meta.chunks_done, meta.duration_ms), (50, 50));
    }

    #[test]
    fn newer_schema_is_read_only() {
        let (_tmp, store) = store();
        let meta = put(&store, Review);
        let newer = Meta {
            schema_version: SCHEMA_VERSION + 1,
            ..meta
        };
        store.write(&newer).unwrap();
        let before = fs::read(meta_path(&store, &newer.id)).unwrap();

        assert_eq!(
            store.get(&newer.id).unwrap().schema_version,
            SCHEMA_VERSION + 1
        );
        assert_eq!(
            store
                .update(&newer.id, |m| {
                    m.title = "x".into();
                    Ok(())
                })
                .unwrap_err(),
            NEWER_VERSION
        );
        assert_eq!(
            store.set_status(&newer.id, Sent).unwrap_err(),
            NEWER_VERSION
        );
        assert_eq!(store.delete(&newer.id).unwrap_err(), NEWER_VERSION);
        store.pause_busy();
        assert_eq!(fs::read(meta_path(&store, &newer.id)).unwrap(), before);
    }

    #[test]
    fn newer_schema_never_counts_as_busy() {
        let (_tmp, store) = store();
        let meta = put(&store, Recording);
        store
            .write(&Meta {
                schema_version: SCHEMA_VERSION + 1,
                ..meta
            })
            .unwrap();
        assert!(store.busy_ids().is_empty());
    }

    #[test]
    fn atomic_write_leaves_parseable_file_and_no_temp() {
        let (_tmp, store) = store();
        let id = put(&store, Recorded).id;
        let path = meta_path(&store, &id);
        // A leftover temp file from an earlier crash must not matter.
        fs::write(path.with_extension("json.tmp"), b"{ torn").unwrap();
        store
            .update(&id, |m| {
                m.title = "새 제목".into();
                Ok(())
            })
            .unwrap();

        let parsed: Meta = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(parsed.title, "새 제목");
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn recovery_classification_covers_every_status() {
        // Architecture 8.4, one row per status.
        let expected = [
            (Recording, Recovery::Interrupted),
            (Recorded, Recovery::Requeue),
            (Transcribing, Recovery::Requeue),
            (Transcribed, Recovery::Requeue),
            (Summarizing, Recovery::Requeue),
            (Review, Recovery::Nothing),
            (Sent, Recovery::Nothing),
            (Archived, Recovery::Nothing),
            (Paused, Recovery::Requeue),
            (TranscribeFailed, Recovery::Nothing),
            (SummarizeFailed, Recovery::Nothing),
            (Deleting, Recovery::FinishDelete),
            (Unknown, Recovery::Nothing),
        ];
        assert_eq!(
            expected
                .iter()
                .map(|(status, _)| *status)
                .collect::<Vec<_>>(),
            ALL
        );
        for (status, recovery) in expected {
            assert_eq!(classify(status), recovery, "{status:?}");
        }
    }

    #[test]
    fn newer_meta_with_unknown_status_or_shape_stays_listed_read_only() {
        let (_tmp, store) = store();
        let unknown_status = put(&store, Review).id;
        let changed_shape = put(&store, Review).id;
        let future = |id: &str, extra: &str| {
            format!(
                r#"{{"schema_version": 2, "id": "{id}", "title": "미래 회의", "created_at": "2026-10-13T05:00:03Z", {extra}}}"#
            )
        };
        fs::write(
            meta_path(&store, &unknown_status),
            future(&unknown_status, r#""status": "uploading", "new_field": 1"#),
        )
        .unwrap();
        fs::write(
            meta_path(&store, &changed_shape),
            future(&changed_shape, r#""status": "review", "gaps": {"v": 2}"#),
        )
        .unwrap();

        let listed = store.list();
        assert_eq!(listed.len(), 2);
        for meta in listed {
            assert_eq!((meta.schema_version, meta.title.as_str()), (2, "미래 회의"));
            assert_eq!(meta.status, Unknown);
            assert_eq!(
                store.set_status(&meta.id, Deleting).unwrap_err(),
                NEWER_VERSION
            );
        }
    }

    #[test]
    fn summary_falls_back_only_when_missing() {
        let (_tmp, store) = store();
        let id = put(&store, Review).id;
        let dir = store.dir.join(&id);
        assert_eq!(store.summary(&id).unwrap(), "");
        fs::write(dir.join("summary.generated.md"), "생성본").unwrap();
        assert_eq!(store.summary(&id).unwrap(), "생성본");
        // The generated title line is `meta.title`, not part of the body.
        fs::write(
            dir.join("summary.generated.md"),
            "# 주간 회의\n\n## 결정사항\n- 없음",
        )
        .unwrap();
        assert_eq!(store.summary(&id).unwrap(), "## 결정사항\n- 없음");
        // An unreadable summary.md must not silently show the generated text instead.
        fs::create_dir(dir.join("summary.md")).unwrap();
        assert!(store.summary(&id).is_err());
    }

    #[test]
    fn saving_a_summary_keeps_the_generated_one_and_works_only_in_review() {
        let (_tmp, store) = store();
        let id = put(&store, Review).id;
        let generated = store.dir.join(&id).join("summary.generated.md");
        fs::write(&generated, "# 생성 제목\n- 생성본").unwrap();
        let meta = store.save_summary(&id, "고친 제목", "- 고친 본문").unwrap();
        assert_eq!(meta.title, "고친 제목");
        assert_eq!(store.summary(&id).unwrap(), "- 고친 본문");
        assert_eq!(
            fs::read_to_string(&generated).unwrap(),
            "# 생성 제목\n- 생성본"
        );
        // One line, however it was typed or pasted.
        let meta = store
            .save_summary(&id, " 주간\r\n회의\t\u{7}정리  ", "- 고친 본문")
            .unwrap();
        assert_eq!(meta.title, "주간 회의 정리");

        // REV-03 "전송 안 함" ends the review; nothing is saved after that.
        assert_eq!(store.set_status(&id, Archived).unwrap().status, Archived);
        assert!(store.save_summary(&id, "x", "y").is_err());
        assert_eq!(store.summary(&id).unwrap(), "- 고친 본문");
        let transcribed = put(&store, Transcribed).id;
        assert!(store.set_status(&transcribed, Archived).is_err());
    }

    #[test]
    fn summarize_local_changes_only_until_summarizing_starts() {
        let (_tmp, store) = store();
        let id = put(&store, Transcribed).id;
        assert!(
            store
                .set_summarize_local(&id, true)
                .unwrap()
                .summarize_local
        );
        store
            .update(&id, |m| {
                m.llm_connection = Some("내 Ollama".into());
                Ok(())
            })
            .unwrap();
        assert!(store.set_summarize_local(&id, false).is_err());
        let review = put(&store, Review).id;
        assert!(store.set_summarize_local(&review, true).is_err());
        assert!(!store.get(&review).unwrap().summarize_local);
    }

    #[test]
    fn retry_summary_takes_the_picked_connection_only_after_a_failure() {
        let (_tmp, store) = store();
        let local = LlmConnection {
            name: "내 Ollama".into(),
            base_url: "http://localhost:11434/v1".into(),
            model: "m".into(),
            context_tokens: 8192,
            is_local: true,
            timeout_secs: 900,
        };
        let failed = put(&store, SummarizeFailed).id;
        let meta = store.retry_summary(&failed, &local).unwrap();
        assert_eq!(
            (meta.llm_connection.as_deref(), meta.summarize_local),
            (Some("내 Ollama"), true)
        );
        let review = put(&store, Review).id;
        assert!(store.retry_summary(&review, &local).is_err());

        // ENG15: a meeting waiting with its transcript takes another connection, and the reason
        // it waited goes.
        let waiting = Meta {
            wait: Some(Wait::ConnectionDeleted),
            llm_connection: Some("지운 연결".into()),
            ..put(&store, Transcribed)
        };
        store.write(&waiting).unwrap();
        let meta = store.retry_summary(&waiting.id, &local).unwrap();
        assert_eq!(
            (meta.llm_connection.as_deref(), meta.wait),
            (Some("내 Ollama"), None)
        );
        // `paused` only once it has a transcript: before that it still needs transcribing.
        let paused = put(&store, Paused).id;
        assert!(store.retry_summary(&paused, &local).is_err());
        fs::write(store.dir.join(&paused).join("transcript.json"), "{}").unwrap();
        assert!(store.retry_summary(&paused, &local).is_ok());
        let recorded = put(&store, Recorded).id;
        assert!(store.retry_summary(&recorded, &local).is_err());
    }

    #[test]
    fn an_m3_meta_reads_without_no_speech() {
        let (_tmp, store) = store();
        let id = put(&store, Transcribed).id;
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(meta_path(&store, &id)).unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("no_speech");
        fs::write(meta_path(&store, &id), value.to_string()).unwrap();
        assert!(!store.get(&id).unwrap().no_speech);
    }

    #[test]
    fn lock_registry_rejects_bad_ids_and_forgets_deleted_meetings() {
        let (_tmp, store) = store();
        assert!(store.update("../x", |_| Ok(())).is_err());
        assert!(store.delete("a/b").is_err());
        assert!(store.locks.lock().unwrap().is_empty());

        let id = put(&store, Review).id;
        store.set_status(&id, Sent).unwrap();
        assert!(store.locks.lock().unwrap().contains_key(&id));
        store.delete(&id).unwrap();
        assert!(store.locks.lock().unwrap().is_empty());
    }

    /// A delete that fails midway keeps meta.json (`deleting`), so the meeting stays listed and
    /// startup recovery removes the leftover recordings.
    #[cfg(unix)]
    #[test]
    fn interrupted_delete_keeps_meta_until_recovery_finishes() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, store) = store();
        let id = put(&store, Review).id;
        let dir = store.dir.join(&id);
        fs::write(dir.join("mic.wav"), b"RIFF").unwrap();
        let stuck = dir.join("stuck");
        fs::create_dir(&stuck).unwrap();
        fs::write(stuck.join("chunk"), b"x").unwrap();
        fs::set_permissions(&stuck, fs::Permissions::from_mode(0o555)).unwrap();

        assert!(store.delete(&id).is_err());
        assert_eq!(store.get(&id).unwrap().status, Deleting);

        fs::set_permissions(&stuck, fs::Permissions::from_mode(0o755)).unwrap();
        store.recover();
        assert!(!dir.exists());
    }

    /// Delete step 4 stopping after meta.json: the delete still succeeds, and startup recovery
    /// removes the empty folder but never a folder that still holds files.
    #[cfg(unix)]
    #[test]
    fn recovery_removes_only_empty_meeting_folders() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, store) = store();
        let id = put(&store, Review).id;
        let orphan = store.dir.join("2026-01-01T0000-ffff");
        fs::create_dir(&orphan).unwrap();
        fs::write(orphan.join("mic.wav"), b"RIFF").unwrap();

        // A read-only meetings/ lets files inside the meeting go but not the folder itself.
        fs::set_permissions(&store.dir, fs::Permissions::from_mode(0o555)).unwrap();
        let deleted = store.delete(&id);
        fs::set_permissions(&store.dir, fs::Permissions::from_mode(0o755)).unwrap();
        deleted.unwrap();
        assert!(store.dir.join(&id).exists());
        assert!(store.list().is_empty());

        store.recover();
        assert!(!store.dir.join(&id).exists());
        assert!(orphan.join("mic.wav").exists());
    }

    #[test]
    fn recovery_removes_an_upload_a_crash_left_half_decoded() {
        let (_tmp, store) = store();
        let temp = store.import_temp();
        fs::write(&temp, b"RIFF").unwrap();
        let kept = put(&store, Review).id;
        store.recover();
        assert!(!temp.exists());
        assert!(store.dir.join(&kept).join("meta.json").exists());
    }

    #[test]
    fn copied_folder_acts_on_itself_not_on_the_original() {
        let (_tmp, store) = store();
        let original = put(&store, Review).id;
        let copy = "2026-01-01T0000-ffff";
        fs::create_dir(store.dir.join(copy)).unwrap();
        fs::copy(meta_path(&store, &original), meta_path(&store, copy)).unwrap();
        let before = fs::read(meta_path(&store, &original)).unwrap();

        assert_eq!(store.get(copy).unwrap().id, copy);
        store
            .update(copy, |m| {
                m.title = "사본".into();
                Ok(())
            })
            .unwrap();
        assert_eq!(store.get(copy).unwrap().title, "사본");
        store.delete(copy).unwrap();
        assert!(!store.dir.join(copy).exists());
        assert_eq!(fs::read(meta_path(&store, &original)).unwrap(), before);
    }

    #[test]
    fn update_rejects_deleting_and_missing_meetings() {
        let (_tmp, store) = store();
        let id = put(&store, Deleting).id;
        let before = fs::read(meta_path(&store, &id)).unwrap();
        assert!(
            store
                .update(&id, |m| {
                    m.chunks_done = 9;
                    Ok(())
                })
                .is_err()
        );
        assert_eq!(fs::read(meta_path(&store, &id)).unwrap(), before);

        assert!(store.update("2026-01-01T0000-ffff", |_| Ok(())).is_err());
        assert!(!store.dir.join("2026-01-01T0000-ffff").exists());
    }

    #[test]
    fn deleting_a_deleted_meeting_succeeds() {
        let (_tmp, store) = store();
        let id = put(&store, Review).id;
        store.delete(&id).unwrap();
        store.delete(&id).unwrap();
        assert!(store.locks.lock().unwrap().is_empty());
        // A broken meta.json is not "gone".
        let broken = put(&store, Review).id;
        fs::write(meta_path(&store, &broken), b"{ torn").unwrap();
        assert!(store.delete(&broken).is_err());
    }

    #[test]
    fn delete_removes_folder_from_any_state() {
        let (_tmp, store) = store();
        let id = put(&store, Summarizing).id;
        store.delete(&id).unwrap();
        assert!(!store.dir.join(&id).exists());
        assert!(store.get(&id).is_err());
    }

    #[test]
    fn ids_cannot_escape_the_meetings_folder() {
        let (tmp, store) = store();
        fs::create_dir(tmp.path().join("victim")).unwrap();
        assert!(store.delete("../victim").is_err());
        assert!(store.get("..").is_err());
        assert!(tmp.path().join("victim").exists());
    }

    #[test]
    fn startup_recovery_acts_on_each_class() {
        let (_tmp, store) = store();
        let interrupted = put(&store, Recording).id;
        let deleting = put(&store, Deleting).id;
        let transcribing = put(&store, Transcribing).id;
        let summarizing = put(&store, Summarizing).id;
        let recorded = put(&store, Recorded).id;

        store.recover();

        assert_eq!(store.get(&interrupted).unwrap().status, Recording);
        assert!(!store.dir.join(&deleting).exists());
        assert_eq!(store.get(&transcribing).unwrap().status, Paused);
        assert_eq!(store.get(&summarizing).unwrap().status, Paused);
        assert_eq!(store.get(&recorded).unwrap().status, Recorded);
        // The interrupted recording is not running, so quitting needs no confirmation.
        assert!(store.busy_ids().is_empty());
    }

    /// STO-03: a meeting left in `recording` by a crash is offered as interrupted; "처리 계속"
    /// moves it to `recorded`. A recording started in this process is never interrupted.
    #[test]
    fn interrupted_recording_can_continue_or_be_deleted() {
        let (_tmp, store) = store();
        let crashed = put(&store, Recording).id;
        let crashed_too = put(&store, Recording).id;
        store.recover();
        let live = store.create("", false).unwrap().id;

        assert!(store.is_interrupted(&crashed));
        assert!(!store.is_interrupted(&live));
        assert!(store.continue_interrupted(&live).is_err());
        assert_eq!(store.get(&live).unwrap().status, Recording);
        assert_eq!(store.busy_ids(), [live]);

        assert_eq!(
            store.continue_interrupted(&crashed).unwrap().status,
            Recorded
        );
        assert!(!store.is_interrupted(&crashed));
        assert!(store.continue_interrupted(&crashed).is_err());

        store.delete(&crashed_too).unwrap();
        assert!(store.get(&crashed_too).is_err());
    }

    #[test]
    fn finish_recording_writes_length_and_gaps_even_after_exit_handling() {
        let (_tmp, store) = store();
        let id = put(&store, Recording).id;
        // RunEvent::Exit can write `recorded` before the writer thread gets there.
        store.pause_busy();
        let gap = Gap {
            at_ms: 1_000,
            len_ms: 60_000,
            reason: "sleep".into(),
        };
        let meta = store
            .finish_recording(&id, 90_000, vec![gap.clone()])
            .unwrap();
        assert_eq!(
            (meta.status, meta.duration_ms, meta.gaps),
            (Recorded, 90_000, vec![gap])
        );
    }

    #[test]
    fn pause_busy_writes_exit_states() {
        let (_tmp, store) = store();
        let recording = put(&store, Recording).id;
        let summarizing = put(&store, Summarizing).id;
        let review = put(&store, Review).id;

        let mut busy = store.busy_ids();
        busy.sort();
        let mut expected = vec![recording.clone(), summarizing.clone()];
        expected.sort();
        assert_eq!(busy, expected);

        store.pause_busy();
        assert_eq!(store.get(&recording).unwrap().status, Recorded);
        assert_eq!(store.get(&summarizing).unwrap().status, Paused);
        assert_eq!(store.get(&review).unwrap().status, Review);
        assert!(store.busy_ids().is_empty());
    }
}
