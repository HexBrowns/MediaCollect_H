//! `.aup2` の中から、ファイル・フォルダを指す項目を見つける（読むだけ。cargo test の中心）。
//!
//! - 書き出しは**行単位の置き換え**にするので、値の位置（バイトの範囲）を持ったまま読む（`rewrite.rs`）
//! - 効果の節（`[N.M]`）の `項目=値` だけを見る。`[project]` の `file=`（プロジェクト自身）は別に持つ。
//!   シーンの `output.file`（出力先）と `[plugin.N]` は見ない
//! - 項目の種類は本体に聞く（`enumerate_effect_items`。ファイル・フォルダ・フォント）。種類が分からない効果
//!   （入っていないスクリプトなど）は、値が実在する絶対パスなら拾い、「推定」の印を付ける
//! - 項目名に「出力」を含むもの（`出力SVG` など）は書き出し先なので拾わない（種類で入力と出力を区別できるかは未確認）

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;
use std::path::Path;

/// 項目の種類（こちらで使うものだけ）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    File,
    Folder,
    Font,
    /// 名前かファイルのパス（リストと文字の複合・図形・マスク）。値が絶対パスならファイルとして拾う。
    /// 図形の効果の「図形の種類」は本体から Combo で返り、SVG などのパスを持てる。本体もこの値を素材のパスとして扱う
    /// （動かした作品で `updateEffectItemFilePath` が SVG を読み替えていた。実機ログ 2026-10-10）
    NameOrFile,
    /// テキスト（本文。パスに見えても拾わない）
    Text,
    /// それ以外（数値・色など）
    Other,
}

/// 効果ごとの項目の種類（効果名 → 項目名 → 種類）。本体から取る
pub type ItemTypes = HashMap<String, HashMap<String, ItemKind>>;

/// 効果の節の 1 行
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub effect: String,
    pub key: String,
    pub value: String,
    /// `value` のバイトの範囲（行末の `\r` を含まない）
    pub range: Range<usize>,
    /// 同じ効果の節に `連番ファイル=1` がある
    pub sequence: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Scan {
    pub entries: Vec<Entry>,
    /// `[project]` の `file=` の値の範囲
    pub project_file: Option<Range<usize>>,
    /// `[project]` の `file=` の値（保存したときのプロジェクトの場所。動かした後は今の場所と違う）
    pub project_saved: Option<String>,
    /// 使っている効果の名前と回数
    pub effects: BTreeMap<String, usize>,
}

/// 拾った参照の印
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// 本体が教えた種類（ファイル・フォルダ）
    Typed,
    /// 種類が分からず、値が実在する絶対パスだった
    Guessed,
}

/// ファイルかフォルダを指す 1 つの値
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub entry: usize,
    pub folder: bool,
    pub origin: Origin,
}

#[derive(Debug, Clone, Default)]
pub struct Classified {
    pub refs: Vec<Ref>,
    /// 使っているフォント（コピーしない。受け手に要るものとして出す）
    pub fonts: BTreeSet<String>,
    /// 種類を本体から取れなかった効果
    pub unknown_effects: BTreeSet<String>,
}

pub fn scan(text: &str) -> Scan {
    let mut s = Scan::default();
    #[derive(PartialEq)]
    enum Sec {
        Project,
        Effect,
        Other,
    }
    let mut sec = Sec::Other;
    let mut effect = String::new();
    let mut effect_start = 0usize;
    let mut pos = 0usize;
    let bytes_len = text.len();
    while pos < bytes_len {
        let end = text[pos..].find('\n').map(|i| pos + i).unwrap_or(bytes_len);
        let mut line_end = end;
        if line_end > pos && text.as_bytes()[line_end - 1] == b'\r' {
            line_end -= 1;
        }
        let mut line_start = pos;
        // 先頭の BOM は飛ばす
        if pos == 0 && text[pos..line_end].starts_with('\u{feff}') {
            line_start += '\u{feff}'.len_utf8();
        }
        let line = &text[line_start..line_end];
        pos = end + 1;
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
            sec = if name == "project" {
                Sec::Project
            } else if is_effect_section(name) {
                effect.clear();
                effect_start = s.entries.len();
                Sec::Effect
            } else {
                Sec::Other
            };
            continue;
        }
        let Some(eq) = line.find('=') else { continue };
        let key = &line[..eq];
        let value = &line[eq + 1..];
        let range = line_start + eq + 1..line_end;
        match sec {
            Sec::Project => {
                if key == "file" {
                    s.project_file = Some(range);
                    s.project_saved = Some(value.to_string());
                }
            }
            Sec::Effect => {
                if key == "effect.name" {
                    effect = value.to_string();
                    *s.effects.entry(effect.clone()).or_default() += 1;
                    continue;
                }
                if key == "連番ファイル" && value.trim() == "1" {
                    for e in &mut s.entries[effect_start..] {
                        e.sequence = true;
                    }
                }
                let sequence = s.entries[effect_start..].first().is_some_and(|e| e.sequence);
                s.entries.push(Entry { effect: effect.clone(), key: key.to_string(), value: value.to_string(), range, sequence });
            }
            Sec::Other => {}
        }
    }
    s
}

