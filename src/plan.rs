//! コピーの計画（どの元をどこへ置くか）。ファイルシステムは `Probe` 越しに見るので、cargo test で確かめられる。
//!
//! - 素材は `<書き出し先>\素材\` に**平らに置く**（ユーザーの決定）。同じ名前で元が違うものは `名前_<元パスの 8 桁>.拡張子`
//! - 同じ元パス（大文字小文字・`/` と `\` の違いは同じとみなす）は 1 回だけコピーし、どの参照も同じ置き先を指す
//! - フォルダの項目は中身ごと `素材\<フォルダ名>\` へ
//! - 除外のフォルダの中にあるものは、コピーせずパスをそのまま残す
//! - 見つからないものも、パスをそのまま残す（書き出しは止めない）

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use crate::refs::{Classified, Origin, Scan};

/// 書き出し先の中の素材のフォルダ名
pub const MEDIA_DIR: &str = "素材";

/// ファイルシステムの見方（テストでは差し替える）
pub trait Probe {
    /// ファイルなら大きさ
    fn file_size(&self, p: &Path) -> Option<u64>;
    /// フォルダなら（中のファイルの数, 合計の大きさ）
    fn folder_size(&self, p: &Path) -> Option<(usize, u64)>;
}

/// 本物のファイルシステム。**ドライブと共有フォルダ（`X:\` / `\\server\share\`）が見えるかを、根ごとに 1 回だけ確かめる。**
/// つながっていない NAS のパスは 1 つ確かめるのに 1 秒以上かかる（実測 1.3 秒）。素材が 100 個あれば 2 分、ウィンドウが止まる。
/// 根が見えなければ、その下はすべて見つからないとして確かめない
#[derive(Default)]
pub struct Disk {
    roots: RefCell<HashMap<String, bool>>,
}

/// パスの根（`X:\` / `\\server\share\`）。根の無い形（相対パスなど）は None
pub fn root_of(p: &Path) -> Option<String> {
    match p.components().next()? {
        Component::Prefix(prefix) => Some(format!("{}\\", prefix.as_os_str().to_string_lossy().trim_end_matches('\\'))),
        _ => None,
    }
}

impl Disk {
    fn reachable(&self, p: &Path) -> bool {
        let Some(root) = root_of(p) else { return true };
        let key = root.to_lowercase();
        if let Some(&ok) = self.roots.borrow().get(&key) {
            return ok;
        }
        let ok = Path::new(&root).exists();
        if !ok {
            tracing::info!("[MediaCollect_H] {root} が見えないので、その下の素材は確かめずに「見つからない」にします");
        }
        self.roots.borrow_mut().insert(key, ok);
        ok
    }

    /// (ファイルか, フォルダか)
    pub fn exists(&self, v: &str) -> (bool, bool) {
        let p = Path::new(v);
        if !self.reachable(p) {
            return (false, false);
        }
        match std::fs::metadata(p) {
            Ok(m) => (m.is_file(), m.is_dir()),
            Err(_) => (false, false),
        }
    }
}

impl Probe for Disk {
    fn file_size(&self, p: &Path) -> Option<u64> {
        if !self.reachable(p) {
            return None;
        }
        let m = std::fs::metadata(p).ok()?;
        m.is_file().then_some(m.len())
    }

    fn folder_size(&self, p: &Path) -> Option<(usize, u64)> {
        if !self.reachable(p) || !p.is_dir() {
            return None;
        }
        let mut n = 0;
        let mut total = 0;
        let mut stack = vec![p.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    stack.push(e.path());
                } else if ft.is_file() {
                    n += 1;
                    total += e.metadata().map(|m| m.len()).unwrap_or(0);
                }
            }
        }
        Some((n, total))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// `dest`（書き出し先からの相対パス）へコピーする
    Copy { dest: PathBuf, renamed: bool },
    Excluded,
    Missing,
}

/// 1 つの元（同じ元パスの参照をまとめたもの）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// `.aup2` に書かれていたままの値（最初の参照のもの）
    pub path: String,
    /// 実際に読む場所。ふつうは `path` と同じで、プロジェクトを動かした後の読み替え（`Relink`）をしたときだけ違う
    pub from: String,
    pub relinked: bool,
    pub folder: bool,
    pub guessed: bool,
    pub sequence: bool,
    /// フォルダなら中のファイルの数（ファイルは 1）
    pub files: usize,
    pub bytes: u64,
    pub state: State,
    /// この元を指す `Scan::entries` の番号
    pub entries: Vec<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub sources: Vec<Source>,
}

