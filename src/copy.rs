//! 書き出し（裏のスレッド 1 本）。素材のコピー → 新しい `.aup2` → 隣のファイル → 収集.json・受け手に要るもの.txt → 読み直して確かめる。
//!
//! - **元のプロジェクトと素材には書き込まない。** 書くのは書き出し先のフォルダの中だけ
//! - ファイルは 8MB ずつ `<置き先>.part` に書いてから名前を変える。中止は 8MB ごとに見る。中止したら書きかけの `.part` だけ消す
//!   （こちらが作ったもの。コピーし終えたファイルは残す）
//! - コピーした後に大きさを比べ、更新日時を元に合わせる
//! - スレッドはプラグインの `Drop` で止めて終わりを待つ（ルール au2-rs-plugin「裏のスレッド」）

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::plan::{Plan, State};
use crate::refs::{self, Scan};
use crate::rewrite;

const CHUNK: usize = 8 * 1024 * 1024;

/// CurveEditor_H がプロジェクトの隣に置くファイル（`storage.rs` の命名）
const SIDECARS: [&str; 2] = ["CurveEditor_H.Live.lua", "CurveEditor_H.Library.json"];

pub struct Job {
    pub project: PathBuf,
    pub text: String,
    pub scan: Scan,
    pub plan: Plan,
    pub fonts: Vec<String>,
    pub unknown_effects: Vec<String>,
    /// 書き出し先のフォルダ（`<出力先>\<名前>`）
    pub target: PathBuf,
    /// 新しい `.aup2` の名前（拡張子なし）
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub target: PathBuf,
    pub aup2: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub sidecars: Vec<String>,
    /// 書いた `.aup2` を読み直して、実在しなかった値
    pub broken: Vec<String>,
    pub seconds: f32,
}

#[derive(Default)]
pub struct Progress {
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub current: String,
    pub result: Option<Result<Report, String>>,
}

