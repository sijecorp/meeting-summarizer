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
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub sent: Option<Sent>,
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
        let mut retries = 0;
        loop {
            match fs::rename(&tmp, path) {
                // Windows: antivirus or the search indexer can hold the target open for a moment.
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

    pub fn create(&self, title: &str) -> Result<Meta, String> {
        let now = time::OffsetDateTime::now_utc();
        // UTC: std/time cannot read the local offset safely; the UI formats it in local time.
        let id = format!(
            "{:04}-{:02}-{:02}T{:02}{:02}-{:04x}",
            now.year(),
            now.month() as u8,
            now.day(),
            now.hour(),
            now.minute(),
            (now.unix_timestamp_nanos() / 1000) as u16
        );
        let meta = Meta {
            schema_version: SCHEMA_VERSION,
            id: id.clone(),
            title: title.to_string(),
            status: Status::Recording,
            created_at: format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
                now.year(),
                now.month() as u8,
                now.day(),
                now.hour(),
                now.minute(),
                now.second()
            ),
            duration_ms: 0,
            gaps: Vec::new(),
            backend: None,
            chunks_done: 0,
            llm_connection: None,
            summarize_local: false,
            error: None,
            sent: None,
        };
        // create_dir (not _all) fails on an id collision instead of reusing a folder.
        fs::create_dir(self.dir_for(&id)?).map_err(|e| format!("회의 폴더 생성 실패: {e}"))?;
        self.write(&meta)?;
        Ok(meta)
    }

    pub fn get(&self, id: &str) -> Result<Meta, String> {
        self.read(id)
    }

    /// `summary.md` (edited) if present, else `summary.generated.md`, else empty.
    /// Only a missing file falls through; any other read error is reported.
    pub fn summary(&self, id: &str) -> Result<String, String> {
        let dir = self.dir_for(id)?;
        for name in ["summary.md", "summary.generated.md"] {
            match fs::read_to_string(dir.join(name)) {
                Ok(text) => return Ok(text),
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{name}을 읽을 수 없습니다: {e}")),
            }
        }
        Ok(String::new())
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
                    eprintln!("{e}");
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
        f(&mut meta)?;
        self.write(&meta)?;
        Ok(meta)
    }

    pub fn set_status(&self, id: &str, to: Status) -> Result<Meta, String> {
        self.update(id, |meta| transition(meta, to))
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
                meta.status = Status::Deleting;
                self.write(&meta)?;
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
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let _ = fs::remove_dir(entry.path());
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
                eprintln!("복구 실패 ({}): {e}", meta.id);
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
                eprintln!("종료 처리 실패 ({id}): {e}");
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
        eprintln!("빈 회의 폴더 삭제 실패 ({}): {e}", dir.display());
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
        let meta = store.create("테스트 회의").unwrap();
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
        // An unreadable summary.md must not silently show the generated text instead.
        fs::create_dir(dir.join("summary.md")).unwrap();
        assert!(store.summary(&id).is_err());
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
        let live = store.create("").unwrap().id;

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
