//! 設定（`Plugin/MediaCollect_H/settings.json`）。全体で 1 つ（プロジェクトごとではない）。
//!
//! ルール au2-plugin-checklist「設定ファイルの下位互換」: 足したキーは省略でき、知らないキーは無視する。
//! 読めなかったら上書きせず `.broken` へ退避し、退避もできなければその起動の間は保存しない。書くときは一時ファイルから置き換える。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 書き出し先の親のフォルダ
    pub out_dir: String,
    /// コピーしないフォルダ（中のファイルはパスをそのまま残す）
    pub excludes: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { out_dir: String::new(), excludes: default_excludes() }
    }
}

/// AviUtl2 のデータフォルダ。プラグインの起動時に `init_data_dir` で覚える（`app_data_path` は本体の外では使えない）
static DATA_DIR: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn init_data_dir() {
    let _ = DATA_DIR.set(aviutl2::config::app_data_path().display().to_string());
}

fn data_dir() -> String {
    DATA_DIR.get().cloned().unwrap_or_else(|| "C:\\ProgramData\\aviutl2".to_string())
}

/// 既定で外すフォルダ（ユーザーの決定、2026-10-10）: AviUtl2 のデータフォルダ・Windows・Program Files
pub fn default_excludes() -> Vec<String> {
    vec![
        format!("{}\\", data_dir().trim_end_matches('\\')),
        "C:\\Windows\\".into(),
        "C:\\Program Files\\".into(),
        "C:\\Program Files (x86)\\".into(),
    ]
}

fn path() -> PathBuf {
    PathBuf::from(data_dir()).join("Plugin").join("MediaCollect_H").join("settings.json")
}

/// (設定, 保存してよいか)
pub fn load() -> (Settings, bool) {
    let p = path();
    let text = match std::fs::read(&p) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Settings::default(), true),
        Err(e) => {
            tracing::warn!("[MediaCollect_H] 設定を読めませんでした（この起動の間は保存しません）: {e}");
            return (Settings::default(), false);
        }
    };
    match std::str::from_utf8(&text).ok().and_then(|t| serde_json::from_str::<Settings>(t).ok()) {
        Some(s) => (s, true),
        None => {
            let broken = p.with_extension("json.broken");
            match std::fs::rename(&p, &broken) {
                Ok(()) => {
                    tracing::warn!("[MediaCollect_H] 設定を読めなかったので {} へ退避しました", broken.display());
                    (Settings::default(), true)
                }
                Err(e) => {
                    tracing::warn!("[MediaCollect_H] 設定を読めず、退避もできませんでした（この起動の間は保存しません）: {e}");
                    (Settings::default(), false)
                }
            }
        }
    }
}

pub fn save(s: &Settings) {
    let p = path();
    let Ok(text) = serde_json::to_string_pretty(s) else { return };
    let res = (|| -> std::io::Result<()> {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &p)
    })();
    if let Err(e) = res {
        tracing::warn!("[MediaCollect_H] 設定を保存できませんでした: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_keys_and_unknown_keys_are_tolerated() {
        let s: Settings = serde_json::from_str(r#"{"out_dir":"E:\\収集","future_key":1}"#).unwrap();
        assert_eq!(s.out_dir, "E:\\収集");
        assert_eq!(s.excludes, default_excludes(), "無いキーは既定");
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, Settings::default());
    }
}
