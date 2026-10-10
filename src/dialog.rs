//! Windows まわり: フォルダを選ぶダイアログ・ファイルの日時・空き容量・エクスプローラーで開く
//!
//! ダイアログはボタンを押したときに UI のスレッドで開くモーダル。シェルが中で起こすスレッドはこちらから待てないので、
//! 初めて開くときに DLL を pin する（BackupDiff_H の `dialog.rs` と同じ。ルール au2-rs-plugin「待てないスレッドが残るなら DLL を pin する」）

use std::path::{Path, PathBuf};
use std::sync::Once;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{FILETIME, HMODULE, SYSTEMTIME};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN,
};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

static PIN: Once = Once::new();

fn pin_module() {
    PIN.call_once(|| unsafe {
        let mut module = HMODULE::default();
        let addr = pin_module as *const () as *const u16;
        if let Err(e) = GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_PIN | GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
            PCWSTR(addr),
            &mut module,
        ) {
            tracing::warn!("MediaCollect_H: DLL を pin できませんでした: {e}");
        }
    });
}

/// フォルダを選ぶ。取り消したら `None`
pub fn pick_folder() -> Option<PathBuf> {
    pin_module();
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let out = unsafe {
        (|| {
            let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            let options = dialog.GetOptions().ok()?;
            dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
            let _ = dialog.SetTitle(w!("書き出し先のフォルダ"));
            dialog.Show(Some(GetForegroundWindow())).ok()?;
            let item = dialog.GetResult().ok()?;
            let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            let path = name.to_string().ok();
            CoTaskMemFree(Some(name.0 as *const _));
            path.map(PathBuf::from)
        })()
    };
    if hr.is_ok() {
        unsafe { CoUninitialize() };
    }
    out
}

/// ファイルの更新日時を、ローカル時刻の `YYYY-MM-DD HH:MM:SS` にする
pub fn modified_text(path: &Path) -> Option<String> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
    // FILETIME は 1601-01-01 からの 100ns 単位
    let ticks = d.as_nanos() / 100 + 116_444_736_000_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        FileTimeToSystemTime(&ft, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
    }
    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond
    ))
}

/// `dir` があるドライブの空き容量（`dir` がまだ無ければ、ある親までさかのぼる）。
/// 根（`X:\` / `\\server\share\`）の無い形は None（`\\server` だけだと `\` までさかのぼり、今のドライブの空きを返していた）
pub fn free_bytes(dir: &Path) -> Option<u64> {
    crate::plan::root_of(dir)?;
    let mut d = dir.to_path_buf();
    while !d.exists() {
        d = d.parent()?.to_path_buf();
    }
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(&HSTRING::from(d.as_os_str()), Some(&mut free), None, None).ok()? };
    Some(free)
}

/// エクスプローラーでフォルダを開く
pub fn open_folder(dir: &Path) {
    if let Err(e) = std::process::Command::new("explorer.exe").arg(dir).spawn() {
        tracing::warn!("[MediaCollect_H] エクスプローラーを開けませんでした: {e}");
    }
}