impl Plan {
    pub fn copy_count(&self) -> usize {
        self.sources.iter().filter(|s| matches!(s.state, State::Copy { .. })).map(|s| s.files).sum()
    }

    pub fn copy_bytes(&self) -> u64 {
        self.sources.iter().filter(|s| matches!(s.state, State::Copy { .. })).map(|s| s.bytes).sum()
    }

    pub fn count(&self, f: impl Fn(&Source) -> bool) -> usize {
        self.sources.iter().filter(|s| f(s)).count()
    }
}

/// 大文字小文字と区切りの違いをならした比較用の形
pub fn norm(p: &str) -> String {
    p.trim().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// `p` が `dir` の中（同じを含む）か
pub fn is_within(p: &str, dir: &str) -> bool {
    let p = norm(p);
    let d = norm(dir);
    !d.is_empty() && (p == d || p.starts_with(&(d + "\\")))
}

/// 元パスの短い印（FNV-1a 64bit の上位 32bit を 8 桁）。同じ元なら必ず同じ
pub fn short_hash(p: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in norm(p).bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", (h >> 32) as u32)
}

fn file_name(p: &str) -> String {
    let t = p.trim().trim_end_matches(['\\', '/']);
    t.rsplit(['\\', '/']).next().unwrap_or(t).to_string()
}

fn with_hash(name: &str, hash: &str, folder: bool) -> String {
    if folder {
        return format!("{name}_{hash}");
    }
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{}_{}{}", &name[..i], hash, &name[i..]),
        _ => format!("{name}_{hash}"),
    }
}

/// プロジェクトを動かした後の読み替え（本体と同じ規則）。
///
/// 本体はプロジェクトを開くとき、`[project]` の `file=`（保存した場所）と今の場所が違えば、**保存した場所のフォルダの中**を指す
/// 見つからないパスを、今のフォルダの同じ所へ読み替える（ログの `updateEffectItemFilePath`）。フォルダの外は読み替えない。
/// 実機のログから割り出した（2026-10-10、本体 2.1.13。上のフォルダの名前を変えて動かした作品 2 本の 51 件がすべてこの形で、
/// 作品のフォルダの外を指すパスは、新しい場所に実在しても読み替えていない）。`aviutl2.txt` には書かれていない。
/// 保存し直すまで `.aup2` には古いパスが残るので、読み替えないと本体では読めている素材を「見つからない」にしてしまう
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relink {
    old_dir: String,
    new_dir: String,
}

impl Relink {
    /// 保存した場所（`file=` の値）と今のプロジェクトのパスから。フォルダが同じなら None
    pub fn new(saved: Option<&str>, project: &Path) -> Option<Relink> {
        let old_dir = Path::new(saved?.trim()).parent()?.to_string_lossy().into_owned();
        let new_dir = project.parent()?.to_string_lossy().into_owned();
        (!old_dir.is_empty() && norm(&old_dir) != norm(&new_dir)).then_some(Relink { old_dir, new_dir })
    }

    /// 保存した場所のフォルダの中なら、今のフォルダでの同じ所
    pub fn map(&self, p: &str) -> Option<String> {
        let p = p.trim();
        if !is_within(p, &self.old_dir) {
            return None;
        }
        let rest = p.get(self.old_dir.trim_end_matches(['\\', '/']).len()..)?.trim_start_matches(['\\', '/']).replace('/', "\\");
        Some(Path::new(&self.new_dir).join(rest).to_string_lossy().into_owned())
    }
}

/// 元を見つける。(実際に読む場所, 読み替えたか, 数と大きさ)。見つからなければ大きさは None
fn locate(path: &str, folder: bool, relink: Option<&Relink>, probe: &dyn Probe) -> (String, bool, Option<(usize, u64)>) {
    let size = |p: &str| {
        if folder {
            probe.folder_size(Path::new(p))
        } else {
            probe.file_size(Path::new(p)).map(|b| (1, b))
        }
    };
    if let Some(found) = size(path) {
        return (path.to_string(), false, Some(found));
    }
    if let Some(moved) = relink.and_then(|r| r.map(path)) {
        if let Some(found) = size(&moved) {
            return (moved, true, Some(found));
        }
    }
    (path.to_string(), false, None)
}