/// `[N.M]`（効果の節）か
fn is_effect_section(name: &str) -> bool {
    name.split_once('.')
        .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty() && a.bytes().all(|c| c.is_ascii_digit()) && b.bytes().all(|c| c.is_ascii_digit()))
}

/// 絶対パスの形か（`X:\` か `\\server\`）
pub fn looks_absolute(v: &str) -> bool {
    let b = v.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')) || v.starts_with("\\\\")
}

/// 項目の種類と、実在するか（`exists` は (ファイルか, フォルダか)）で、拾うものを決める
pub fn classify(scan: &Scan, types: &ItemTypes, exists: impl Fn(&str) -> (bool, bool)) -> Classified {
    let mut c = Classified::default();
    for (i, e) in scan.entries.iter().enumerate() {
        let value = e.value.trim();
        if value.is_empty() || e.key.contains("出力") {
            continue;
        }
        match types.get(&e.effect) {
            Some(items) => match items.get(&e.key) {
                Some(ItemKind::File) => c.refs.push(Ref { entry: i, folder: false, origin: Origin::Typed }),
                Some(ItemKind::Folder) => c.refs.push(Ref { entry: i, folder: true, origin: Origin::Typed }),
                Some(ItemKind::NameOrFile) if looks_absolute(value) => {
                    c.refs.push(Ref { entry: i, folder: false, origin: Origin::Typed })
                }
                Some(ItemKind::Font) => {
                    c.fonts.insert(value.to_string());
                }
                Some(ItemKind::Text) => {}
                // ほかの種類（と、本体が教えなかった項目）でも、値が実在するファイルの絶対パスなら推定として拾う。
                // 種類が分かっていても、ほかにパスを持つ項目があるかもしれない（図形の「図形の種類」が Combo だったのは、これで拾って分かった。
                // 拾った項目の種類はログの「種類がファイル以外の項目で拾ったもの」で見る）。フォルダは拾わない（保存先などを丸ごと写さないため）
                _ => {
                    if looks_absolute(value) && exists(value).0 {
                        c.refs.push(Ref { entry: i, folder: false, origin: Origin::Guessed });
                    }
                }
            },
            None => {
                c.unknown_effects.insert(e.effect.clone());
                if looks_absolute(value) {
                    let (file, dir) = exists(value);
                    if file || dir {
                        c.refs.push(Ref { entry: i, folder: dir, origin: Origin::Guessed });
                    }
                }
            }
        }
    }
    c
}

