//! 本体なしで `.aup2` を読み、拾う参照とコピーの計画を出す（開発用。項目の種類は本体に聞けないので、すべて推定で拾う）。
//!
//!     cargo run --example scan -- "D:\Videos\動画編集\作品\作品.aup2"

use std::io::Write;

use media_collect_h::plan::{self, State};
use media_collect_h::refs::{self, ItemTypes};

fn main() {
    let path = std::env::args().nth(1).expect("引数に .aup2 のパス");
    let text = String::from_utf8_lossy(&std::fs::read(&path).expect("読めない")).into_owned();
    let scan = refs::scan(&text);
    let disk = plan::Disk::default();
    let relink = plan::Relink::new(scan.project_saved.as_deref(), std::path::Path::new(&path));
    let exists = |v: &str| match disk.exists(v) {
        (false, false) => relink.as_ref().and_then(|r| r.map(v)).map(|m| disk.exists(&m)).unwrap_or((false, false)),
        found => found,
    };
    let c = refs::classify(&scan, &ItemTypes::new(), exists);
    let excludes = vec![
        "C:\\ProgramData\\aviutl2\\".to_string(),
        "C:\\Windows\\".to_string(),
        "C:\\Program Files\\".to_string(),
        "C:\\Program Files (x86)\\".to_string(),
    ];
    let p = plan::build(&scan, &c, &excludes, relink.as_ref(), &disk);
    writeln!(std::io::stdout(), "読み替え {:?} / {} 件", relink, p.count(|s| s.relinked)).ok();
    let mut out = std::io::stdout().lock();
    let mut keys = std::collections::BTreeMap::<String, usize>::new();
    for r in &c.refs {
        *keys.entry(scan.entries[r.entry].key.clone()).or_default() += 1;
    }
    writeln!(out, "効果の行 {} / 参照 {} / 元 {} / 効果 {} 種", scan.entries.len(), c.refs.len(), p.sources.len(), scan.effects.len()).ok();
    writeln!(out, "拾った項目: {keys:?}").ok();
    for s in &p.sources {
        let st = match &s.state {
            State::Copy { dest, renamed } => format!("コピー → {}{}", dest.display(), if *renamed { "（印）" } else { "" }),
            State::Excluded => "除外".into(),
            State::Missing => "見つからない".into(),
        };
        writeln!(out, "  {} {} [{} か所] {}", if s.folder { "D" } else { "F" }, s.path, s.entries.len(), st).ok();
    }
    writeln!(out, "コピー {} 個・{} バイト", p.copy_count(), p.copy_bytes()).ok();
}