pub fn build(scan: &Scan, c: &Classified, excludes: &[String], relink: Option<&Relink>, probe: &dyn Probe) -> Plan {
    let mut plan = Plan::default();
    let mut by_path: HashMap<String, usize> = HashMap::new();
    let excluded = |p: &str| excludes.iter().any(|x| is_within(p, x));
    for r in &c.refs {
        let e = &scan.entries[r.entry];
        let path = e.value.trim().to_string();
        // 除外のフォルダの中は見に行かない
        let (from, relinked, found) = if excluded(&path) { (path.clone(), false, None) } else { locate(&path, r.folder, relink, probe) };
        // 同じ元は 1 回だけ（古いパスと新しいパスで書かれていても、読み替えた先が同じなら同じ元）
        let key = norm(&from);
        if let Some(&i) = by_path.get(&key) {
            plan.sources[i].entries.push(r.entry);
            plan.sources[i].sequence |= e.sequence;
            continue;
        }
        let (state, files, bytes) = match found {
            _ if excluded(&from) => (State::Excluded, 0, 0),
            Some((n, b)) => (State::Copy { dest: PathBuf::new(), renamed: false }, n, b),
            None => (State::Missing, 0, 0),
        };
        by_path.insert(key, plan.sources.len());
        plan.sources.push(Source {
            path,
            from,
            relinked,
            folder: r.folder,
            guessed: r.origin == Origin::Guessed,
            sequence: e.sequence,
            files,
            bytes,
            state,
            entries: vec![r.entry],
        });
    }
    // 置き先の名前を決める。同じ名前（大文字小文字は同じとみなす）が 2 つ以上あれば、元パスの印を付ける
    let mut names: HashMap<String, usize> = HashMap::new();
    for s in &plan.sources {
        if matches!(s.state, State::Copy { .. }) {
            *names.entry(file_name(&s.from).to_lowercase()).or_default() += 1;
        }
    }
    for s in &mut plan.sources {
        if let State::Copy { dest, renamed } = &mut s.state {
            let name = file_name(&s.from);
            let clash = names.get(&name.to_lowercase()).copied().unwrap_or(0) > 1;
            let final_name = if clash { with_hash(&name, &short_hash(&s.from), s.folder) } else { name };
            *dest = Path::new(MEDIA_DIR).join(final_name);
            *renamed = clash;
        }
    }
    plan
}

