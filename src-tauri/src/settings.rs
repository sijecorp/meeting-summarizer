use crate::store::{NEWER_VERSION, write_atomic};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

/// `settings.json`'s own version, apart from `meta.json`'s (`store::SCHEMA_VERSION`).
const SCHEMA_VERSION: u32 = 1;

const KEYRING_SERVICE: &str = "com.sije.meeting-summarizer";
const GLOSSARY_MAX: usize = 100;
/// The built-in route's connection name (plan 5.4): `meta.llm_connection` and `list_connections`,
/// never a saved connection's. The screens call it `BUILTIN_LABEL`, which no saved one may use
/// either (d6).
pub const BUILTIN: &str = "builtin";
pub const BUILTIN_LABEL: &str = "이 PC";

/// A name only the built-in route may have (d6). A saved connection named so is left out of every
/// route until renamed; there is no migration.
pub fn reserved(name: &str) -> bool {
    matches!(name.trim(), BUILTIN | BUILTIN_LABEL)
}

/// The built-in route as a connection (plan 5.4, 5.6): the sidecar gives its address at run time.
pub fn builtin() -> LlmConnection {
    LlmConnection {
        name: BUILTIN.into(),
        base_url: String::new(),
        model: crate::transcribe::model::SUMMARY.file.into(),
        context_tokens: crate::sidecar::CONTEXT_TOKENS,
        is_local: true,
        timeout_secs: crate::sidecar::TIMEOUT_SECS,
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Auto,
    Gpu,
    Cpu,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LlmConnection {
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub context_tokens: u32,
    pub is_local: bool,
    /// Whole-response limit (architecture 6.1). Missing in the file means the default:
    /// 300 cloud / 900 local. Saving 0 is rejected.
    #[serde(default)]
    pub timeout_secs: u32,
}

/// settings.json (architecture 9.1). No secrets here; those live in the keychain.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    pub recorder_name: String,
    pub backend: Backend,
    pub gpu_failed: bool,
    /// The summary engine's GPU died of its own fault and the CPU then worked (plan 5.3): automatic
    /// mode runs llama-server on the CPU. Written by the sidecar the way `gpu_failed` is by the queue.
    pub summary_gpu_failed: bool,
    /// The app version whose first launch cleared `gpu_failed` (plan 5.1-10).
    pub gpu_reset_version: Option<String>,
    /// Whisper's flash attention, the worker's `--fa` (plan 5.1-6). No screen sets it: only an
    /// edit of settings.json, which a save keeps, and the queue, which turns it off for good after
    /// a GPU crash with it on.
    pub transcribe_flash_attn: bool,
    pub model: String,
    pub model_base_url: String,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub llm_connections: Vec<LlmConnection>,
    pub default_llm: Option<String>,
    pub summarize_local_by_default: bool,
    /// The signed-in Slack workspace's name (ONB-04), written by signing in and out (slack.rs); its
    /// token is in the keychain. An older file's `webhooks` are dropped on the next save.
    pub slack_team: Option<String>,
    /// The channel id last sent to (SND-04).
    pub last_channel: Option<String>,
    pub glossary: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            recorder_name: std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_default(),
            backend: Backend::Auto,
            gpu_failed: false,
            summary_gpu_failed: false,
            gpu_reset_version: None,
            transcribe_flash_attn: true,
            model: "large-v3-turbo-q8_0".into(),
            model_base_url: "https://huggingface.co".into(),
            input_device: None,
            output_device: None,
            llm_connections: Vec::new(),
            default_llm: None,
            summarize_local_by_default: false,
            slack_team: None,
            last_channel: None,
            glossary: Vec::new(),
        }
    }
}

impl Settings {
    /// Every connection a summary can take (plan 5.4): the saved ones less any with a reserved
    /// name, then the built-in one. `llm_connections` (and so `get_settings`/`save_settings`) never
    /// holds it: the settings screen would save it back and every save would be refused.
    pub fn connections(&self) -> Vec<LlmConnection> {
        let saved = self.llm_connections.iter().filter(|c| !reserved(&c.name));
        saved.cloned().chain([builtin()]).collect()
    }

