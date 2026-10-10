# MediaCollect_H（素材の収集）

保存済みの `.aup2` が参照するファイル・フォルダを 1 つのフォルダへコピーし、パスをコピー先に書き換えた**別名の** `.aup2` を書き出す AviUtl2 の汎用プラグイン。
**元のプロジェクトと素材には書き込まない。** YMM4 の「YMM4素材同梱プラグイン」に当たる（README の説明だけを元にした。コードは使っていない）。

- 仕様: `AI/specifications/20261009_MediaCollect_H_spec.md`
- 本体 2.1.11 以降（aviutl2-rs 0.48）

## ビルドと配置

```bash
python AI/tools/au2_build.py MediaCollect_H                 # 本番へ
python AI/tools/au2_build.py MediaCollect_H --no-deploy     # 本番に書かず、違いだけ出す
cargo run --release --example scan -- "<.aup2 のパス>"       # 本体なしで、拾う参照とコピーの計画を出す（種類はすべて推定）
```

## 作りの要点

| ファイル | 中身 |
|---|---|
| `src/refs.rs` | `.aup2` の効果の節を、値のバイトの範囲つきで読む。項目の種類（ファイル・フォルダ・フォント）は本体に聞き、分からない効果は実在する絶対パスを推定で拾う。項目名に「出力」を含むものは拾わない |
| `src/plan.rs` | コピーの計画。同じ元パスは 1 回、同じ名前で元が違うものは `名前_<元パスの 8 桁>`、除外・見つからないはパスを残す。書き出し先の確かめ |
| `src/rewrite.rs` | 値の範囲だけを差し替える（ほかの 1 バイトも変えない） |
| `src/copy.rs` | 書き出しのスレッド。8MB ずつ `.part` に書いてから名前を変える・中止・大きさの照合・更新日時。`.aup2`・CurveEditor_H の隣のファイル・`収集.json`・`受け手に要るもの.txt`。書いた `.aup2` を読み直して確かめる |
| `src/settings.rs` | 出力先と除外のフォルダ（`Plugin/MediaCollect_H/settings.json`。読めなければ退避して上書きしない） |
| `src/gui.rs` | ウィンドウ（egui）。1 行の欄は Esc で入力前に戻す |
| `src/dialog.rs` | フォルダを選ぶダイアログ・日時・空き容量・エクスプローラー |

- 本体の API は、「一覧を作る」を押したときに効果ごとの項目の種類を聞くだけ（`enumerate_effect_items`。起動の途中には呼ばない）
- プロジェクトは書き換えない（`call_edit_section` を持たない）。読むのは保存済みの `.aup2`
- 自分で起こすスレッドは書き出しの 1 本。プラグインの `Drop` で止めて終わりを待つ
