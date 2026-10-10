//! 素材の収集のウィンドウ（egui）
//!
//! 上: 対象のプロジェクト・出力先・名前。中: 一覧（種類ごとの数と大きさ、詳しい一覧、除外のフォルダ）。下: 書き出す・進み具合・結果。
//! 読むのは保存済みの `.aup2` だけで、本体の API はボタンを押したときに項目の種類を聞くだけ（`enumerate_effect_items`）。
//!
//! 重さ（ルール au2-rs-plugin「ウィンドウの重さ」）:
//! - **ディスクを 1 フレームごとに見ない。** 書き出し先の確かめ（空か・空き容量）は、書き出し先・一覧・書き出しが変わったときと
//!   ボタンを押したときだけ。つながっていない NAS のパスは 1 回見るのに 1 秒以上かかる（実測 1.3 秒）。
//!   出力先と名前の欄は確定したとき（Enter・ほかをクリック）だけ書き出し先に入れる（打っている途中の `\\n` などを見に行かない）
//! - 対象のプロジェクトの保存日時は 1 秒に 1 回まで見る
//! - 数（種類ごと）と詳しい一覧の並びは一覧を作ったときに 1 回だけ数える。詳しい一覧は見えている行だけを描く（素材が数千でも同じ重さ）

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use aviutl2::generic::EffectItemType;
use aviutl2_eframe::{eframe, egui, AviUtl2EframeHandle};

use crate::copy::{Copier, Job};
use crate::plan::{self, Plan, State};
use crate::refs::{self, Classified, ItemKind, ItemTypes, Scan};
use crate::settings::{self, Settings};
use crate::{dialog, COPIER, EDIT_HANDLE, PROJECT};

const RED: egui::Color32 = egui::Color32::from_rgb(235, 110, 110);
const YELLOW: egui::Color32 = egui::Color32::from_rgb(225, 190, 90);
const DIM: egui::Color32 = egui::Color32::from_rgb(150, 150, 150);

/// 対象のプロジェクトの保存日時を見る間隔
const PROJECT_POLL: Duration = Duration::from_secs(1);

/// 1 行の入力欄。Esc で入力前の文字に戻す（ルール au2-rs-plugin「入力の確定と取り消し」。MidpointTable_H の `number_text` と同じ）。
/// 確定したら true
fn line_edit(ui: &mut egui::Ui, text: &mut String, width: f32) -> bool {
    let resp = ui.add(egui::TextEdit::singleline(text).desired_width(width));
    let key = resp.id.with("before_edit");
    if resp.gained_focus() {
        ui.data_mut(|d| d.insert_temp(key, text.clone()));
    }
    if resp.lost_focus() {
        let before = ui.data_mut(|d| d.remove_temp::<String>(key));
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            if let Some(b) = before {
                *text = b;
            }
            return false;
        }
        return true;
    }
    false
}

pub fn bytes_text(b: u64) -> String {
    const K: f64 = 1024.0;
    let f = b as f64;
    if f >= K * K * K {
        format!("{:.1} GB", f / K / K / K)
    } else if f >= K * K {
        format!("{:.1} MB", f / K / K)
    } else if f >= K {
        format!("{:.0} KB", f / K)
    } else {
        format!("{b} B")
    }
}

/// 種類ごとの数（一覧を作ったときに 1 回だけ数える）
#[derive(Debug, Default, Clone, PartialEq)]
struct Summary {
    files: usize,
    folders: usize,
    renamed: usize,
    guessed: usize,
    missing: usize,
    excluded: usize,
    sequence: usize,
    relinked: usize,
    copy_count: usize,
    copy_bytes: u64,
}

impl Summary {
    fn of(p: &Plan) -> Summary {
        let copy = |s: &plan::Source| matches!(s.state, State::Copy { .. });
        Summary {
            relinked: p.count(|s| s.relinked),
            files: p.count(|s| copy(s) && !s.folder),
            folders: p.count(|s| copy(s) && s.folder),
            renamed: p.count(|s| matches!(s.state, State::Copy { renamed: true, .. })),
            guessed: p.count(|s| s.guessed),
            missing: p.count(|s| s.state == State::Missing),
            excluded: p.count(|s| s.state == State::Excluded),
            sequence: p.count(|s| s.sequence),
            copy_count: p.copy_count(),
            copy_bytes: p.copy_bytes(),
        }
    }
}