    /// The saved connection `default_llm` names. `None` is the built-in route (plan 5.4): no
    /// default, a name no saved connection has, or a reserved one. The route and the model
    /// download's scope both ask this one.
    pub fn default_connection(&self) -> Option<&LlmConnection> {
        let name = self.default_llm.as_deref()?;
        let named = |c: &&LlmConnection| c.name == name && !reserved(&c.name);
        self.llm_connections.iter().find(named)
    }
}

fn fill_timeouts(settings: &mut Settings) {
    for c in &mut settings.llm_connections {
        if c.timeout_secs == 0 {
            c.timeout_secs = if c.is_local { 900 } else { 300 };
        }
    }
}

fn validate(settings: &Settings) -> Result<(), String> {
    let mut seen = HashSet::new();
    for c in &settings.llm_connections {
        let name = &c.name;
        if name.trim().is_empty() || !seen.insert(name) {
            return Err(format!("LLM 연결 이름이 비었거나 중복됩니다: {name}"));
        }
        if c.context_tokens == 0 {
            return Err(format!("컨텍스트 토큰은 1 이상이어야 합니다: {name}"));
        }
        if c.timeout_secs == 0 {
            return Err(format!("응답 시간 제한은 1초 이상이어야 합니다: {name}"));
        }
    }
    if settings.glossary.len() > GLOSSARY_MAX {
        return Err(format!("용어집은 {GLOSSARY_MAX}개까지 넣을 수 있습니다"));
    }
    Ok(())
}

#[derive(Serialize, Debug)]
pub struct LoadedSettings {
    pub settings: Settings,
    /// settings.json came from a newer app: show it, refuse to save (STO-05).
    pub read_only: bool,
    /// Saved connections with a reserved name (d6): out of every route, and 저장 refuses them
    /// until they are renamed. The settings screen asks for that (stage 6).
    pub rename: Vec<String>,
}

impl LoadedSettings {
    fn new(settings: Settings, read_only: bool) -> Self {
        let rename = settings.llm_connections.iter().map(|c| &c.name);
        let rename = rename.filter(|n| reserved(n)).cloned().collect();
        Self {
            settings,
            read_only,
            rename,
        }
    }
}