pub struct Copier {
    pub progress: Arc<Mutex<Progress>>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

/// 描き直しの依頼を 0.1 秒に 1 回までにする。小さいファイルが数千あると、ファイルごとの依頼は 1 秒に 200 回になる
/// （実測。ウィンドウがそのたびに 1 フレーム描く）
const NOTE_EVERY: Duration = Duration::from_millis(100);

impl Copier {
    pub fn start(job: Job, on_change: impl Fn() + Send + 'static) -> Copier {
        let progress = Arc::new(Mutex::new(Progress { total_bytes: job.plan.copy_bytes(), ..Default::default() }));
        let cancel = Arc::new(AtomicBool::new(false));
        let (p, c) = (Arc::clone(&progress), Arc::clone(&cancel));
        let handle = std::thread::Builder::new()
            .name("MediaCollect_H 書き出し".into())
            .spawn(move || {
                let last = std::cell::Cell::new(Instant::now());
                let note = || {
                    if last.get().elapsed() >= NOTE_EVERY {
                        last.set(Instant::now());
                        on_change();
                    }
                };
                let r = run(&job, &p, &c, &note);
                p.lock().result = Some(r);
                on_change();
            })
            .ok();
        if handle.is_none() {
            progress.lock().result = Some(Err("書き出しのスレッドを起こせませんでした".into()));
        }
        Copier { progress, cancel, handle }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    pub fn finished(&self) -> bool {
        self.progress.lock().result.is_some()
    }

    /// 止めて終わりを待つ（上限 5 秒。8MB の書き込み 1 回より十分長い）
    pub fn stop(&mut self) {
        self.cancel();
        if let Some(h) = self.handle.take() {
            let t = Instant::now();
            while !h.is_finished() {
                if t.elapsed() > Duration::from_secs(5) {
                    tracing::warn!("[MediaCollect_H] 書き出しのスレッドが 5 秒で止まりませんでした");
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = h.join();
        }
    }
}

impl Drop for Copier {
    fn drop(&mut self) {
        self.stop();
    }
}

fn cancelled(c: &AtomicBool) -> Result<(), String> {
    if c.load(Ordering::Acquire) {
        Err("中止しました（書き出し先には、コピーし終えたファイルが残っています）".into())
    } else {
        Ok(())
    }
}

/// 1 つのファイルを 8MB ずつ写す。`buf` は書き出し全体で 1 つを使い回す。置き先のフォルダは呼ぶ側が作っておく
fn copy_file(
    src: &Path,
    dst: &Path,
    buf: &mut [u8],
    progress: &Mutex<Progress>,
    cancel: &AtomicBool,
    on_change: &dyn Fn(),
) -> Result<u64, String> {
    let part = PathBuf::from(format!("{}.part", dst.display()));
    let mut input = std::fs::File::open(src).map_err(|e| format!("{} を開けません: {e}", src.display()))?;
    let mut output = std::fs::File::create(&part).map_err(|e| format!("{} を作れません: {e}", part.display()))?;
    let mut total = 0u64;
    loop {
        if let Err(e) = cancelled(cancel) {
            drop(output);
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
        let n = input.read(buf).map_err(|e| format!("{} を読めません: {e}", src.display()))?;
        if n == 0 {
            break;
        }
        output.write_all(&buf[..n]).map_err(|e| format!("{} に書けません: {e}", part.display()))?;
        total += n as u64;
        progress.lock().done_bytes += n as u64;
        on_change();
    }
    output.flush().map_err(|e| format!("{} に書けません: {e}", part.display()))?;
    let src_meta = input.metadata().ok();
    if let Some(m) = src_meta.as_ref().and_then(|m| m.modified().ok()) {
        let _ = output.set_modified(m);
    }
    drop(output);
    let src_len = src_meta.map(|m| m.len()).unwrap_or(total);
    if src_len != total {
        return Err(format!("{} の大きさが合いません（元 {src_len} / 写し {total}）", src.display()));
    }
    std::fs::rename(&part, dst).map_err(|e| format!("{} の名前を変えられません: {e}", part.display()))?;
    Ok(total)
}

/// フォルダの中身を写す（ファイルの数, 大きさ）
fn copy_folder(
    src: &Path,
    dst: &Path,
    buf: &mut [u8],
    progress: &Mutex<Progress>,
    cancel: &AtomicBool,
    on_change: &dyn Fn(),
) -> Result<(usize, u64), String> {
    let mut n = 0;
    let mut bytes = 0;
    let mut stack = vec![(src.to_path_buf(), dst.to_path_buf())];
    while let Some((s, d)) = stack.pop() {
        std::fs::create_dir_all(&d).map_err(|e| format!("{} を作れません: {e}", d.display()))?;
        let rd = std::fs::read_dir(&s).map_err(|e| format!("{} を読めません: {e}", s.display()))?;
        for e in rd.flatten() {
            cancelled(cancel)?;
            let Ok(ft) = e.file_type() else { continue };
            let to = d.join(e.file_name());
            if ft.is_dir() {
                stack.push((e.path(), to));
            } else if ft.is_file() {
                progress.lock().current = e.path().display().to_string();
                bytes += copy_file(&e.path(), &to, buf, progress, cancel, on_change)?;
                n += 1;
            }
        }
    }
    Ok((n, bytes))
}

/// 新しいファイルを書く（一時ファイルに書いてから名前を変える）
fn write_new(path: &Path, data: &[u8]) -> Result<(), String> {
    let tmp = PathBuf::from(format!("{}.part", path.display()));
    std::fs::write(&tmp, data).map_err(|e| format!("{} に書けません: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{} の名前を変えられません: {e}", tmp.display()))
}

fn json_escape(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn run(job: &Job, progress: &Mutex<Progress>, cancel: &AtomicBool, on_change: &dyn Fn()) -> Result<Report, String> {
    let t0 = Instant::now();
    std::fs::create_dir_all(&job.target).map_err(|e| format!("{} を作れません: {e}", job.target.display()))?;
    let mut report = Report { target: job.target.clone(), ..Default::default() };
    let mut buf = vec![0u8; CHUNK];

    // 1. 素材（置き先のフォルダは、前のファイルと違うときだけ作る。ふつうはすべて `素材\`）
    let mut made_dir: Option<PathBuf> = None;
    for s in &job.plan.sources {
        cancelled(cancel)?;
        let State::Copy { dest, .. } = &s.state else { continue };
        let to = job.target.join(dest);
        progress.lock().current = s.from.clone();
        on_change();
        if s.folder {
            let (n, b) = copy_folder(Path::new(&s.from), &to, &mut buf, progress, cancel, on_change)?;
            report.files += n;
            report.bytes += b;
        } else {
            if let Some(dir) = to.parent() {
                if made_dir.as_deref() != Some(dir) {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
                    made_dir = Some(dir.to_path_buf());
                }
            }
            report.bytes += copy_file(Path::new(&s.from), &to, &mut buf, progress, cancel, on_change)?;
            report.files += 1;
        }
    }
    cancelled(cancel)?;

    // 2. 新しい .aup2（値だけを差し替える）
    progress.lock().current = "プロジェクトを書いています".into();
    let aup2 = job.target.join(format!("{}.aup2", job.name));
    let mut edits = Vec::new();
    for s in &job.plan.sources {
        let State::Copy { dest, .. } = &s.state else { continue };
        let new_path = job.target.join(dest).display().to_string();
        for &i in &s.entries {
            edits.push((job.scan.entries[i].range.clone(), new_path.clone()));
        }
    }
    if let Some(r) = &job.scan.project_file {
        edits.push((r.clone(), aup2.display().to_string()));
    }
    let text = rewrite::apply(&job.text, &edits)?;
    write_new(&aup2, text.as_bytes())?;
    report.aup2 = aup2.clone();

    // 3. CurveEditor_H の隣のファイル（プロジェクトごとのもの。無ければフォルダに 1 つの以前のもの）
    if let (Some(dir), Some(stem)) = (job.project.parent(), job.project.file_stem()) {
        for suffix in SIDECARS {
            let own = dir.join(format!("{}.{suffix}", stem.to_string_lossy()));
            let shared = dir.join(suffix);
            let from = if own.is_file() { own } else if shared.is_file() { shared } else { continue };
            let to = job.target.join(format!("{}.{suffix}", job.name));
            copy_file(&from, &to, &mut buf, progress, cancel, on_change)?;
            report.sidecars.push(to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        }
    }

    // 4. 収集.json（元のパス → 新しいパス。別の場所で開き直すときに使う）と、受け手に要るもの.txt
    let mut json = String::from("{\r\n");
    json += &format!("  \"source_project\": {},\r\n", json_escape(&job.project.display().to_string()));
    json += &format!("  \"project\": {},\r\n", json_escape(&format!("{}.aup2", job.name)));
    json += "  \"items\": [\r\n";
    let mut first = true;
    for s in &job.plan.sources {
        let (state, to) = match &s.state {
            State::Copy { dest, .. } => ("copied", dest.display().to_string()),
            State::Excluded => ("excluded", String::new()),
            State::Missing => ("missing", String::new()),
        };
        if !first {
            json += ",\r\n";
        }
        first = false;
        // written: .aup2 に書かれていた値（プロジェクトを動かした後の読み替えをしたときだけ from と違う）
        json += &format!(
            "    {{\"from\": {}, \"written\": {}, \"to\": {}, \"state\": \"{state}\", \"folder\": {}, \"files\": {}, \"bytes\": {}}}",
            json_escape(&s.from),
            json_escape(&s.path),
            json_escape(&to),
            s.folder,
            s.files,
            s.bytes
        );
    }
    json += "\r\n  ]\r\n}\r\n";
    write_new(&job.target.join("収集.json"), json.as_bytes())?;
    write_new(&job.target.join("受け手に要るもの.txt"), needs_text(job).as_bytes())?;

    // 5. 読み直して、差し替えた値がすべて実在するか
    let written = std::fs::read_to_string(&aup2).map_err(|e| format!("書いた {} を読めません: {e}", aup2.display()))?;
    let again = refs::scan(&written);
    for s in &job.plan.sources {
        if !matches!(s.state, State::Copy { .. }) {
            continue;
        }
        for &i in &s.entries {
            let v = again.entries.get(i).map(|e| e.value.clone()).unwrap_or_default();
            if !Path::new(&v).exists() {
                report.broken.push(v);
            }
        }
    }
    report.seconds = t0.elapsed().as_secs_f32();
    Ok(report)
}

/// 受け手に要るもの.txt の本文（CRLF）
pub fn needs_text(job: &Job) -> String {
    let mut t = String::new();
    let mut line = |s: &str| {
        t.push_str(s);
        t.push_str("\r\n");
    };
    line(&format!("このフォルダは、{} を MediaCollect_H（素材の収集）で書き出したものです。", job.project.display()));
    line("プロジェクトの中のパスは絶対パスです。フォルダを別の場所へ動かすと、素材のつながりが切れます。");
    line("");
    line("■ 受け手の AviUtl2 に入っている必要があるもの");
    line("");
    line(&format!("フォント（{} 個。コピーしていません）", job.fonts.len()));
    for f in &job.fonts {
        line(&format!("  {f}"));
    }
    line("");
    line(&format!("使っている効果・スクリプト（{} 種。標準のものを含みます）", job.scan.effects.len()));
    for (name, n) in &job.scan.effects {
        line(&format!("  {name}（{n}）"));
    }
    if !job.unknown_effects.is_empty() {
        line("");
        line("書き出したときの AviUtl2 に入っていなかった効果（素材のパスは推定で拾っています）");
        for e in &job.unknown_effects {
            line(&format!("  {e}"));
        }
    }
    let excluded: Vec<&str> = job.plan.sources.iter().filter(|s| s.state == State::Excluded).map(|s| s.path.as_str()).collect();
    if !excluded.is_empty() {
        line("");
        line("コピーしなかった（除外のフォルダの中にある）ファイル。受け手の同じ場所に要ります");
        for p in excluded {
            line(&format!("  {p}"));
        }
    }
    let missing: Vec<&str> = job.plan.sources.iter().filter(|s| s.state == State::Missing).map(|s| s.path.as_str()).collect();
    if !missing.is_empty() {
        line("");
        line("書き出したときに見つからなかったファイル（パスは元のまま）");
        for p in missing {
            line(&format!("  {p}"));
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{self, Disk};
    use crate::refs::{tests::types, Classified};

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mediacollect_test_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 本物のファイルで、書き出し → 読み直しまで通す。元のファイルは変わらない
    #[test]
    fn end_to_end_on_temp_files() {
        let root = tmp_dir("e2e");
        let src = root.join("src");
        std::fs::create_dir_all(src.join("a")).unwrap();
        std::fs::create_dir_all(src.join("b")).unwrap();
        std::fs::create_dir_all(src.join("群").join("中")).unwrap();
        std::fs::write(src.join("a").join("x.png"), vec![1u8; 10]).unwrap();
        std::fs::write(src.join("b").join("x.png"), vec![2u8; 20]).unwrap();
        std::fs::write(src.join("群").join("中").join("1.png"), b"abc").unwrap();
        let proj_dir = root.join("作品");
        std::fs::create_dir_all(&proj_dir).unwrap();
        let project = proj_dir.join("作品.aup2");
        std::fs::write(proj_dir.join("作品.CurveEditor_H.Live.lua"), "@.live.1\r\nreturn t\r\n").unwrap();
        let a = src.join("a").join("x.png").display().to_string();
        let b = src.join("b").join("x.png").display().to_string();
        let g = src.join("群").display().to_string();
        let text = format!(
            "[project]\r\nfile={}\r\n[0]\r\nlayer=0\r\n[0.0]\r\neffect.name=画像ファイル\r\nファイル={a}\r\n[1]\r\n[1.0]\r\neffect.name=画像ファイル\r\nファイル={b}\r\n\
[2]\r\n[2.0]\r\neffect.name=謎@未導入\r\n画像フォルダ={g}\r\n[3]\r\n[3.0]\r\neffect.name=画像ファイル\r\nファイル={}\r\n",
            project.display(),
            src.join("無い.png").display()
        );
        std::fs::write(&project, &text).unwrap();
        let before = std::fs::read(&project).unwrap();

        let scan = refs::scan(&text);
        let c: Classified = refs::classify(&scan, &types(), refs::exists_on_disk);
        let plan = plan::build(&scan, &c, &[], None, &Disk::default());
        let target = root.join("出力").join("作品_収集");
        let job = Job {
            project: project.clone(),
            text: text.clone(),
            scan,
            plan,
            fonts: c.fonts.into_iter().collect(),
            unknown_effects: c.unknown_effects.into_iter().collect(),
            target: target.clone(),
            name: "作品_収集".into(),
        };
        let progress = Mutex::new(Progress::default());
        let report = run(&job, &progress, &AtomicBool::new(false), &|| {}).unwrap();
        assert_eq!(report.files, 3, "x.png 2 つと フォルダの中の 1 つ");
        assert_eq!(report.bytes, 33);
        assert!(report.broken.is_empty(), "{:?}", report.broken);
        assert_eq!(report.sidecars, ["作品_収集.CurveEditor_H.Live.lua"]);
        let out = std::fs::read_to_string(target.join("作品_収集.aup2")).unwrap();
        assert!(out.contains(&format!("file={}", target.join("作品_収集.aup2").display())));
        assert!(out.contains(&format!("画像フォルダ={}", target.join("素材").join("群").display())));
        assert!(out.contains("無い.png"), "見つからないものは元のパスのまま");
        assert!(target.join("素材").join("群").join("中").join("1.png").is_file());
        assert!(target.join("収集.json").is_file() && target.join("受け手に要るもの.txt").is_file());
        assert_eq!(std::fs::read(&project).unwrap(), before, "元のプロジェクトは変わらない");
        // 書きかけの .part は残らない
        let parts = walk(&target).into_iter().filter(|p| p.to_string_lossy().ends_with(".part")).count();
        assert_eq!(parts, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cancel_leaves_no_part_file() {
        let root = tmp_dir("cancel");
        let src = root.join("big.bin");
        std::fs::write(&src, vec![0u8; CHUNK * 2 + 5]).unwrap();
        let cancel = AtomicBool::new(true);
        let mut buf = vec![0u8; CHUNK];
        std::fs::create_dir_all(root.join("out")).unwrap();
        let r = copy_file(&src, &root.join("out").join("big.bin"), &mut buf, &Mutex::new(Progress::default()), &cancel, &|| {});
        assert!(r.is_err());
        assert!(!root.join("out").join("big.bin.part").exists() && !root.join("out").join("big.bin").exists());
        // 中止しなければ写せる（2 回より多く読む大きさ）
        let ok = copy_file(&src, &root.join("out").join("big.bin"), &mut buf, &Mutex::new(Progress::default()), &AtomicBool::new(false), &|| {});
        assert_eq!(ok.unwrap(), (CHUNK * 2 + 5) as u64);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn walk(d: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![d.to_path_buf()];
        while let Some(x) = stack.pop() {
            for e in std::fs::read_dir(&x).into_iter().flatten().flatten() {
                if e.path().is_dir() {
                    stack.push(e.path());
                } else {
                    out.push(e.path());
                }
            }
        }
        out
    }
}
