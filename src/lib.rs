//! MediaCollect_H（素材の収集）
//!
//! 保存済みの `.aup2` が参照するファイル・フォルダを 1 つのフォルダへコピーし、パスをコピー先に書き換えた**別名の** `.aup2` を書き出す。
//! **元のプロジェクトと素材には書き込まない。** YMM4 の「YMM4素材同梱プラグイン」に当たる（README の説明だけを元にした）。
//! 仕様は `AI/specifications/20261009_MediaCollect_H_spec.md`。
//!
//! 本体の API は、ボタンを押したときに項目の種類を聞くだけ（`enumerate_effect_items`）。プロジェクトは書き換えない（`call_edit_section` を持たない）。
//! 自分で起こすスレッドは書き出しの 1 本（`copy.rs`）。プラグインの `Drop` で止めて終わりを待つ（ルール au2-rs-plugin）。

pub mod copy;
pub mod dialog;
mod gui;
pub mod plan;
pub mod refs;
pub mod rewrite;
mod settings;

use std::path::PathBuf;

use aviutl2::generic::GlobalEditHandle;
use aviutl2::AnyResult;
use parking_lot::{Mutex, RwLock};

pub const WINDOW_NAME: &str = "素材の収集";

pub(crate) static EDIT_HANDLE: GlobalEditHandle = GlobalEditHandle::new();

/// 開いているプロジェクトの `.aup2`。未保存なら None
pub(crate) static PROJECT: RwLock<Option<PathBuf>> = RwLock::new(None);

/// 書き出し（1 本だけ）。プラグインの `Drop` で止めて待つため、ウィンドウの外に置く
pub(crate) static COPIER: Mutex<Option<copy::Copier>> = Mutex::new(None);

fn init_logging() {
    use aviutl2::tracing::Level;
    let level = if cfg!(debug_assertions) { Level::DEBUG } else { Level::INFO };
    let _ = aviutl2::tracing_subscriber::fmt()
        .with_max_level(level)
        .event_format(aviutl2::logger::AviUtl2Formatter)
        .with_writer(aviutl2::logger::AviUtl2LogWriter)
        .try_init();
}

#[aviutl2::plugin(GenericPlugin)]
pub struct MediaCollectPlugin {
    window: Mutex<Option<aviutl2_eframe::EframeWindow>>,
}

impl aviutl2::generic::GenericPlugin for MediaCollectPlugin {
    fn new(_info: aviutl2::AviUtl2Info) -> AnyResult<Self> {
        init_logging();
        settings::init_data_dir();
        tracing::info!("MediaCollect_H v{} 初期化", env!("CARGO_PKG_VERSION"));
        Ok(Self { window: Mutex::new(None) })
    }

    fn plugin_info(&self) -> aviutl2::generic::GenericPluginTable {
        aviutl2::generic::GenericPluginTable {
            name: "MediaCollect_H".to_string(),
            information: format!("MediaCollect_H v{} - 素材の収集 / by HexBrowns", env!("CARGO_PKG_VERSION")),
        }
    }

    fn register(&mut self, registry: &mut aviutl2::generic::HostAppHandle) {
        EDIT_HANDLE.init(registry.create_edit_handle());
        let window = match aviutl2_eframe::EframeWindow::new(WINDOW_NAME, move |cc, handle| Ok(Box::new(gui::App::new(cc, handle)))) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("素材の収集のウィンドウを作れませんでした: {e:#}");
                return;
            }
        };
        match window.handle() {
            Ok(handle) => {
                if let Err(e) = registry.register_window_client(WINDOW_NAME, &handle) {
                    tracing::error!("register_window_client 失敗: {e}");
                }
            }
            Err(e) => tracing::error!("素材の収集のウィンドウのハンドルを取れませんでした: {e:#}"),
        }
        *self.window.lock() = Some(window);
        // ウィンドウは表示メニューから開く（メニューから ShowWindow しても開かないので「開く」は作らない。2026-10-10 のユーザーの指摘）
    }

    fn on_project_load(&mut self, project: &mut aviutl2::generic::ProjectFile) {
        *PROJECT.write() = project.get_path();
    }

    fn on_project_save(&mut self, project: &mut aviutl2::generic::ProjectFile) {
        if let Some(p) = project.get_path() {
            *PROJECT.write() = Some(p);
        }
    }
}

impl Drop for MediaCollectPlugin {
    fn drop(&mut self) {
        // 書き出しのスレッドを止めて終わりを待つ（DLL が外れた後に動くと終了時に落ちる。ルール au2-rs-plugin）
        if let Some(mut c) = COPIER.lock().take() {
            c.stop();
        }
    }
}

// 本体が呼ぶ関数（GetCommonPluginTable など）を書き出す（ルール au2-rs-plugin「ビルド」）
aviutl2::register_generic_plugin!(MediaCollectPlugin);