pub struct SettingsStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl SettingsStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    pub fn load(&self) -> Result<LoadedSettings, String> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LoadedSettings::new(Settings::default(), false));
            }
            Err(e) => return Err(format!("settings.json을 읽을 수 없습니다: {e}")),
        };
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| format!("settings.json이 손상됐습니다: {e}"))?;
        let version = value["schema_version"]
            .as_u64()
            .unwrap_or(SCHEMA_VERSION.into());
        let parsed = serde_json::from_value::<Settings>(value);
        if version > SCHEMA_VERSION.into() {
            // Newer fields may not fit this version's types; fall back to defaults, still read-only.
            return Ok(LoadedSettings::new(parsed.unwrap_or_default(), true));
        }
        let mut settings = parsed.map_err(|e| format!("settings.json이 손상됐습니다: {e}"))?;
        fill_timeouts(&mut settings);
        Ok(LoadedSettings::new(settings, false))
    }

    pub fn save(&self, mut settings: Settings) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap();
        let stored = fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .unwrap_or_default();
        if stored["schema_version"]
            .as_u64()
            .is_some_and(|v| v > SCHEMA_VERSION.into())
        {
            return Err(NEWER_VERSION.to_string());
        }
        // d6: here and not in `validate`, so the app's own writes (`patch`) still work while a
        // connection saved before the update keeps such a name.
        if let Some(c) = settings.llm_connections.iter().find(|c| reserved(&c.name)) {
            return Err(format!(
                "'{}'은(는) 이 PC 요약이 쓰는 이름이라 연결 이름으로 쓸 수 없습니다. 이름을 바꿔 주세요",
                c.name.trim()
            ));
        }
        // One spelling for the built-in default: `builtin`, or a name nothing has, is stored `None`.
        if settings.default_connection().is_none() {
            settings.default_llm = None;
        }
        // `gpu_failed` belongs to the queue (architecture 5.4) and `summary_gpu_failed` to the
        // sidecar: a save from the settings screen keeps the stored values, and switching the
        // backend to "GPU" clears both (one device setting for both, plan 5.3).
        let gpu_chosen = settings.backend == Backend::Gpu && stored["backend"] != "gpu";
        let kept = |field: &str| stored[field].as_bool().unwrap_or(false) && !gpu_chosen;
        settings.gpu_failed = kept("gpu_failed");
        settings.summary_gpu_failed = kept("summary_gpu_failed");
        // `last_channel` belongs to sending (SND-04) the same way, `slack_team` to signing in, and
        // the two below to the app's start and to settings.json alone.
        settings.last_channel = stored["last_channel"].as_str().map(String::from);
        settings.slack_team = stored["slack_team"].as_str().map(String::from);
        settings.gpu_reset_version = stored["gpu_reset_version"].as_str().map(String::from);
        settings.transcribe_flash_attn = stored["transcribe_flash_attn"].as_bool().unwrap_or(true);
        self.write(settings)
    }

    /// One field written by the app itself, the rest as stored.
    fn patch(&self, f: impl FnOnce(&mut Settings)) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap();
        let loaded = self.load()?;
        if loaded.read_only {
            return Err(NEWER_VERSION.to_string());
        }
        let mut settings = loaded.settings;
        f(&mut settings);
        self.write(settings)
    }

    /// The GPU crashed twice on one chunk: automatic mode uses the CPU from now on (TRN-06).
    pub fn mark_gpu_failed(&self) -> Result<(), String> {
        self.patch(|s| s.gpu_failed = true)
    }

    /// A GPU worker crashed with flash attention on (plan 5.1-6). It stays off until settings.json
    /// is edited; `reset_gpu_failed_once` leaves it alone.
    pub fn turn_off_flash_attn(&self) -> Result<(), String> {
        self.patch(|s| s.transcribe_flash_attn = false)
    }

    /// Plan 5.1-10: the first launch of `version` clears `gpu_failed` once, so a PC an older
    /// version tied to the CPU tries this version's GPU handling. A mark made after that stays.
    pub fn reset_gpu_failed_once(&self, version: &str) -> Result<(), String> {
        let loaded = self.load()?;
        if loaded.read_only || loaded.settings.gpu_reset_version.as_deref() == Some(version) {
            return Ok(());
        }
        self.patch(|s| {
            s.gpu_failed = false;
            s.gpu_reset_version = Some(version.into());
        })
    }

    /// The summary engine's GPU failed and its CPU worked (plan 5.3).
    pub fn mark_summary_gpu_failed(&self) -> Result<(), String> {
        self.patch(|s| s.summary_gpu_failed = true)
    }

    /// SND-04: a send went to this channel, so it is the next default.
    pub fn set_last_channel(&self, id: &str) -> Result<(), String> {
        self.patch(|s| s.last_channel = Some(id.to_string()))
    }

    /// Signed in to `team`, or signed out with `None`: the last channel goes with the workspace.
    pub fn set_slack_team(&self, team: Option<&str>) -> Result<(), String> {
        self.patch(|s| {
            if s.slack_team.as_deref() != team {
                s.last_channel = None;
            }
            s.slack_team = team.map(String::from);
        })
    }

    fn write(&self, mut settings: Settings) -> Result<(), String> {
        validate(&settings)?;
        settings.schema_version = SCHEMA_VERSION;
        let bytes = serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?;
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("설정 폴더 생성 실패: {e}"))?;
        }
        write_atomic(&self.path, &bytes).map_err(|e| format!("settings.json 저장 실패: {e}"))
    }

    /// "기본값으로 다시 시작" for an unreadable settings.json: keep it as
    /// `settings.json.bak-<unix seconds>` and start from defaults.
    pub fn reset(&self) -> Result<LoadedSettings, String> {
        let _guard = self.lock.lock().unwrap();
        if self.load().is_ok() {
            return Err("설정 파일에 문제가 없어 초기화하지 않았습니다".into());
        }
        let mut backup = self.path.as_os_str().to_owned();
        backup.push(format!(
            ".bak-{}",
            time::OffsetDateTime::now_utc().unix_timestamp()
        ));
        fs::rename(&self.path, &backup).map_err(|e| format!("손상된 설정 파일 백업 실패: {e}"))?;
        self.load()
    }
}

