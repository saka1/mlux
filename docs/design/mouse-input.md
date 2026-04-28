# mlux Mouse Input Design

## Overview

mlux ターミナルビューアにおけるマウスホイール入力(`--mouse` opt-in、コミット系列
は `feature/mouse-input` ブランチ)の設計判断を、なぜそうなっているかの根拠と共に
記録する。コードから直接読み取れない設計判断(試したが捨てた選択肢、参照した規範、
トレードオフの構造)を残すのが目的。

着手前の分析は `../2026-04-28-design-mouse-input.md` を参照。本ドキュメントは
「採択した方式」の側に寄る。

## Goals

ズーム機能(`+` / `-` / `=`)着地後、less `--mouse` 同型のマウスホイール入力を opt-in で
導入する。期待挙動:

- **ホイール上下** → 縦スクロール
- **Ctrl + ホイール** → zoom in / out (preset 経由)

ホットキー操作と等価なことだけを目的とし、横スクロール・ドラッグ・クリックは初期
スコープ外とした。

## Architecture: 入力源で Action を分離

Event → Action → Effect の 3 層を踏襲しつつ、Action 層をキーボード/マウスで分離する。

| 層 | 役割 | キーボード | マウス |
|---|---|---|---|
| **Event** | crossterm から届く生入力 | `Event::Key('j')` | `Event::Mouse(ScrollDown)` |
| **Action** | ユーザ意図 | `Action::ScrollDown(n)` | `Action::WheelDown(n)` |
| **Effect** | 描画系への命令 | `Effect::ScrollTo(y)` | `Effect::ScrollTo(y)` ← **共通** |

zoom 側も同じ:

| 層 | キーボード | マウス |
|---|---|---|
| Event | `Event::Key('+')` | `Event::Mouse(ScrollUp + CONTROL)` |
| Action | `Action::ZoomIn` | `Action::WheelZoomIn(n)` |
| Effect | `Effect::Exit(SetScale)` | `Effect::AccumulateZoom(±n)` → coalesce → 同左 |

### Why split Action by input source

「`Action::ScrollDown(n)` を共有して n の意味を文脈で変える」のではなく、
**WheelDown / WheelUp / WheelZoom\* を別 Action にする**。理由は 3 つあり、いずれも
現コードに既に存在する制約が根拠:

#### 1. `ScrollStrategy::Adaptive` の連打計測を汚染しない

`ScrollStrategy::Adaptive` は j/k の連打間隔を `InputHistory` で測って step を伸縮する
設計(`docs/2026-04-12-design-scroll-acceleration.md`)。wheel イベントを混ぜると測定
が破壊される — wheel は連打というより連続スクロールで、タイミング分布が j/k と
別物。

Wheel を別 Action にして scroll_strategy を経由させないことで、Adaptive の数学を
触らずに済む。

#### 2. キーボード `scroll_step` と独立した `wheel_step`

キーボード `j` 1 押下と wheel 1 ノッチは、ユーザの感覚では別の量。`viewer.scroll_step`
(既定 3、cell_h 単位)と `viewer.wheel_step`(既定 2、cell_h 単位)を独立に持つことで、
それぞれを実機で個別チューニングできる。

#### 3. Ctrl+wheel zoom 専用の coalesce を局所化

Ctrl+wheel zoom は 1 ノッチ = 1 SetScale = 1 ドキュメント全体再ビルド(~1 秒)を
引き起こすため、coalesce 必須。**Action を分けておけば handler 側で局所的に集約でき、
キーボード `+` 側には不要なロジックを混ぜずに済む**。

`Action::ZoomIn` (キーボード)は即時 `Effect::Exit(SetScale)`、`Action::WheelZoomIn(n)`
(マウス)は `Effect::AccumulateZoom(+n)` を返す、という分離になる。

## Effect 層は共通

最終 `Effect::ScrollTo(y)` / `Effect::Exit(SetScale)` は既存パスを再利用する。Wheel
スクロールは既存の `viewport::apply` の `Effect::ScrollTo` arm にそのまま流れ込み、
zoom 再ビルドは既存の outer loop が `ExitReason::SetScale` を受けて 1 回回るだけ。

これにより wheel は「入口を増やす」差分のみで成立し、tile.rs / display_state /
prefetch などの内部機構には一切触らない。

## Coalesce: pending_zoom_delta の frame-budget flush

### Why coalesce is mandatory

Ctrl+wheel zoom は 1 ノッチごとに `Effect::Exit(SetScale)` を出すと、`SetScale` の
inner loop break → outer loop でドキュメント全体再ビルド(typst::compile + 全タイル
再レンダ)が走る。1 秒前後かかるため、wheel を回すと即詰まる。

Wheel zoom 側だけ集約することで、burst 入力(ホイールを 5 ノッチ連続で回す等)を
1 回の rebuild に折りたたむ。

### Considered alternatives

| 方式 | 概要 | 採否 |
|---|---|---|
| ① | `pending_zoom_delta: i32` を貯めて `frame_budget` 境界で flush | ⭕ 採択 |
| ② | 一定時間(例 80ms)入力が止まってから発火する debounce | ❌ |
| ③ | wheel 専用の thread + channel で集約 | ❌ over-engineered |

#### Why ② fails