/// 書き出す前の確かめ。止める理由があれば返す
pub fn check_target(plan: &Plan, project: &str, target: &Path, target_exists_nonempty: bool) -> Result<(), String> {
    let t = target.to_string_lossy();
    if target_exists_nonempty {
        return Err(format!("書き出し先 {t} はもうあって、中身が空ではありません。別の名前にしてください"));
    }
    if let Some(dir) = Path::new(project).parent() {
        if norm(&dir.to_string_lossy()) == norm(&t) {
            return Err("書き出し先が、元のプロジェクトのフォルダと同じです".into());
        }
    }
    for s in &plan.sources {
        if s.folder && matches!(s.state, State::Copy { .. }) && is_within(&t, &s.from) {
            return Err(format!("書き出し先が、コピーするフォルダ {} の中にあります", s.from));
        }
        if is_within(&s.from, &t) {
            return Err(format!("素材 {} が書き出し先の中にあります", s.from));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::refs::{self, Ref};

    struct Fake;

    impl Probe for Fake {
        fn file_size(&self, p: &Path) -> Option<u64> {
            match p.to_string_lossy().as_ref() {
                "D:\\a\\x.png" => Some(100),
                "D:\\b\\x.png" | "d:/b/X.PNG" => Some(200),
                "D:\\a\\y.wav" => Some(300),
                "C:\\ProgramData\\aviutl2\\Script\\z.png" => Some(5),
                _ => None,
            }
        }
        fn folder_size(&self, p: &Path) -> Option<(usize, u64)> {
            (p.to_string_lossy() == "D:\\群").then_some((3, 30))
        }
    }

    fn scan_of(values: &[(&str, &str)]) -> (refs::Scan, refs::Classified) {
        let mut text = String::new();
        for (i, (k, v)) in values.iter().enumerate() {
            text.push_str(&format!("[{i}.0]\r\neffect.name=E\r\n{k}={v}\r\n"));
        }
        let s = refs::scan(&text);
        let refs = s
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| Ref { entry: i, folder: e.key == "フォルダ", origin: refs::Origin::Typed })
            .collect();
        (s, refs::Classified { refs, ..Default::default() })
    }

    #[test]
    fn same_path_is_copied_once_and_same_name_is_renamed() {
        let (s, c) = scan_of(&[
            ("ファイル", "D:\\a\\x.png"),
            ("ファイル", "D:\\b\\x.png"),
            ("ファイル", "d:/b/X.PNG"),
            ("ファイル", "D:\\a\\y.wav"),
            ("フォルダ", "D:\\群"),
            ("ファイル", "D:\\無い.png"),
            ("ファイル", "C:\\ProgramData\\aviutl2\\Script\\z.png"),
        ]);
        let p = build(&s, &c, &["C:\\ProgramData\\aviutl2\\".to_string()], None, &Fake);
        assert_eq!(p.sources.len(), 6, "大文字小文字と区切りが違うだけの d:/b/X.PNG は D:\\b\\x.png と同じ元");
        let b = &p.sources[1];
        assert_eq!(b.entries, [1, 2]);
        let dest = |i: usize| match &p.sources[i].state {
            State::Copy { dest, renamed } => (dest.to_string_lossy().into_owned(), *renamed),
            st => panic!("{st:?}"),
        };
        let (a_dest, a_ren) = dest(0);
        let (b_dest, b_ren) = dest(1);
        assert!(a_ren && b_ren && a_dest != b_dest, "同名の 2 つはどちらも印付き: {a_dest} / {b_dest}");
        assert!(a_dest.starts_with("素材\\x_") && a_dest.ends_with(".png"));
        assert_eq!(dest(2), ("素材\\y.wav".to_string(), false));
        assert_eq!(dest(3), ("素材\\群".to_string(), false));
        assert_eq!(p.sources[4].state, State::Missing);
        assert_eq!(p.sources[5].state, State::Excluded);
        assert_eq!((p.copy_count(), p.copy_bytes()), (1 + 1 + 1 + 3, 100 + 200 + 300 + 30));
    }

    /// `D:\ビデオ\作品\` で保存し、`D:\Videos\作品\` へ動かした作品
    struct Moved;

    impl Probe for Moved {
        fn file_size(&self, p: &Path) -> Option<u64> {
            match p.to_string_lossy().as_ref() {
                "D:\\Videos\\作品\\素材\\x.png" => Some(10),
                "D:\\Videos\\その他\\y.png" => Some(20),
                "D:\\Pictures\\z.png" => Some(30),
                _ => None,
            }
        }
        fn folder_size(&self, p: &Path) -> Option<(usize, u64)> {
            (p.to_string_lossy() == "D:\\Videos\\作品\\連番").then_some((2, 4))
        }
    }

    #[test]
    fn moved_project_is_relinked_like_the_host() {
        let relink = Relink::new(Some("D:\\ビデオ\\作品\\作品.aup2"), Path::new("D:\\Videos\\作品\\作品.aup2")).unwrap();
        assert_eq!(relink.map("D:\\ビデオ\\作品\\素材\\x.png").as_deref(), Some("D:\\Videos\\作品\\素材\\x.png"));
        assert_eq!(relink.map("d:/ビデオ/作品/素材/x.png").as_deref(), Some("D:\\Videos\\作品\\素材\\x.png"));
        assert_eq!(relink.map("D:\\ビデオ\\その他\\y.png"), None, "保存した場所のフォルダの外は読み替えない（本体と同じ）");
        assert_eq!(relink.map("D:\\ビデオ\\作品2\\y.png"), None, "名前が前方一致するだけの別のフォルダは外");
        assert!(Relink::new(Some("D:\\Videos\\作品\\作品.aup2"), Path::new("d:\\videos\\作品\\作品.aup2")).is_none(), "動かしていない");
        assert!(Relink::new(None, Path::new("D:\\Videos\\作品\\作品.aup2")).is_none());

        let (s, c) = scan_of(&[
            ("ファイル", "D:\\ビデオ\\作品\\素材\\x.png"),
            ("ファイル", "D:\\Videos\\作品\\素材\\x.png"),
            ("ファイル", "D:\\ビデオ\\その他\\y.png"),
            ("ファイル", "D:\\Pictures\\z.png"),
            ("フォルダ", "D:\\ビデオ\\作品\\連番"),
        ]);
        let p = build(&s, &c, &[], Some(&relink), &Moved);
        assert_eq!(p.sources.len(), 4, "古いパスと新しいパスで書かれた x.png は同じ元");
        let x = &p.sources[0];
        assert!(x.relinked && x.from == "D:\\Videos\\作品\\素材\\x.png" && x.path == "D:\\ビデオ\\作品\\素材\\x.png");
        assert_eq!(x.entries, [0, 1]);
        assert!(matches!(x.state, State::Copy { .. }));
        assert_eq!(p.sources[1].state, State::Missing, "外の y.png は新しい場所にあっても読み替えない");
        assert!(!p.sources[2].relinked && matches!(p.sources[2].state, State::Copy { .. }), "見つかるものはそのまま");
        assert!(p.sources[3].relinked && p.sources[3].files == 2, "フォルダも読み替える");
        // 読み替えなしなら、x.png は 2 つの元に分かれ、古いほうは見つからない
        let p = build(&s, &c, &[], None, &Moved);
        assert_eq!(p.count(|s| s.state == State::Missing), 3);
    }

    #[test]
    fn roots_of_drive_and_share() {
        assert_eq!(root_of(Path::new("D:\\素材\\a.png")).as_deref(), Some("D:\\"));
        assert_eq!(root_of(Path::new("d:/素材/a.png")).as_deref(), Some("d:\\"));
        assert_eq!(root_of(Path::new("\\\\nas\\share\\素材\\a.png")).as_deref(), Some("\\\\nas\\share\\"));
        assert_eq!(root_of(Path::new("素材\\a.png")), None);
    }

    #[test]
    fn hash_is_stable_and_case_blind() {
        assert_eq!(short_hash("D:\\a\\x.png"), short_hash("d:/A/X.png"));
        assert_ne!(short_hash("D:\\a\\x.png"), short_hash("D:\\b\\x.png"));
        assert_eq!(short_hash("D:\\a\\x.png").len(), 8);
        assert_eq!(with_hash("x.tar.gz", "12345678", false), "x.tar_12345678.gz");
        assert_eq!(with_hash(".env", "12345678", false), ".env_12345678");
        assert_eq!(with_hash("群", "12345678", true), "群_12345678");
    }

    #[test]
    fn within_and_target_checks() {
        assert!(is_within("C:\\ProgramData\\aviutl2\\Script\\a.png", "c:\\programdata\\AVIUTL2"));
        assert!(!is_within("C:\\ProgramData\\aviutl2x\\a.png", "C:\\ProgramData\\aviutl2"));
        assert!(!is_within("C:\\a", ""));
        let (s, c) = scan_of(&[("フォルダ", "D:\\群"), ("ファイル", "D:\\a\\x.png")]);
        let p = build(&s, &c, &[], None, &Fake);
        assert!(check_target(&p, "D:\\作品\\a.aup2", Path::new("E:\\収集\\a"), false).is_ok());
        assert!(check_target(&p, "D:\\作品\\a.aup2", Path::new("E:\\収集\\a"), true).is_err(), "中身のある書き出し先");
        assert!(check_target(&p, "D:\\作品\\a.aup2", Path::new("D:\\群\\out"), false).is_err(), "コピーするフォルダの中");
        assert!(check_target(&p, "D:\\作品\\a.aup2", Path::new("D:\\a"), false).is_err(), "素材が書き出し先の中");
        assert!(check_target(&p, "D:\\作品\\a.aup2", Path::new("D:\\作品"), false).is_err(), "元のフォルダそのもの");
    }
}