/// The webview's keys: an LLM connection's, by its name. The Slack token is Rust's alone
/// (`slack_tokens`).
#[derive(Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "lowercase")]
pub enum SecretKind {
    Llm,
}

fn keychain(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, account)
        .map_err(|e| format!("키체인을 열 수 없습니다: {e}"))
}

fn entry(kind: SecretKind, name: &str) -> Result<keyring::Entry, String> {
    if name.trim().is_empty() {
        return Err("이름이 비어 있습니다".into());
    }
    let SecretKind::Llm = kind;
    keychain(&format!("llm:{name}"))
}

/// The Slack sign-in's tokens (slack.rs), JSON. Never a command, like `get_secret`.
const SLACK_TOKENS: &str = "slack-auth";

pub fn slack_tokens() -> Result<Option<String>, String> {
    match keychain(SLACK_TOKENS)?.get_password() {
        Ok(tokens) => Ok(Some(tokens)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("키체인 읽기 실패: {e}")),
    }
}

/// Writes them, or deletes them with `None`.
pub fn set_slack_tokens(tokens: Option<&str>) -> Result<(), String> {
    let entry = keychain(SLACK_TOKENS)?;
    match tokens {
        Some(tokens) => entry
            .set_password(tokens)
            .map_err(|e| format!("키체인 저장 실패: {e}")),
        None => match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("키체인 삭제 실패: {e}")),
        },
    }
}

pub fn set_secret(kind: SecretKind, name: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("값이 비어 있습니다".into());
    }
    entry(kind, name)?
        .set_password(value)
        .map_err(|e| format!("키체인 저장 실패: {e}"))
}

/// Checks for the secret without reading it.
pub fn has_secret(kind: SecretKind, name: &str) -> Result<bool, String> {
    let entry = entry(kind, name)?;
    // macOS: an attributes-only search never decrypts the item, so it never shows a
    // Keychain access prompt. `get_password`/`get_attributes` both read the secret data.
    #[cfg(target_os = "macos")]
    {
        // Never search with empty values: Keychain treats those as wildcards.
        let (service, user) = entry
            .inner
            .get_specifiers()
            .ok_or("키체인 항목 이름을 알 수 없습니다")?;
        let spec = std::collections::HashMap::from([("service", &*service), ("user", &*user)]);
        keyring_core::Entry::search(&spec)
            .map(|found| !found.is_empty())
            .map_err(|e| format!("키체인 읽기 실패: {e}"))
    }
    // Windows Credential Manager does not prompt. Raw bytes, so a non-UTF-8 secret still counts.
    #[cfg(not(target_os = "macos"))]
    match entry.get_secret() {
        Ok(_) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(format!("키체인 읽기 실패: {e}")),
    }
}

/// The secret itself, for the LLM request in Rust. Never a command: secrets do not go back to the
/// webview (architecture 9.3), and never into a log.
pub fn get_secret(kind: SecretKind, name: &str) -> Result<Option<String>, String> {
    match entry(kind, name)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("키체인 읽기 실패: {e}")),
    }
}