/// 詳しい一覧の並び。見つからないものを先に（数千の中から探さずに済むように）、次にコピーするもの、最後に除外
fn detail_order(p: &Plan) -> Vec<usize> {
    let rank = |s: &plan::Source| match s.state {
        State::Missing => 0,
        State::Copy { .. } => 1,
        State::Excluded => 2,
    };
    let mut order: Vec<usize> = (0..p.sources.len()).collect();
    order.sort_by_key(|&i| rank(&p.sources[i]));
    order
}

/// 詳しい一覧。見えている行だけを描く
fn detail_list(ui: &mut egui::Ui, p: &Plan, order: &[usize]) {
    let row_h = ui.text_style_height(&egui::TextStyle::Body);
    egui::ScrollArea::both().id_salt("detail_rows").max_height(220.0).auto_shrink([false, true]).show_rows(
        ui,
        row_h,
        order.len(),
        |ui, range| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for &i in &order[range] {
                let s = &p.sources[i];
                let (mark, color) = match &s.state {
                    State::Copy { .. } => ("コピー", ui.visuals().text_color()),
                    State::Excluded => ("除外", DIM),
                    State::Missing => ("見つからない", YELLOW),
                };
                ui.horizontal(|ui| {
                    ui.add_sized([84.0, row_h], egui::Label::new(egui::RichText::new(mark).color(color)));
                    ui.add_sized([52.0, row_h], egui::Label::new(if s.folder { "フォルダ" } else { "ファイル" }));
                    ui.label(&s.from);
                    if let State::Copy { dest, .. } = &s.state {
                        ui.colored_label(DIM, format!("→ {}（{}）", dest.display(), bytes_text(s.bytes)));
                    }
                    if s.relinked {
                        ui.colored_label(DIM, format!("（.aup2 では {}）", s.path));
                    }
                });
            }
        },
    );
}

/// 一覧を作った結果
struct Listing {
    /// 作るたびに変わる番号（書き出し先の確かめを作り直す印）
    id: u64,
    project: PathBuf,
    modified: Option<SystemTime>,
    text: String,
    scan: Scan,
    classified: Classified,
    plan: Plan,
    summary: Summary,
    order: Vec<usize>,
}

/// 対象のプロジェクトの保存日時（`PROJECT_POLL` に 1 回まで見る）
struct ProjectSeen {
    path: Option<PathBuf>,
    at: Instant,
    modified: Option<SystemTime>,
    label: Option<String>,
}

/// 書き出し先の確かめ。`key` が変わったときとボタンを押したときだけ作り直す
#[derive(Clone, PartialEq)]
struct CheckKey {
    target: Option<PathBuf>,
    listing: u64,
    runs: u64,
    done: bool,
}

struct TargetCheck {
    key: CheckKey,
    result: Result<(), String>,
    free: Option<u64>,
}

/// 書き出し先を確かめる（ディスクを見る。1 フレームごとには呼ばない）
fn check_target(l: &Listing, target: Option<&Path>) -> (Result<(), String>, Option<u64>) {
    let Some(t) = target else { return (Err("出力先と名前を決めてください".to_string()), None) };
    let free = dialog::free_bytes(t);
    let nonempty = t.exists() && std::fs::read_dir(t).map(|mut d| d.next().is_some()).unwrap_or(true);
    let need = l.summary.copy_bytes;
    let result = plan::check_target(&l.plan, &l.project.display().to_string(), t, nonempty).and_then(|_| match free {
        Some(free) if free < need + 64 * 1024 * 1024 => {
            Err(format!("空きが足りません（空き {} / 必要 {}）", bytes_text(free), bytes_text(need)))
        }
        _ => Ok(()),
    });
    (result, free)
}

