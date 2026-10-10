//! `.aup2` の行単位の置き換え。差し替える値の範囲だけを書き換え、ほかの 1 バイトも変えない
//! （パーサーで組み立て直さない。改行コード・BOM・並び・知らない節はそのまま）。

use std::ops::Range;

/// `text` の `edits`（範囲と新しい値）を置き換えた文字列。範囲が重なる・はみ出す・文字の境目でないなら Err
pub fn apply(text: &str, edits: &[(Range<usize>, String)]) -> Result<String, String> {
    let mut sorted: Vec<&(Range<usize>, String)> = edits.iter().collect();
    sorted.sort_by_key(|(r, _)| r.start);
    let mut out = String::with_capacity(text.len() + edits.iter().map(|(_, v)| v.len()).sum::<usize>());
    let mut pos = 0;
    for (r, v) in sorted {
        if r.start < pos || r.end < r.start || r.end > text.len() || !text.is_char_boundary(r.start) || !text.is_char_boundary(r.end) {
            return Err(format!("置き換える範囲 {}..{} がおかしい", r.start, r.end));
        }
        if v.contains(['\r', '\n']) {
            return Err("新しい値に改行が入っている".into());
        }
        out.push_str(&text[pos..r.start]);
        out.push_str(v);
        pos = r.end;
    }
    out.push_str(&text[pos..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::refs::{self, tests::SAMPLE};

    #[test]
    fn only_values_change() {
        let s = refs::scan(SAMPLE);
        let f = s.entries.iter().find(|e| e.value == "D:\\素材\\bg.png" && e.key == "ファイル").unwrap();
        let edits = vec![
            (f.range.clone(), "E:\\収集\\a\\素材\\bg.png".to_string()),
            (s.project_file.clone().unwrap(), "E:\\収集\\a\\a.aup2".to_string()),
        ];
        let out = apply(SAMPLE, &edits).unwrap();
        let expect = SAMPLE
            .replacen("ファイル=D:\\素材\\bg.png\r\n", "ファイル=E:\\収集\\a\\素材\\bg.png\r\n", 1)
            .replacen("file=D:\\作品\\a.aup2\r\n", "file=E:\\収集\\a\\a.aup2\r\n", 1);
        assert_eq!(out, expect, "差し替えた 2 か所以外は同じ（CRLF も、テキストの中の同じ文字列も、plugin の節も）");
        // 置き換えた後に読み直すと、新しい値が同じ項目に入っている
        let again = refs::scan(&out);
        assert!(again.entries.iter().any(|e| e.key == "ファイル" && e.value == "E:\\収集\\a\\素材\\bg.png"));
        assert_eq!(again.entries.len(), s.entries.len());
    }

    #[test]
    fn rejects_bad_edits() {
        let t = "ab=cd\r\n";
        assert!(apply(t, &[(3..5, "x".into()), (4..5, "y".into())]).is_err(), "重なり");
        assert!(apply(t, &[(3..99, "x".into())]).is_err(), "はみ出し");
        assert!(apply("あ=い", &[(1..2, "x".into())]).is_err(), "文字の途中");
        assert!(apply(t, &[(3..5, "x\r\ny".into())]).is_err(), "改行入り");
        assert_eq!(apply(t, &[]).unwrap(), t);
    }
}