/// 実際のファイルシステムで (ファイルか, フォルダか)
pub fn exists_on_disk(v: &str) -> (bool, bool) {
    let p = Path::new(v);
    (p.is_file(), p.is_dir())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const SAMPLE: &str = "[project]\r\nversion=2011300\r\nfile=D:\\作品\\a.aup2\r\n\
[scene.0]\r\nscene=0\r\nname=Root\r\noutput.file=D:\\出力\\a.mp4\r\n\
[0]\r\nlayer=0\r\nframe=0,59\r\n[0.0]\r\neffect.name=画像ファイル\r\nファイル=D:\\素材\\bg.png\r\n連番ファイル=0\r\n[0.1]\r\neffect.name=標準描画\r\nX=0.00\r\n\
[1]\r\nlayer=1\r\nframe=0,59\r\n[1.0]\r\neffect.name=テキスト\r\nフォント=源ノ角ゴシック\r\nテキスト=D:\\素材\\bg.png は文字\r\n\
[2]\r\nlayer=2\r\nframe=0,59\r\n[2.0]\r\neffect.name=画像ファイル\r\nファイル=E:\\連番\\x_0001.png\r\n連番ファイル=1\r\n\
[3]\r\nlayer=3\r\nframe=0,59\r\n[3.0]\r\neffect.name=謎のスクリプト@未導入\r\n画像フォルダ=D:\\素材\\群\r\n関数ファイル=D:\\無い.lua\r\n出力SVG=D:\\出力\\x.svg\r\n\
[4]\r\nlayer=4\r\nframe=0,59\r\n[4.0]\r\neffect.name=SVG書き出し\r\n出力SVG=D:\\出力\\y.svg\r\n\
[plugin.0]\r\nplugin.name=Foo\r\npath=D:\\素材\\bg.png\r\n";

    pub fn types() -> ItemTypes {
        let mut t = ItemTypes::new();
        t.insert("画像ファイル".into(), HashMap::from([("ファイル".into(), ItemKind::File), ("連番ファイル".into(), ItemKind::Other)]));
        t.insert("標準描画".into(), HashMap::from([("X".into(), ItemKind::Other)]));
        t.insert("テキスト".into(), HashMap::from([("フォント".into(), ItemKind::Font), ("テキスト".into(), ItemKind::Other)]));
        // 出力の項目も種類はファイル（区別できない前提で、名前で外す）
        t.insert("SVG書き出し".into(), HashMap::from([("出力SVG".into(), ItemKind::File)]));
        t
    }

    #[test]
    fn scans_effect_entries_with_ranges() {
        let s = scan(SAMPLE);
        let r = s.project_file.clone().unwrap();
        assert_eq!(&SAMPLE[r], "D:\\作品\\a.aup2");
        let f = s.entries.iter().find(|e| e.key == "ファイル" && e.effect == "画像ファイル").unwrap();
        assert_eq!(&SAMPLE[f.range.clone()], "D:\\素材\\bg.png", "範囲は値だけ（\\r を含まない）");
        assert!(!f.sequence);
        let seq = s.entries.iter().find(|e| e.value.contains("x_0001")).unwrap();
        assert!(seq.sequence, "連番ファイル=1 が後ろにあっても同じ節の項目に印が付く");
        assert!(s.entries.iter().all(|e| e.key != "output.file" && e.key != "path"), "シーンと plugin の節は見ない");
        assert_eq!(s.effects.get("画像ファイル"), Some(&2));
    }

    #[test]
    fn classifies_files_folders_fonts() {
        let s = scan(SAMPLE);
        let exists = |v: &str| match v {
            "D:\\素材\\群" => (false, true),
            "D:\\素材\\bg.png" | "E:\\連番\\x_0001.png" => (true, false),
            _ => (false, false),
        };
        let c = classify(&s, &types(), exists);
        let picked: Vec<(&str, bool, Origin)> =
            c.refs.iter().map(|r| (s.entries[r.entry].value.as_str(), r.folder, r.origin)).collect();
        assert_eq!(
            picked,
            [
                ("D:\\素材\\bg.png", false, Origin::Typed),
                ("E:\\連番\\x_0001.png", false, Origin::Typed),
                ("D:\\素材\\群", true, Origin::Guessed),
            ],
            "テキストの中のパス・無いファイルの推定・出力の項目は拾わない"
        );
        assert_eq!(c.fonts.iter().collect::<Vec<_>>(), ["源ノ角ゴシック"]);
        assert!(c.unknown_effects.contains("謎のスクリプト@未導入"));
    }

    #[test]
    fn figure_with_a_file_path_is_picked() {
        let text = "[0]\r\n[0.0]\r\neffect.name=図形\r\n図形の種類=円\r\n[1]\r\n[1.0]\r\neffect.name=図形\r\n図形の種類=D:\\素材\\svg\\ト音記号2.svg\r\n";
        let s = scan(text);
        let mut t = ItemTypes::new();
        t.insert("図形".into(), HashMap::from([("図形の種類".into(), ItemKind::NameOrFile)]));
        let c = classify(&s, &t, |_| (false, false));
        let picked: Vec<&str> = c.refs.iter().map(|r| s.entries[r.entry].value.as_str()).collect();
        assert_eq!(picked, ["D:\\素材\\svg\\ト音記号2.svg"], "図形の名前（円）は拾わず、パスは見つからなくても拾う");

        // 本体がファイル以外の種類で返した項目（実機の「図形の種類」）は、実在するファイルなら推定で拾う。テキストとフォルダは拾わない
        let mut t = ItemTypes::new();
        t.insert(
            "図形".into(),
            HashMap::from([("図形の種類".into(), ItemKind::Other), ("文字".into(), ItemKind::Text), ("置き場".into(), ItemKind::Other)]),
        );
        let text = "[0]\r\n[0.0]\r\neffect.name=図形\r\n図形の種類=D:\\素材\\a.svg\r\n文字=D:\\素材\\a.svg\r\n置き場=D:\\素材\r\n";
        let s = scan(text);
        let c = classify(&s, &t, |v| (v == "D:\\素材\\a.svg", v == "D:\\素材"));
        let picked: Vec<(&str, Origin)> = c.refs.iter().map(|r| (s.entries[r.entry].key.as_str(), r.origin)).collect();
        assert_eq!(picked, [("図形の種類", Origin::Guessed)]);
    }

    #[test]
    fn absolute_path_shapes() {
        assert!(looks_absolute("C:\\a"));
        assert!(looks_absolute("c:/a"));
        assert!(looks_absolute("\\\\server\\share\\a"));
        assert!(!looks_absolute("素材\\a.png"));
        assert!(!looks_absolute("C:"));
        assert!(!looks_absolute("12.5"));
    }

    #[test]
    fn broken_input_does_not_panic() {
        for t in ["", "\u{feff}[project]\nfile=", "[0.0\nファイル=x", "[0.0]\n=\n==\nfile", "[project]\r\nfile=途中で", "[0.0]\r"] {
            let s = scan(t);
            let _ = classify(&s, &types(), |_| (true, false));
        }
        // BOM 付きでも値の範囲は正しい
        let t = "\u{feff}[project]\nfile=C:\\x.aup2";
        let s = scan(t);
        assert_eq!(&t[s.project_file.unwrap()], "C:\\x.aup2");
    }
}