debounce timer を別途持つと、既存の inner loop の `event::poll(timeout)` パターンと
タイマー管理が二重化する。frame_budget 境界(既存)で十分な集約効果が得られるので、
新たな状態を増やす理由がない。

#### Why ① works

`Viewport` に `pending_zoom_delta: i32` を置き、`Effect::AccumulateZoom(d)` の
`viewport::apply` arm で `pending_zoom_delta += d; dirty = true;` するだけ。
`dirty = true` により次の `event::poll` の timeout は `frame_budget` (既定 32ms)に
落ちる。32ms 以内に追加 wheel が来れば再加算、来なければ poll が timeout して flush。

flush 自体は inner loop の poll-timeout 直後・redraw 直前に置く:

```rust
if vp.pending_zoom_delta != 0 {
    let dir = vp.pending_zoom_delta.signum();
    let mut target = app.config.scale;
    for _ in 0..vp.pending_zoom_delta.unsigned_abs() {
        target = mode_normal::next_zoom_preset(target, dir);
    }
    vp.pending_zoom_delta = 0;
    /* zoom_effects(current, target) を apply ループに流す */
}
```

`next_zoom_preset` は `+`/`-` ホットキーと共有なので、wheel zoom 1 ノッチは `+` 1 押下
と完全に等価な preset 移動になる。これにより「キーボードズームと挙動を一致させたい」
という設計上の不変条件が単一関数に局所化される。

### Frame budget as natural boundary

flush タイミングを `frame_budget` 境界に揃えることで:

- 集約最大遅延が 32ms に bounded される(知覚的にラグなし)
- inner loop の既存タイマー(redraw 用)を流用、新タイマー不要
- burst 入力時は最大 32ms まで蓄積 → 1 ノッチでも 5 ノッチでも rebuild は 1 回

## Other-mode handling

ホイールスクロールは Normal モード**のみ**で有効化する。Search / Toc / UrlPicker /
Command / InlineSearch / Grep / Log の各モードは picker UI なので、ホイールでカーソル
移動させたい場合は別途 keymap (`map_*_key`) を拡張する設計余地が必要 — 初期実装で
シンプルに保つために Normal 限定とした。

実装上は `Event::Mouse` arm 内で `match &mut vp.mode { ViewerMode::Normal => ..., _ => vec![] }`
で他モードは効果ゼロにする。`MouseEventKind` と Normal-mode 専用の Action しか作って
いないので、後で他モードに拡張する場合も Action を増やすところから始められる。

## Opt-in by default

less `--mouse` と同じ opt-in モデル。`viewer.mouse = false` 既定、`--mouse` CLI フラグで
有効化。`EnableMouseCapture` を発行する箇所(`RawGuard::enter(mouse: bool)`)を `Drop`
実装と対称にすることで、`DisableMouseCapture` を panic 含む全終了パスで保証する。

### Native text selection trade-off

`EnableMouseCapture` が有効化されている間、端末ネイティブのテキスト選択は無効化
される。ただし mlux の本文は Kitty Graphics Protocol で PNG 画像として描画されている
ため、`--mouse` 有無にかかわらずネイティブ選択でテキストをコピーすることは元々
できない。影響範囲はステータスバー(ファイル名・スクロール位置の文字列)のみで、
コンテンツのコピーは mlux 固有の `y` / `Y` + OSC 52 で代替できる。

このため CLI ヘルプにも README にも警告は載せていない。

## Tunable parameters

| パラメータ | 既定 | 単位 | 役割 |
|---|---|---|---|
| `viewer.mouse` | `false` | bool | mouse capture 自体の opt-in スイッチ |
| `viewer.wheel_step` | `2` | cell_h 倍 | 1 ノッチで進む高さ |
| `viewer.frame_budget` | `32ms` | Duration | zoom coalesce の最大遅延 |

`wheel_step = 2` は `scroll_step = 3` (キーボード j)よりやや小さい。実機チューニング
の結果、3 だと burst で行き過ぎる、1 だと一行刻みすぎる、ということで 2 に落ちた。

## Known constraints

### Ctrl modifier reachability via tmux

Kitty / WezTerm / iTerm2 / Alacritty / foot は概ね Ctrl+wheel をそのまま中継するが、
tmux 経由だと欠けることがある(tmux の `xterm-keys` / `mouse` 設定依存)。本機能は
opt-in なのでユーザ責任で tmux 設定を整える前提とする。

### Wheel scroll never feeds Adaptive

`ScrollStrategy::Adaptive` は wheel から完全に切り離されているため、wheel を連打しても
keyboard scroll が加速することはない。`--mouse --scroll-mode=adaptive` を併用しても
両者は独立して動く。これは「2 と 3 の Why」で意図した設計通り。

### Future: WheelStrategy

将来、wheel 特有のペーシング・スムージング・アニメーション(慣性スクロール等)を
統合最適化する余地として、`scroll.rs::ScrollStrategy` の隣に `WheelStrategy` を置く
拡張点が空けてある。現状は `wheel_step * cell_h` 固定で十分。

## References

- 着手前分析: `../2026-04-28-design-mouse-input.md`
- Adaptive scroll(Action 共有を避けた根拠): `../2026-04-12-design-scroll-acceleration.md`
- ズーム preset と `next_zoom_preset` の設計: `./zoom.md`
