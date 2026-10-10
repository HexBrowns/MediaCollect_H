//! 実用の規模での時間の目安（ルール au2-plugin-checklist「実用の規模で確かめる」）。
//! 見本: 30 分のゆっくり実況を想定。音声 3000 個（台詞ごとに別のファイル）・立ち絵と背景の画像 300 個を 1500 か所で使う・
//! 画像フォルダ 1 つ（中に 5000 個）・テキスト 3000 個（フォント 3 種）。効果の行はおよそ 8 万行。
//! 本物のファイルを一時フォルダに作り、一覧を作る（読む・分ける・大きさを見る）と書き出しを、本番と同じ関数で通す。
//! 時間は機械で変わるので、既定では走らせない:
//!   cargo test --release --test scale -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use media_collect_h::copy::{Copier, Job};
use media_collect_h::plan::{self, Disk};
use media_collect_h::refs::{self, ItemKind, ItemTypes};

const VOICES: usize = 3000;
const IMAGES: usize = 300;
const IMAGE_USES: usize = 1500;
const FOLDER_FILES: usize = 5000;

fn types() -> ItemTypes {
    let mut t = ItemTypes::new();
    t.insert("画像ファイル".into(), [("ファイル".to_string(), ItemKind::File), ("連番ファイル".into(), ItemKind::Other)].into());
    t.insert("音声ファイル".into(), [("ファイル".to_string(), ItemKind::File)].into());
    t.insert("テキスト".into(), [("フォント".to_string(), ItemKind::Font), ("テキスト".into(), ItemKind::Other)].into());
    t.insert("画像フォルダ@未導入".into(), [("フォルダ".to_string(), ItemKind::Folder)].into());
    t.insert("標準描画".into(), [("X".to_string(), ItemKind::Other)].into());
    t.insert("標準再生".into(), [("音量".to_string(), ItemKind::Other)].into());
    t
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// 見本の素材と `.aup2` を作る
fn make_sample(root: &Path) -> (PathBuf, String) {
    let media = root.join("素材");
    for sub in ["voice", "image", "群"] {
        std::fs::create_dir_all(media.join(sub)).unwrap();
    }
    let small = vec![7u8; 4096];
    for i in 0..VOICES {
        std::fs::write(media.join("voice").join(format!("{i:05}_台詞.wav")), &small).unwrap();
    }
    for i in 0..IMAGES {
        std::fs::write(media.join("image").join(format!("立ち絵_{i:03}.png")), &small).unwrap();
    }
    for i in 0..FOLDER_FILES {
        std::fs::write(media.join("群").join(format!("{i:05}.png")), &small[..512]).unwrap();
    }
    let project = root.join("作品").join("作品.aup2");
    std::fs::create_dir_all(project.parent().unwrap()).unwrap();
    let mut t = format!("[project]\r\nversion=2011300\r\nfile={}\r\n[scene.0]\r\nscene=0\r\nname=Root\r\n", project.display());
    let mut obj = 0;
    fn push_obj(t: &mut String, obj: &mut usize, layer: usize, frame: usize, body: &str) {
        t.push_str(&format!("[{obj}]\r\nlayer={layer}\r\nframe={frame},{}\r\n{body}", frame + 59));
        *obj += 1;
    }
    let draw = |o: usize, e: usize| {
        format!(
            "[{o}.{e}]\r\neffect.name=標準描画\r\nX=0.00\r\nY=0.00\r\nZ=0.00\r\nGroup=1\r\n中心X=0.00\r\n中心Y=0.00\r\n中心Z=0.00\r\n\
X軸回転=0.00\r\nY軸回転=0.00\r\nZ軸回転=0.00\r\n拡大率=100.000\r\n縦横比=0.000\r\n透明度=0.00\r\n合成モード=通常\r\n"
        )
    };
    for i in 0..VOICES {
        let o = obj;
        let v = media.join("voice").join(format!("{i:05}_台詞.wav"));
        push_obj(&mut t, &mut obj, 1, i * 60, &format!("[{o}.0]\r\neffect.name=音声ファイル\r\n再生位置=0.000\r\n再生速度=100.00\r\nファイル={}\r\nトラック=0\r\n[{o}.1]\r\neffect.name=標準再生\r\n音量=100.00\r\n左右=0.00\r\n", v.display()));
        let o = obj;
        let font = ["源ノ角ゴシック JP Bold", "Rounded M+ 1c", "メイリオ"][i % 3];
        push_obj(&mut t, &mut obj, 2, i * 60, &format!("[{o}.0]\r\neffect.name=テキスト\r\nサイズ=48.00\r\n字間=0.00\r\nフォント={font}\r\n文字色=ffffff\r\nテキスト=台詞 {i} の字幕\r\n{}", draw(o, 1)));
    }
    for i in 0..IMAGE_USES {
        let o = obj;
        let p = media.join("image").join(format!("立ち絵_{:03}.png", i % IMAGES));
        push_obj(&mut t, &mut obj, 3, i * 120, &format!("[{o}.0]\r\neffect.name=画像ファイル\r\nファイル={}\r\n連番ファイル=0\r\n{}", p.display(), draw(o, 1)));
    }
    let o = obj;
    push_obj(&mut t, &mut obj, 4, 0, &format!("[{o}.0]\r\neffect.name=画像フォルダ@未導入\r\nフォルダ={}\r\n", media.join("群").display()));
    std::fs::write(&project, &t).unwrap();
    (project, t)
}

#[test]
#[ignore]
fn listing_and_writing_at_scale() {
    let root = std::env::temp_dir().join(format!("mediacollect_scale_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let t0 = Instant::now();
    let (project, _) = make_sample(&root);
    println!("[INFO] 見本を作る: {:.0} ms", ms(t0.elapsed()));

    // 一覧を作る（gui.rs の make_listing と同じ順。種類を本体に聞く所だけ表で代える）
    let t = Instant::now();
    let text = String::from_utf8_lossy(&std::fs::read(&project).unwrap()).into_owned();
    let read = t.elapsed();
    let t = Instant::now();
    let scan = refs::scan(&text);
    let scanned = t.elapsed();
    let t = Instant::now();
    let disk = Disk::default();
    let c = refs::classify(&scan, &types(), |v| disk.exists(v));
    let classified = t.elapsed();
    let t = Instant::now();
    let p = plan::build(&scan, &c, &[], None, &disk);
    let planned = t.elapsed();
    println!(
        "[INFO] {} KB・効果の行 {}・参照 {}・元 {}・コピー {} 個",
        text.len() / 1024,
        scan.entries.len(),
        c.refs.len(),
        p.sources.len(),
        p.copy_count()
    );
    println!(
        "[INFO] 一覧を作る: 読む {:.1} / 行を読む {:.1} / 分ける {:.1} / 大きさを見る {:.1} ms",
        ms(read),
        ms(scanned),
        ms(classified),
        ms(planned)
    );
    assert_eq!(p.sources.len(), VOICES + IMAGES + 1);
    assert_eq!(p.copy_count(), VOICES + IMAGES + FOLDER_FILES);

    // 書き出し先の確かめ（gui.rs の check_target と同じ。書き出し先・一覧が変わったときとボタンを押したときだけ）
    let target = root.join("出力").join("作品_収集");
    let t = Instant::now();
    let n = 20;
    for _ in 0..n {
        let nonempty = target.exists() && std::fs::read_dir(&target).map(|mut d| d.next().is_some()).unwrap_or(true);
        let _ = plan::check_target(&p, &project.display().to_string(), &target, nonempty);
        let _ = media_collect_h::dialog::free_bytes(&target);
    }
    println!("[INFO] 書き出し先の確かめ 1 回（ローカルのドライブ）: {:.2} ms", ms(t.elapsed()) / n as f64);

    // 書き出し（裏のスレッド。on_change はウィンドウの描き直しを頼む回数）
    let changes = Arc::new(AtomicUsize::new(0));
    let ch = Arc::clone(&changes);
    let job = Job {
        project: project.clone(),
        text: text.clone(),
        scan: scan.clone(),
        plan: p.clone(),
        fonts: c.fonts.iter().cloned().collect(),
        unknown_effects: c.unknown_effects.iter().cloned().collect(),
        target: target.clone(),
        name: "作品_収集".into(),
    };
    let t = Instant::now();
    let copier = Copier::start(job, move || {
        ch.fetch_add(1, Ordering::Relaxed);
    });
    while !copier.finished() {
        std::thread::sleep(Duration::from_millis(5));
    }
    let took = t.elapsed();
    let result = copier.progress.lock().result.clone().unwrap();
    let report = result.expect("書き出しが失敗した");
    println!(
        "[INFO] 書き出し: {:.0} ms（ファイル {} 個・描き直しの依頼 {} 回・読み直して無かったパス {}）",
        ms(took),
        report.files,
        changes.load(Ordering::Relaxed),
        report.broken.len()
    );
    assert_eq!(report.files, VOICES + IMAGES + FOLDER_FILES);
    assert!(report.broken.is_empty());
    drop(copier);
    let _ = std::fs::remove_dir_all(&root);
}

/// つながっていない NAS の素材が 100 個あっても、一覧を作るのは根を 1 回見る分（1 秒あまり）で済む
#[test]
#[ignore]
fn many_sources_on_unreachable_share() {
    let mut text = String::from("[project]\r\nfile=D:\\作品\\作品.aup2\r\n");
    for i in 0..100 {
        text.push_str(&format!(
            "[{i}]\r\nlayer=1\r\n[{i}.0]\r\neffect.name=音声ファイル\r\nファイル=\\\\mediacollect-no-such-nas\\share\\voice\\{i:03}.wav\r\n"
        ));
    }
    // 入っていない効果（推定）の分も、同じ根なら見に行かない
    text.push_str("[100]\r\nlayer=2\r\n[100.0]\r\neffect.name=謎@未導入\r\n画像=\\\\mediacollect-no-such-nas\\share\\a.png\r\n");
    let scan = refs::scan(&text);
    let t = Instant::now();
    let disk = Disk::default();
    let c = refs::classify(&scan, &types(), |v| disk.exists(v));
    let p = plan::build(&scan, &c, &[], None, &disk);
    let took = t.elapsed();
    println!(
        "[INFO] 届かない共有フォルダの素材 100 個: 一覧 {:.0} ms（見つからない {}）",
        ms(took),
        p.count(|s| s.state == plan::State::Missing)
    );
    assert_eq!(p.sources.len(), 100);
    assert!(took < Duration::from_secs(10), "根ごとに 1 回だけ見るなら 100 回分（2 分）かからない");
}

/// 無いサーバーの UNC パス（打っている途中の `\\nas\…` や、つながっていない NAS）を 1 回確かめる時間
#[test]
#[ignore]
fn unreachable_unc_path() {
    for p in [r"\\mediacollect-no-such-host\share\a.wav", r"\\mediacollect-no-such-host", r"Z:\無い\a.wav"] {
        let t = Instant::now();
        let e = Path::new(p).exists();
        println!("[INFO] exists({p}) = {e}: {:.0} ms", ms(t.elapsed()));
        let t = Instant::now();
        let f = media_collect_h::dialog::free_bytes(Path::new(p));
        println!("[INFO] free_bytes({p}) = {f:?}: {:.0} ms", ms(t.elapsed()));
    }
}
