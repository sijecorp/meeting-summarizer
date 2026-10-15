use crate::store::{NEWER_VERSION, SCHEMA_VERSION, write_atomic};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

const KEYRING_SERVICE: &str = "com.sije.meeting-summarizer";
const GLOSSARY_MAX: usize = 100;

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

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Webhook {
    pub name: String,
}

/// settings.json (architecture 9.1). No secrets here; those live in the keychain.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    pub recorder_name: String,
    pub backend: Backend,
    pub gpu_failed: bool,
    pub model: String,
    pub model_base_url: String,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub llm_connections: Vec<LlmConnection>,
    pub default_llm: Option<String>,
    pub summarize_local_by_default: bool,
    pub webhooks: Vec<Webhook>,
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
            model: "large-v3-turbo-q8_0".into(),
            model_base_url: "https://huggingface.co".into(),
            input_device: None,
            output_device: None,
            llm_connections: Vec::new(),
            default_llm: None,
            summarize_local_by_default: false,
            webhooks: Vec::new(),
            last_channel: None,
            glossary: Vec::new(),
        }
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
    let mut seen = HashSet::new();
    for name in settings.webhooks.iter().map(|w| &w.name) {
        if name.trim().is_empty() || !seen.insert(name) {
            return Err(format!("웹훅 이름이 비었거나 중복됩니다: {name}"));
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
                return Ok(LoadedSettings {
                    settings: Settings::default(),
                    read_only: false,
                });
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
            return Ok(LoadedSettings {
                settings: parsed.unwrap_or_default(),
                read_only: true,
            });
        }
        let mut settings = parsed.map_err(|e| format!("settings.json이 손상됐습니다: {e}"))?;
        fill_timeouts(&mut settings);
        Ok(LoadedSettings {
            settings,
            read_only: false,
        })
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
        // `gpu_failed` belongs to the queue (architecture 5.4): a save from the settings screen
        // keeps the stored value, and switching the backend to "GPU" clears it.
        settings.gpu_failed = stored["gpu_failed"].as_bool().unwrap_or(false)
            && !(settings.backend == Backend::Gpu && stored["backend"] != "gpu");
        // `last_channel` belongs to sending (SND-04) the same way.
        settings.last_channel = stored["last_channel"].as_str().map(String::from);
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

    /// SND-04: a send went to this webhook, so it is the next default.
    pub fn set_last_channel(&self, name: &str) -> Result<(), String> {
        self.patch(|s| s.last_channel = Some(name.to_string()))
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

#[derive(Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "lowercase")]
pub enum SecretKind {
    Llm,
    Slack,
}

fn entry(kind: SecretKind, name: &str) -> Result<keyring::Entry, String> {
    if name.trim().is_empty() {
        return Err("이름이 비어 있습니다".into());
    }
    let prefix = match kind {
        SecretKind::Llm => "llm",
        SecretKind::Slack => "slack",
    };
    keyring::Entry::new(KEYRING_SERVICE, &format!("{prefix}:{name}"))
        .map_err(|e| format!("키체인을 열 수 없습니다: {e}"))
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
            webhooks: vec![Webhook {
                name: "#회의록".into(),
            }],
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
    fn a_send_sets_last_channel_and_settings_saves_keep_it() {
        let (_tmp, store) = store();
        let loaded_earlier = Settings {
            recorder_name: "홍길동".into(),
            ..Settings::default()
        };
        store.save(loaded_earlier.clone()).unwrap();
        store.set_last_channel("#회의록").unwrap();
        // The settings screen still holds what it loaded before the send.
        store.save(loaded_earlier).unwrap();
        let loaded = store.load().unwrap().settings;
        assert_eq!(loaded.last_channel.as_deref(), Some("#회의록"));
        assert_eq!(loaded.recorder_name, "홍길동");
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