pub struct App {
    _handle: AviUtl2EframeHandle,
    settings: Settings,
    can_save: bool,
    /// 出力先の欄（確定したら `settings.out_dir` に入れる）
    out_text: String,
    /// 書き出しに使う名前（確定したもの）と、名前の欄
    name: String,
    name_text: String,
    /// `name` を既定の名前にしたプロジェクト
    name_for: Option<PathBuf>,
    excludes_text: String,
    listing: Option<Listing>,
    listings_made: u64,
    /// 書き出しを始めた回数
    runs: u64,
    check: Option<TargetCheck>,
    seen: ProjectSeen,
    status: String,
    status_error: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, handle: AviUtl2EframeHandle) -> Self {
        cc.egui_ctx.all_styles_mut(|style| {
            style.visuals = aviutl2_eframe::aviutl2_visuals();
        });
        cc.egui_ctx.set_fonts(aviutl2_eframe::aviutl2_fonts());
        let (settings, can_save) = settings::load();
        let excludes_text = settings.excludes.join("\n");
        App {
            _handle: handle,
            out_text: settings.out_dir.clone(),
            settings,
            can_save,
            name: String::new(),
            name_text: String::new(),
            name_for: None,
            excludes_text,
            listing: None,
            listings_made: 0,
            runs: 0,
            check: None,
            seen: ProjectSeen { path: None, at: Instant::now(), modified: None, label: None },
            status: String::new(),
            status_error: false,
        }
    }

    fn save_settings(&self) {
        if self.can_save {
            settings::save(&self.settings);
        }
    }

    /// 書き出し先（確定した出力先と名前から）
    fn target(&self) -> Option<PathBuf> {
        let out = self.settings.out_dir.trim();
        let name = self.name.trim();
        (!out.is_empty() && !name.is_empty()).then(|| Path::new(out).join(name))
    }

    fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = text.into();
        self.status_error = error;
    }

    /// 対象のプロジェクトの保存日時を、変わったときか `PROJECT_POLL` ごとに見る
    fn refresh_seen(&mut self, project: Option<&PathBuf>) {
        if self.seen.path.as_ref() == project && self.seen.at.elapsed() < PROJECT_POLL {
            return;
        }
        self.seen.path = project.cloned();
        self.seen.at = Instant::now();
        self.seen.modified = project.and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        self.seen.label = project.and_then(|p| dialog::modified_text(p));
    }

    /// 項目の種類を本体に聞く（ボタンを押したときだけ。起動の途中に効果を列挙すると例外になることがある。ルール au2-host-exceptions）
    /// (項目の種類, 本体が返した種類の名前（ログ用。効果名と項目名から）)
    fn item_types(scan: &Scan) -> (ItemTypes, HashMap<(String, String), String>) {
        let mut types = ItemTypes::new();
        let mut raw = HashMap::new();
        if !EDIT_HANDLE.is_ready() {
            return (types, raw);
        }
        for name in scan.effects.keys() {
            let mut items = HashMap::new();
            let res = EDIT_HANDLE.enumerate_effect_items(name, |info| {
                raw.insert((name.clone(), info.name.clone()), format!("{:?}", info.item_type));
                let kind = match info.item_type {
                    EffectItemType::File => ItemKind::File,
                    EffectItemType::Folder => ItemKind::Folder,
                    EffectItemType::Font => ItemKind::Font,
                    // 図形の「図形の種類」は Combo で返る（実機ログ 2026-10-10）
                    EffectItemType::Combo | EffectItemType::Figure | EffectItemType::Mask => ItemKind::NameOrFile,
                    EffectItemType::Text => ItemKind::Text,
                    _ => ItemKind::Other,
                };
                items.insert(info.name, kind);
            });
            // 種類が 1 つも取れない効果は「分からない」として推定に回す
            if res.is_ok() && !items.is_empty() {
                types.insert(name.clone(), items);
            }
        }
        (types, raw)
    }

    fn make_listing(&mut self, project: &Path) {
        let t0 = Instant::now();
        let text = match std::fs::read(project) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) => {
                self.set_status(format!("{} を読めません: {e}", project.display()), true);
                return;
            }
        };
        let modified = std::fs::metadata(project).and_then(|m| m.modified()).ok();
        let scan = refs::scan(&text);
        let t1 = Instant::now();
        let (types, raw_types) = Self::item_types(&scan);
        let t2 = Instant::now();
        let disk = plan::Disk::default();
        // プロジェクトを動かした後なら、本体と同じく保存した場所のフォルダの中を今のフォルダへ読み替える
        let relink = plan::Relink::new(scan.project_saved.as_deref(), project);
        let exists = |v: &str| match disk.exists(v) {
            (false, false) => relink.as_ref().and_then(|r| r.map(v)).map(|m| disk.exists(&m)).unwrap_or((false, false)),
            found => found,
        };
        let classified = refs::classify(&scan, &types, exists);
        let plan = plan::build(&scan, &classified, &self.settings.excludes, relink.as_ref(), &disk);
        let t3 = Instant::now();
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        // 重さを後から読めるように、1 回押すごとに 1 行だけ出す（ルール au2-plugin-checklist「実用の規模で確かめる」）
        tracing::info!(
            "[MediaCollect_H] 一覧: 効果の行 {}・参照 {}・元 {}（読み替え {}）を {:.0} ms（読む {:.0}・種類を聞く {:.0}・ファイルを見る {:.0}）",
            scan.entries.len(),
            classified.refs.len(),
            plan.sources.len(),
            plan.count(|s| s.relinked),
            ms(t3 - t0),
            ms(t1 - t0),
            ms(t2 - t1),
            ms(t3 - t2)
        );
        // 種類がファイル以外の項目（入っている効果）で、実在するパスだったので拾ったもの。どの種類かを 1 行で残す
        let mut odd: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for r in classified.refs.iter().filter(|r| r.origin == refs::Origin::Guessed) {
            let e = &scan.entries[r.entry];
            if types.contains_key(&e.effect) {
                let kind = raw_types.get(&(e.effect.clone(), e.key.clone())).map(String::as_str).unwrap_or("項目なし");
                *odd.entry(format!("{}/{}（{kind}）", e.effect, e.key)).or_default() += 1;
            }
        }
        if !odd.is_empty() {
            let list: Vec<String> = odd.iter().map(|(k, n)| format!("{k} {n}")).collect();
            tracing::info!("[MediaCollect_H] 種類がファイル以外の項目で拾ったもの: {}", list.join("・"));
        }
        self.set_status(
            format!("一覧を作りました（参照 {} か所・{:.1} 秒）", classified.refs.len(), (t3 - t0).as_secs_f32()),
            false,
        );
        self.listings_made += 1;
        let summary = Summary::of(&plan);
        let order = detail_order(&plan);
        self.listing = Some(Listing {
            id: self.listings_made,
            project: project.to_path_buf(),
            modified,
            text,
            scan,
            classified,
            plan,
            summary,
            order,
        });
    }

    fn render_top(&mut self, ui: &mut egui::Ui, project: Option<&PathBuf>) {
        egui::Grid::new("top").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("対象:");
            match project {
                Some(p) => {
                    let when = self.seen.label.as_ref().map(|t| format!("（保存 {t}）")).unwrap_or_default();
                    ui.label(format!("{}{when}", p.display()));
                }
                None => {
                    ui.colored_label(YELLOW, "プロジェクトを一度保存してください（保存済みの .aup2 を読みます）");
                }
            }
            ui.end_row();

            ui.label("出力先:");
            ui.horizontal(|ui| {
                if line_edit(ui, &mut self.out_text, 360.0) && self.out_text != self.settings.out_dir {
                    self.settings.out_dir = self.out_text.clone();
                    self.save_settings();
                }
                if ui.button("参照").clicked() {
                    if let Some(d) = dialog::pick_folder() {
                        self.settings.out_dir = d.display().to_string();
                        self.out_text = self.settings.out_dir.clone();
                        self.save_settings();
                    }
                }
            });
            ui.end_row();

            ui.label("名前:");
            ui.horizontal(|ui| {
                if line_edit(ui, &mut self.name_text, 200.0) {
                    self.name = self.name_text.clone();
                }
                // 打っている途中も、欄の文字で行き先を出す（文字を組むだけで、ディスクは見ない）
                let (out, name) = (self.out_text.trim(), self.name_text.trim());
                if out.is_empty() || name.is_empty() {
                    ui.colored_label(DIM, "→ 出力先と名前を決めてください");
                } else {
                    ui.colored_label(DIM, format!("→ {}", Path::new(out).join(name).display()));
                }
            });
            ui.end_row();
        });
        ui.horizontal(|ui| {
            let can = project.is_some() && COPIER.lock().as_ref().is_none_or(|c| c.finished());
            if ui.add_enabled(can, egui::Button::new("一覧を作る")).clicked() {
                if let Some(p) = project {
                    let p = p.clone();
                    self.make_listing(&p);
                }
            }
            if let Some(l) = &self.listing {
                if project != Some(&l.project) || self.seen.modified != l.modified {
                    ui.colored_label(YELLOW, "プロジェクトが変わりました。一覧を作り直してください");
                }
            }
        });
    }

    fn render_listing(&mut self, ui: &mut egui::Ui) {
        let Some(l) = &self.listing else {
            ui.colored_label(DIM, "「一覧を作る」で、プロジェクトが参照しているファイルを数えます");
            return;
        };
        let s = &l.summary;
        egui::Grid::new("summary").num_columns(3).spacing([16.0, 4.0]).striped(true).show(ui, |ui| {
            ui.strong("種類");
            ui.strong("数");
            ui.strong("状態");
            ui.end_row();
            ui.label("ファイル");
            ui.label(s.files.to_string());
            if s.renamed > 0 {
                ui.label(format!("同じ名前 {}（名前に印を付けて置く）", s.renamed));
            } else {
                ui.label("");
            }
            ui.end_row();
            ui.label("フォルダ");
            ui.label(s.folders.to_string());
            ui.label("中身ごとコピー");
            ui.end_row();
            if s.guessed > 0 {
                ui.label("推定");
                ui.label(s.guessed.to_string());
                ui.colored_label(YELLOW, "種類がファイルでない項目か、入っていない効果の項目。実在するファイルを指していたので拾った");
                ui.end_row();
            }
            if s.relinked > 0 {
                ui.label("読み替え");
                ui.label(s.relinked.to_string());
                ui.label("プロジェクトを動かす前のパス。本体と同じく、今のプロジェクトのフォルダで見つけた");
                ui.end_row();
            }
            ui.label("見つからない");
            ui.label(s.missing.to_string());
            if s.missing > 0 {
                ui.colored_label(YELLOW, "パスは元のまま残す（詳しい一覧の先頭に出す）");
            } else {
                ui.label("");
            }
            ui.end_row();
            ui.label("除外");
            ui.label(s.excluded.to_string());
            ui.label("コピーせずパスを残す");
            ui.end_row();
            if s.sequence > 0 {
                ui.label("連番");
                ui.label(s.sequence.to_string());
                ui.colored_label(YELLOW, "連番の画像は指定のファイルだけを写す（ほかの番号は写さない）");
                ui.end_row();
            }
            ui.label("フォント");
            ui.label(l.classified.fonts.len().to_string());
            ui.label("コピーしない（受け手に要るもの.txt に名前を出す）");
            ui.end_row();
        });
        ui.add_space(4.0);
        egui::CollapsingHeader::new(format!("詳しい一覧（{} 件）", l.plan.sources.len()))
            .id_salt("detail")
            .show(ui, |ui| detail_list(ui, &l.plan, &l.order));
    }

    fn render_excludes(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(format!("除外するフォルダ（{} 個）", self.settings.excludes.len())).id_salt("excludes").show(ui, |ui| {
            ui.colored_label(DIM, "1 行に 1 つ。この中にあるファイルはコピーせず、パスをそのまま残します");
            // 複数行の欄は Esc で戻さない（ルール au2-rs-plugin）。ほかをクリックしたときに使う
            let resp = ui.add(egui::TextEdit::multiline(&mut self.excludes_text).desired_rows(4).desired_width(f32::INFINITY));
            if resp.lost_focus() {
                let list: Vec<String> =
                    self.excludes_text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
                if list != self.settings.excludes {
                    self.settings.excludes = list;
                    self.save_settings();
                    self.listing = None;
                    self.set_status("除外を変えました。一覧を作り直してください", false);
                }
            }
            if ui.small_button("既定に戻す").clicked() {
                self.settings.excludes = settings::default_excludes();
                self.excludes_text = self.settings.excludes.join("\n");
                self.save_settings();
                self.listing = None;
            }
        });
    }

    fn render_bottom(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // 書き出し中・書き出した後
        let mut copier_guard = COPIER.lock();
        let mut done = false;
        if let Some(c) = copier_guard.as_ref() {
            let p = c.progress.lock();
            match &p.result {
                None => {
                    let ratio = if p.total_bytes > 0 { p.done_bytes as f32 / p.total_bytes as f32 } else { 0.0 };
                    ui.horizontal(|ui| {
                        ui.add(egui::ProgressBar::new(ratio).show_percentage().desired_width(240.0));
                        ui.label(format!("{} / {}", bytes_text(p.done_bytes), bytes_text(p.total_bytes)));
                        if ui.button("中止").clicked() {
                            c.cancel();
                        }
                    });
                    ui.colored_label(DIM, egui::RichText::new(&p.current).small());
                    ctx.request_repaint_after(Duration::from_millis(200));
                    return;
                }
                Some(Ok(r)) => {
                    done = true;
                    let msg = format!(
                        "書き出しました: {}（ファイル {} 個・{}・{:.1} 秒）",
                        r.aup2.display(),
                        r.files,
                        bytes_text(r.bytes),
                        r.seconds
                    );
                    let target = r.target.clone();
                    let broken = r.broken.clone();
                    drop(p);
                    ui.horizontal(|ui| {
                        ui.label(msg);
                        if ui.button("フォルダを開く").clicked() {
                            dialog::open_folder(&target);
                        }
                    });
                    if !broken.is_empty() {
                        ui.colored_label(RED, format!("書いたプロジェクトを読み直したら、{} 個のパスが見つかりません: {}", broken.len(), broken.join(" / ")));
                    }
                    ui.colored_label(DIM, "パスは絶対パスです。フォルダを動かすと素材のつながりが切れます");
                }
                Some(Err(e)) => {
                    done = true;
                    let e = e.clone();
                    drop(p);
                    ui.colored_label(RED, e);
                }
            }
        }

        // 書き出すボタン
        let Some(l) = &self.listing else { return };
        let target = self.target();
        let key = CheckKey { target: target.clone(), listing: l.id, runs: self.runs, done };
        if self.check.as_ref().is_none_or(|c| c.key != key) {
            let (result, free) = check_target(l, target.as_deref());
            self.check = Some(TargetCheck { key, result, free });
        }
        let Some(check) = &self.check else { return };
        let mut start = false;
        ui.horizontal(|ui| {
            let label = format!("書き出す（{} 個・{}）", l.summary.copy_count, bytes_text(l.summary.copy_bytes));
            start = ui.add_enabled(check.result.is_ok(), egui::Button::new(label)).clicked();
            if let Some(free) = check.free {
                ui.colored_label(DIM, format!("空き {}", bytes_text(free)));
            }
        });
        if let Err(e) = &check.result {
            ui.colored_label(YELLOW, e);
        }
        if !start {
            return;
        }
        // 押したときは確かめ直す（前に確かめた後で、書き出し先に何か置かれたかもしれない）
        let Some(t) = target else { return };
        if let (Err(e), _) = check_target(l, Some(&t)) {
            self.check = None;
            self.set_status(e, true);
            return;
        }
        let job = Job {
            project: l.project.clone(),
            text: l.text.clone(),
            scan: l.scan.clone(),
            plan: l.plan.clone(),
            fonts: l.classified.fonts.iter().cloned().collect(),
            unknown_effects: l.classified.unknown_effects.iter().cloned().collect(),
            target: t,
            name: self.name.trim().to_string(),
        };
        let ctx2 = ctx.clone();
        *copier_guard = Some(Copier::start(job, move || ctx2.request_repaint()));
        self.runs += 1;
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let project = PROJECT.read().clone();
        // プロジェクトが変わったら、名前を既定（<プロジェクト名>_収集）にする
        if project != self.name_for {
            if let Some(stem) = project.as_ref().and_then(|p| p.file_stem()) {
                self.name = format!("{}_収集", stem.to_string_lossy());
                self.name_text = self.name.clone();
            }
            self.name_for = project.clone();
        }
        self.refresh_seen(project.as_ref());
        egui::Panel::top("top").show(ui, |ui| self.render_top(ui, project.as_ref()));
        egui::Panel::bottom("bottom").show(ui, |ui| {
            self.render_bottom(ui, &ctx);
            if !self.status.is_empty() {
                let color = if self.status_error { RED } else { DIM };
                ui.colored_label(color, egui::RichText::new(&self.status).small());
            }
        });
        // 中央パネルは最後に追加する（ルール au2-rs-plugin「egui レイアウト」）
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                self.render_listing(ui);
                ui.add_space(8.0);
                self.render_excludes(ui);
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discard_frame(mut out: egui::FullOutput) {
        out.textures_delta.clear();
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    /// 1 フレーム描く。`focus` なら Tab で欄にフォーカスを移す（クリックと同じ。MidpointTable_H のテストと同じ）
    fn frame(ctx: &egui::Context, t: &mut String, focus: bool, events: Vec<egui::Event>) -> bool {
        let events = if focus { vec![key(egui::Key::Tab)] } else { events };
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))),
            events,
            ..Default::default()
        };
        let mut committed = false;
        discard_frame(ctx.run_ui(input, |ui| {
            committed = line_edit(ui, t, 200.0);
        }));
        committed
    }

    #[test]
    fn line_edit_reverts_on_escape_and_commits_on_enter() {
        let ctx = egui::Context::default();
        let mut t = String::from("作品_収集");
        frame(&ctx, &mut t, true, vec![]);
        frame(&ctx, &mut t, false, vec![]);
        frame(&ctx, &mut t, false, vec![egui::Event::Text("X".into())]);
        assert_eq!(t, "作品_収集X", "打っている途中の文字は欄に出る");
        assert!(!frame(&ctx, &mut t, false, vec![key(egui::Key::Escape)]));
        assert_eq!(t, "作品_収集", "Esc で入力前に戻る");
        for _ in 0..3 {
            frame(&ctx, &mut t, false, vec![]);
            assert_eq!(t, "作品_収集");
        }

        frame(&ctx, &mut t, true, vec![]);
        frame(&ctx, &mut t, false, vec![]);
        frame(&ctx, &mut t, false, vec![egui::Event::Text("Y".into())]);
        assert!(frame(&ctx, &mut t, false, vec![key(egui::Key::Enter)]), "Enter で確定");
        assert_eq!(t, "作品_収集Y");
    }

    #[test]
    fn bytes_are_readable() {
        assert_eq!(bytes_text(512), "512 B");
        assert_eq!(bytes_text(2048), "2 KB");
        assert_eq!(bytes_text(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(bytes_text(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }

    fn sample_plan(n: usize) -> Plan {
        let sources = (0..n)
            .map(|i| plan::Source {
                path: format!("D:\\Videos\\動画編集\\作品\\素材\\voice\\{i:05}_台詞.wav"),
                from: format!("D:\\Videos\\動画編集\\作品\\素材\\voice\\{i:05}_台詞.wav"),
                relinked: false,
                folder: false,
                guessed: false,
                sequence: false,
                files: 1,
                bytes: 48_000,
                state: if i % 50 == 7 {
                    State::Missing
                } else {
                    State::Copy { dest: PathBuf::from(format!("素材\\{i:05}_台詞.wav")), renamed: false }
                },
                entries: vec![i],
            })
            .collect();
        Plan { sources }
    }

    #[test]
    fn missing_sources_come_first_in_the_detail_list() {
        let p = sample_plan(120);
        let order = detail_order(&p);
        let first_copy = order.iter().position(|&i| p.sources[i].state != State::Missing).unwrap();
        assert_eq!(first_copy, 3, "見つからない 3 つ（7・57・107）が先頭");
        assert!(order[first_copy..].iter().all(|&i| p.sources[i].state != State::Missing));
        assert_eq!(Summary::of(&p).missing, 3);
    }

    /// 詳しい一覧を開いたときの 1 フレーム（素材 3300）。見えている行だけを描くので、数が増えても変わらない。
    /// 前の作り（すべての行を Grid で描く）と比べる。時間は機械で変わるので、既定では走らせない:
    ///   cargo test --release --lib frame_cost -- --ignored --nocapture
    #[test]
    #[ignore]
    fn frame_cost_of_detail_list() {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(720.0, 480.0))),
            ..Default::default()
        };
        for n in [300, 3300] {
            let p = sample_plan(n);
            let order = detail_order(&p);
            let measure = |all_rows: bool| {
                let frames = 20;
                let t = Instant::now();
                for _ in 0..frames {
                    discard_frame(ctx.run_ui(input(), |ui| {
                        if all_rows {
                            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                                egui::Grid::new("all").num_columns(4).show(ui, |ui| {
                                    for s in &p.sources {
                                        ui.label("コピー");
                                        ui.label("ファイル");
                                        ui.label(&s.path);
                                        if let State::Copy { dest, .. } = &s.state {
                                            ui.label(format!("→ {}（{}）", dest.display(), bytes_text(s.bytes)));
                                        }
                                        ui.end_row();
                                    }
                                });
                            });
                        } else {
                            detail_list(ui, &p, &order);
                        }
                    }));
                }
                t.elapsed().as_secs_f64() * 1000.0 / frames as f64
            };
            println!("[INFO] 素材 {n}: 見えている行だけ {:.2} ms / すべての行（前の作り） {:.2} ms", measure(false), measure(true));
        }
    }
}
