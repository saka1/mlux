# 設計: マウス入力ハンドリング

**対象**: viewer (`src/viewer/`) のマウスホイール対応
**実施日**: 2026-04-28
**ステータス**: 設計合意 / 実装未着手

## 背景

ズーム機能 (`+`/`-`/`=`) の追加(コミット `86733d3`)を機に、マウスホイール直接ハンドリングの導入を検討する。期待される操作:

- **ホイール**: 上下スクロール
- **Ctrl + ホイール**: zoom in / out

less (v566 以降) が `--mouse` で同様の機能を opt-in 提供しているのが直接の参照モデル。

## 現状コードのアセスメント

- `src/viewer/mod.rs:243` と `:364` の 2 箇所で `event::read()` を呼び、`Event::Key` と `Event::Resize` だけマッチ。それ以外は `_ => {}` で破棄 → **現在マウスイベントは届いていない**。
- `src/viewer/terminal.rs:28` には `enable_raw_mode()` のみ。`EnableMouseCapture` は未呼び出し。
- crossterm `0.28.1` が `MouseEvent { kind: ScrollUp/ScrollDown, modifiers, .. }` と `EnableMouseCapture` / `DisableMouseCapture` をフルサポート。
- ズーム配線は `Action::ZoomIn / ZoomOut` → `mode_normal::handle` → `Effect::Exit(ExitReason::SetScale)` → 外側ループ再ビルド、が既に通っている(`mode_normal.rs:113-115`)。
- スクロール配線は `Action::ScrollDown(n)` → `mode_normal::handle` → `Effect::ScrollTo(y)` で、`n` を pixel に変換するのは `ScrollStrategy::step()`(`scroll.rs`)。

つまり Effect 層と zoom 再ビルド機構はそのまま流用できる。

## 設計: Event / Action / Effect の 3 層

| 層 | 役割 | キーボード | マウス |
|---|---|---|---|
| **Event** | crossterm から届く生入力 | `Event::Key(j)` | `Event::Mouse(ScrollDown)` |
| **Action** | ユーザ意図 | `Action::ScrollDown(n_steps)` | `Action::WheelDown(n_notches)` |
| **Effect** | 描画系への命令 | `Effect::ScrollTo(y_pixels)` | `Effect::ScrollTo(y_pixels)` ← **共通** |

zoom 側も同様:

| 層 | キーボード | マウス |
|---|---|---|
| Event | `Event::Key('+')` | `Event::Mouse(ScrollUp + CONTROL)` |
| Action | `Action::ZoomIn` | `Action::WheelZoomIn(n_notches)` |
| Effect | `Effect::Exit(ExitReason::SetScale {..})` | 同左(coalesce 後) |

### Action を入力源で分ける根拠

「`Action::ScrollDown(n)` を共有して n の意味を文脈で変える」ではなく、**WheelDown/WheelUp/WheelZoom を別 Action にする**。理由は現コードに既に存在する:

1. **Adaptive 汚染回避**: `ScrollStrategy::Adaptive`(`scroll.rs`)は j/k の連打間隔を計測して step を伸縮する設計。wheel イベントを混ぜると測定が破壊される(wheel は連打というより連続スクロール)。
2. **設定の独立**: `viewer.scroll_step`(キーボード)と `viewer.wheel_step`(マウス)を別個に持てる。
3. **debounce/coalesce の局所化**: Ctrl+wheel zoom は `SetScale` → ドキュメント全体再ビルドを起こすため、coalesce が事実上必須。Action を分けておけば handler 側で局所的に集約できる。キーボード `+` 側には不要なロジックを混ぜずに済む。

## 採用する方針(合意済み)

- **opt-in**: 既定 off。`viewer.mouse = true` (config) または `--mouse` (CLI) で有効化。less の `--mouse` が同型の前例。
- **Action 層を入力源で分離**: `WheelDown` / `WheelUp` / `WheelZoomIn` / `WheelZoomOut` を新設し、既存の `ScrollDown` / `ZoomIn` 経路に流さない。
- **Effect 層は共通**: 最終的な `Effect::ScrollTo` / `Effect::Exit(SetScale)` は再利用。
- **Ctrl+wheel zoom は coalesce 必須**: 1 ホイールイベント = 1 再ビルドにすると即座に詰まる。

## 未決定事項(着手時に決める / 別調査が必要)

- **wheel スクロール量の決定アルゴリズム**: 当面は `wheel_step * cell_h` の固定値で十分。将来 `WheelStrategy`(`ScrollStrategy` の隣)として、ホイール特有のペーシング・スムージング・アニメーションを統合最適化する余地がある。
- **zoom coalesce の具体策**: 候補は ① `pending_zoom_delta` を貯めて `frame_budget` 境界で 1 回 `SetScale`、② 一定時間(例 80ms)入力が止まってから発火。①を初期実装に推す。
- **テキスト選択の扱い**: `EnableMouseCapture` を有効化すると端末ネイティブのテキスト選択・URL クリック・スクロールバーが効かなくなる。多くの端末で Shift 押下回避はできるが挙動は端末依存。`--mouse` を opt-in にした時点でこの点はユーザ責任とするが、status bar 等での明示は要検討。
- **Ctrl 修飾子の到達性**: Kitty / WezTerm / iTerm2 / Alacritty / foot は概ね OK だが、tmux 経由だと欠けることがある。実機確認が必要。

## 実装スコープ(参考)

合意分の実装で触る箇所:

1. `terminal.rs`: init/cleanup で `EnableMouseCapture` / `DisableMouseCapture` を `execute!` に追加(`viewer.mouse` が真のときのみ)。
2. `mod.rs` の `match ev`: `Event::Mouse(me) => …` 追加。
3. `keymap.rs`: `map_mouse_event(MouseEvent) -> Option<Action>` 新設。`WheelDown/Up/ZoomIn/ZoomOut` を発出。純関数なのでユニットテストは既存 `keymap.rs` 内で完結。
4. `mode_normal.rs`: `Action::Wheel*` ハンドラ追加。`WheelZoom*` 側は coalesce バッファ経由で `Effect::Exit(SetScale)` を発出。
5. `config.rs`: `viewer.mouse: bool`(既定 `false`)、`viewer.wheel_step: u32`(既定値は実機チューニング)を追加。