pub fn delete_secret(kind: SecretKind, name: &str) -> Result<(), String> {
    match entry(kind, name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("키체인 삭제 실패: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, SettingsStore) {
        let tmp = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(tmp.path().join("settings.json"));
        (tmp, store)
    }

    fn connection(name: &str, is_local: bool) -> LlmConnection {
        LlmConnection {
            name: name.into(),
            base_url: "http://localhost:11434/v1".into(),
            model: "m".into(),
            context_tokens: 8000,
            is_local,
            timeout_secs: 300,
        }
    }

    #[test]
    fn missing_file_gives_defaults() {
        let (_tmp, store) = store();
        let loaded = store.load().unwrap();
        assert!(!loaded.read_only);
        assert_eq!(loaded.settings.schema_version, 1);
        assert_eq!(loaded.settings.backend, Backend::Auto);
        let os_user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_default();
        assert_eq!(loaded.settings.recorder_name, os_user);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let (_tmp, store) = store();
        let settings = Settings {
            recorder_name: "홍길동".into(),
            backend: Backend::Cpu,
            llm_connections: vec![connection("클라우드", false), connection("내 Ollama", true)],
            glossary: vec!["모놀리스".into()],
            ..Settings::default()
        };
        store.save(settings.clone()).unwrap();
        assert_eq!(store.load().unwrap().settings, settings);
    }

    #[test]
    fn missing_timeouts_load_as_defaults() {
        let (_tmp, store) = store();
        let file = r#"{"schema_version": 1, "llm_connections": [
            {"name": "c", "base_url": "u", "model": "m", "context_tokens": 8000, "is_local": false},
            {"name": "l", "base_url": "u", "model": "m", "context_tokens": 8000, "is_local": true}]}"#;
        fs::write(&store.path, file).unwrap();
        let timeouts: Vec<u32> = store
            .load()
            .unwrap()
            .settings
            .llm_connections
            .iter()
            .map(|c| c.timeout_secs)
            .collect();
        assert_eq!(timeouts, [300, 900]);
    }

    #[test]
    fn rejects_zero_context_tokens_and_timeout() {
        let (_tmp, store) = store();
        let with = |patch: fn(&mut LlmConnection)| {
            let mut c = connection("a", false);
            patch(&mut c);
            Settings {
                llm_connections: vec![c],
                ..Settings::default()
            }
        };
        let err = store.save(with(|c| c.context_tokens = 0)).unwrap_err();
        assert!(err.contains("컨텍스트 토큰"), "{err}");
        let err = store.save(with(|c| c.timeout_secs = 0)).unwrap_err();
        assert!(err.contains("응답 시간 제한"), "{err}");
        assert!(!store.path.exists());
    }

    #[test]
    fn reset_backs_up_a_corrupt_file_and_loads_defaults() {
        let (tmp, store) = store();
        fs::write(&store.path, b"{ torn").unwrap();
        assert!(store.load().is_err());

        let loaded = store.reset().unwrap();
        assert!(!loaded.read_only);
        assert_eq!(loaded.settings, Settings::default());
        assert!(!store.path.exists());
        let backups: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().contains("settings.json.bak-"))
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(&backups[0]).unwrap(), b"{ torn");

        // A readable file is never reset.
        store.save(Settings::default()).unwrap();
        assert!(store.reset().is_err());
        assert!(store.path.exists());
    }

    #[test]
    fn newer_schema_is_read_only_and_file_unchanged() {
        let (_tmp, store) = store();
        let newer = r#"{"schema_version": 2, "recorder_name": "미래", "backend": {"kind": "npu"}}"#
            .as_bytes();
        fs::write(&store.path, newer).unwrap();

        let loaded = store.load().unwrap();
        assert!(loaded.read_only);
        assert_eq!(store.save(Settings::default()).unwrap_err(), NEWER_VERSION);
        assert_eq!(fs::read(&store.path).unwrap(), newer);
    }

    #[test]
    fn gpu_failed_survives_settings_saves_until_gpu_is_chosen() {
        let (_tmp, store) = store();
        store.save(Settings::default()).unwrap();
        store.mark_gpu_failed().unwrap();
        assert!(store.load().unwrap().settings.gpu_failed);

        // The settings screen sends what it loaded earlier, before the queue marked the GPU.
        let stale = Settings {
            recorder_name: "홍길동".into(),
            ..Settings::default()
        };
        store.save(stale.clone()).unwrap();
        let loaded = store.load().unwrap().settings;
        assert!(loaded.gpu_failed);
        assert_eq!(loaded.recorder_name, "홍길동");

        store
            .save(Settings {
                backend: Backend::Gpu,
                gpu_failed: true,
                ..stale
            })
            .unwrap();
        assert!(!store.load().unwrap().settings.gpu_failed);
    }

    #[test]
    fn summary_gpu_failed_survives_settings_saves_until_gpu_is_chosen() {
        let (_tmp, store) = store();
        store.save(Settings::default()).unwrap();
        store.mark_summary_gpu_failed().unwrap();
        // The settings screen sends what it loaded before the sidecar marked the GPU.
        store.save(Settings::default()).unwrap();
        let loaded = store.load().unwrap().settings;
        assert!(loaded.summary_gpu_failed && !loaded.gpu_failed);
        store
            .save(Settings {
                backend: Backend::Gpu,
                summary_gpu_failed: true,
                ..Settings::default()
            })
            .unwrap();
        assert!(!store.load().unwrap().settings.summary_gpu_failed);
    }

    /// Plan 5.1-10: a 1.0.2 file's `gpu_failed` goes on 1.0.3's first launch only; a GPU that
    /// fails again in 1.0.3 stays failed through a settings save and the next launch.
    #[test]
    fn gpu_failed_is_cleared_once_by_a_new_version() {
        let (_tmp, store) = store();
        fs::write(&store.path, r#"{"schema_version": 1, "gpu_failed": true}"#).unwrap();
        store.reset_gpu_failed_once("1.0.3").unwrap();
        let loaded = store.load().unwrap().settings;
        assert!(!loaded.gpu_failed);
        assert_eq!(loaded.gpu_reset_version.as_deref(), Some("1.0.3"));

        store.mark_gpu_failed().unwrap();
        store.save(Settings::default()).unwrap();
        store.reset_gpu_failed_once("1.0.3").unwrap();
        assert!(store.load().unwrap().settings.gpu_failed);
    }

    /// Plan 5.1-6: on by default, and a hand edit survives the settings screen, which has no
    /// control for it.
    #[test]
    fn flash_attention_is_on_unless_settings_json_says_otherwise() {
        let (_tmp, store) = store();
        assert!(store.load().unwrap().settings.transcribe_flash_attn);
        fs::write(
            &store.path,
            r#"{"schema_version": 1, "transcribe_flash_attn": false}"#,
        )
        .unwrap();
        store
            .save(Settings {
                recorder_name: "홍길동".into(),
                ..Settings::default()
            })
            .unwrap();
        let loaded = store.load().unwrap().settings;
        assert!(!loaded.transcribe_flash_attn);
        assert_eq!(loaded.recorder_name, "홍길동");
    }

    #[test]
    fn a_send_and_a_sign_in_keep_their_fields_through_settings_saves() {
        let (_tmp, store) = store();
        let loaded_earlier = Settings {
            recorder_name: "홍길동".into(),
            ..Settings::default()
        };
        store.save(loaded_earlier.clone()).unwrap();
        store.set_slack_team(Some("SIJE")).unwrap();
        store.set_last_channel("C0123").unwrap();
        // The settings screen still holds what it loaded before the sign-in and the send.
        store.save(loaded_earlier).unwrap();
        let loaded = store.load().unwrap().settings;
        assert_eq!(loaded.last_channel.as_deref(), Some("C0123"));
        assert_eq!(loaded.slack_team.as_deref(), Some("SIJE"));
        assert_eq!(loaded.recorder_name, "홍길동");

        // Another workspace, or none: the channel was the old one's.
        store.set_slack_team(Some("SIJE")).unwrap();
        assert_eq!(
            store.load().unwrap().settings.last_channel.as_deref(),
            Some("C0123")
        );
        store.set_slack_team(None).unwrap();
        let loaded = store.load().unwrap().settings;
        assert_eq!((loaded.slack_team, loaded.last_channel), (None, None));
    }

    #[test]
    fn an_older_file_with_webhooks_still_loads() {
        let (_tmp, store) = store();
        fs::write(
            &store.path,
            r##"{"schema_version": 1, "webhooks": [{"name": "#회의록"}], "last_channel": "#회의록"}"##,
        )
        .unwrap();
        let loaded = store.load().unwrap().settings;
        assert_eq!(loaded.slack_team, None);
        store.save(loaded).unwrap();
        assert!(
            !fs::read_to_string(&store.path)
                .unwrap()
                .contains("webhooks")
        );
    }

    /// d6: the built-in route's names are refused on save; one saved before the update stays in
    /// the file, is flagged for renaming, never hides the built-in route, and the app's own writes
    /// go on.
    #[test]
    fn reserved_names_are_refused_and_never_shadow_the_builtin_route() {
        let (_tmp, store) = store();
        for name in ["builtin", "이 PC", " 이 PC "] {
            let settings = Settings {
                llm_connections: vec![connection(name, false)],
                ..Settings::default()
            };
            let err = store.save(settings).unwrap_err();
            assert!(err.contains("이름을 바꿔 주세요"), "{err}");
        }
        assert!(!store.path.exists());

        let file = r#"{"schema_version": 1, "llm_connections": [
            {"name": "builtin", "base_url": "https://api.example.com/v1", "model": "m", "context_tokens": 8000, "is_local": false},
            {"name": "내 Ollama", "base_url": "http://localhost:11434/v1", "model": "m", "context_tokens": 8000, "is_local": true}]}"#;
        fs::write(&store.path, file).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.rename, ["builtin"]);
        store.mark_gpu_failed().unwrap();
        let connections = store.load().unwrap().settings.connections();
        let names: Vec<_> = connections.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["내 Ollama", "builtin"]);
        let builtin = &connections[1];
        assert!(builtin.is_local && builtin.base_url.is_empty());
        // ENG6, plan 5.6: the built-in route's limits are the plan's, not a saved connection's.
        assert_eq!(
            (builtin.timeout_secs, builtin.context_tokens),
            (1_800, 16_384)
        );
    }

    /// Plan 5.4: one spelling for "this PC is the default": no name, `builtin`, a name nothing
    /// has, all stored `None`; a saved connection's name stays.
    #[test]
    fn a_default_that_names_no_saved_connection_is_stored_as_none() {
        let (_tmp, store) = store();
        let with = |default: &str| Settings {
            llm_connections: vec![connection("내 Ollama", true)],
            default_llm: Some(default.into()),
            ..Settings::default()
        };
        for builtin in ["builtin", "지운 연결"] {
            assert_eq!(with(builtin).default_connection(), None);
            store.save(with(builtin)).unwrap();
            assert_eq!(store.load().unwrap().settings.default_llm, None);
        }
        store.save(with("내 Ollama")).unwrap();
        let saved = store.load().unwrap().settings;
        assert_eq!(saved.default_llm.as_deref(), Some("내 Ollama"));
        assert_eq!(saved.default_connection().map(|c| c.is_local), Some(true));
    }

    #[test]
    fn rejects_duplicate_names() {
        let (_tmp, store) = store();
        let settings = Settings {
            llm_connections: vec![connection("a", false), connection("a", true)],
            ..Settings::default()
        };
        assert!(store.save(settings).is_err());
        assert!(!store.path.exists());
    }
}
