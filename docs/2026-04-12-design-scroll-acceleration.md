# Scroll Acceleration Design

ターミナルビューアのスクロール加速アルゴリズム。
次セッションでの実機チューニングを想定したメモ。

> **Status: experimental, opt-in.** `--scroll=adaptive` で有効化する。
> デフォルトは `--scroll=fixed`(クラシックな定量スクロール、`scroll_step=3`)。
> 以下のパラメータとアルゴリズムは adaptive モードでのみ適用される。

## Background

初期実装(コミット `e3a3aea`)は以下の設計だった:

- `InputHistory` が直近 5 秒のスクロールイベントをリングバッファに保持
- `ScrollPolicy::effective_step` が直近 5 秒の同方向イベント数を数え、階段状の係数 (x1/x2/x3/x4) を返す

コードレビュー(`claude/user-input-history-tracking-PyKcU` ブランチ)で以下の問題が指摘された:

1. **観測窓が粗すぎる** — 5 秒前のイベントは現在の入力密度を反映しない。実用的な窓は 500〜800ms。
2. **係数が過大** — x3 などでは、OS キーリピート (~30ms 間隔) と組み合わさって移動量が暴走する。

## Design

### コンセプト

- 入力密度から「ユーザーはいま速く進みたい」という**意図を推定**する
- 推定に応じて**合計移動量を押し上げる**(ただし 1 イベントあたりの step は暴走させない)
- 指を離した瞬間に即座に減速する(Mario-jump HCI 研究 — 目標位置で止まれること)

### 3状態ステートマシン(履歴から都度算出、永続状態なし)

```text
dt_last      = 最新同方向イベントからの経過時間
window_500   = 直近 500ms の同方向イベント数
window_800   = 直近 800ms の同方向イベント数

// 1. 急減衰ゲート — 最優先
if dt_last > 150ms:
    return Normal

// 2. 状態判定
if window_500 >= 6 AND window_800 >= 10:   // 持続条件
    return High
if window_500 >= 3:
    return Mid
return Normal
```

非対称条件(入るには 800ms の持続、抜けるには 150ms の沈黙)で**ヒステリシス**を実現。`window_800 ⊇ window_500` なので、単発の偶然的な高頻度バーストでは High に入らない。

### 係数

| State  | multiplier | 挙動                           |
| ------ | ---------- | ------------------------------ |
| Normal | 1.0        | 単発タップ、稀な入力            |
| Mid    | 1.3        | 短く連打、3発以上 / 500ms      |
| High   | 1.6        | 長めの連打、10発以上 / 800ms    |

係数はあえて控えめ。高頻度による移動量増は「イベント数」側で既に稼いでいる前提で、multiplier はバイアス分のみ上乗せする。

## Current Parameters

チューニング値は `src/viewer/scroll_policy.rs` の `--- tuning zone ---` コメントで囲まれた const に集約されている:

```rust
const DECAY_GATE: Duration = Duration::from_millis(200);
const FAST_WINDOW: Duration = Duration::from_millis(500);
const SUSTAIN_WINDOW: Duration = Duration::from_millis(800);
const MID_THRESHOLD: usize = 3;
const HIGH_FAST_THRESHOLD: usize = 6;
const HIGH_SUSTAIN_THRESHOLD: usize = 10;
// 係数は「ユーザーの scroll_step × cell_h」に直接乗る。
// 内部的な 2-cell 基準 ÷ fixed mode 3-cell = 2/3 比を畳み込み済み。
const COEFFICIENT_NORMAL: f32 = 2.0 / 3.0; // ≈ 0.667
const COEFFICIENT_MID:    f32 = 1.0;
const COEFFICIENT_HIGH:   f32 = 4.0 / 3.0; // ≈ 1.333
```

base step は `Config::viewer::scroll_step * cell_h` (`mod.rs:397-398`)。`scroll_step` デフォルトは **3 cells**(fixed/adaptive とも同じ値)。cell_h=28 の環境では:

| mode/state | 計算 | pixel |
|---|---|--:|
| Fixed | 3 × 28 | 84 |
| Adaptive Normal | 3 × 28 × 2/3 | 56 |
| Adaptive Mid | 3 × 28 × 1 | 84 |
| Adaptive High | 3 × 28 × 4/3 | 112 |

履歴バッファは `src/viewer/scroll.rs` の `ADAPTIVE_HISTORY_WINDOW` (5s) / `ADAPTIVE_HISTORY_CAP` (128) として定義。Adaptive variant 構築時のみ確保される(Fixed は履歴を持たない)。外側の 5 秒窓は `SUSTAIN_WINDOW` (800ms) より十分広ければよい。

## Files

- `src/viewer/input_history.rs` — 履歴バッファ。`count_in_window(dir, window)` と `last_gap(dir)` を提供
- `src/viewer/scroll_policy.rs` — 3 状態分類器 `classify()` とステート→係数マッピング。`effective_step` で debug ログを 1 行出す
- `src/viewer/scroll.rs` — `ScrollStrategy` enum(`Fixed` / `Adaptive`)。モード選択に応じて policy/history を所有する
- `src/viewer/mod.rs` — `ScrollStrategy::step(base, dir)` を呼び出す箇所(line ~397)

## Tuning Guide

実機で触って調整する観点:

### 挙動チェックリスト

- **単発タップ**: 自然な 1 ステップ移動(Normal, 1.0x)
- **短い連打 (3〜5発)**: 明確に加速を感じる(Mid, 1.5x)。連打している実感が移動に反映されているか
- **長い連打 (10発以上)**: 明確に速く進む(High, 2.0x)。速すぎないか、遅すぎないか
- **指を離す**: 即座に Normal へ戻る(次のタップが 1.0x)。オーバーランしないか
- **方向切り替え**: 反対方向の履歴は影響しない(下→上の切り替えが鋭いか)

### 調整のヒント

- **加速が物足りない場合**: `MULTIPLIER_MID` / `MULTIPLIER_HIGH` を 0.2〜0.3 上げる。しきい値を下げる(例: `MID_THRESHOLD = 2`)のも手
- **速すぎて止まれない場合**: multiplier を下げる、または `DECAY_GATE` を短くする(150ms 程度)
- **連打の入りが遅い場合**: `FAST_WINDOW` を 700ms 程度に伸ばすか `MID_THRESHOLD` を下げる
- **High に入りにくい場合**: `HIGH_SUSTAIN_THRESHOLD` を 8 に下げる
- **手動連打で Mid が抜けやすい場合**: `DECAY_GATE` を広げる。200ms でも狭ければ 250ms まで。OS キーリピート間隔 (30〜50ms) より十分長く保つ

### 測定方法

ログを有効化 (`cargo run -- --log /tmp/mlux.log <file>`) して **scroll_policy は既に effective_step で毎イベント debug ログを出す**:

```
scroll_policy: dir=Down state=Mid mult=1.50 base=56 eff=84 fast=4 sustain=5 last_gap_ms=132
```

`last_gap_ms` は直前の同方向イベントとの間隔(手動連打の実測テンポ)。Python ワンライナーで state 分布・gap 分布・run 長・遷移を出せる。過去セッションでの解析例は `## Session Log` 参照。

## Session Log

### 2026-04-12: 初回実装〜パラメータ調整セッション

このセッションで判明した知見を時系列でメモ:

#### ⚠️ DECAY_GATE 測定バグ(修正済み)

初期実装では `classify()` 内で `history.time_since_last(dir)` を使って急減衰ゲートを判定していた。しかし `mod.rs:394` で **`record()` が `effective_step()` の直前に呼ばれている**ため、`time_since_last` は常に ≈0 を返し、ゲートは一度も発動しなかった。

- **症状**: ログ上 `dt_last_ms=0` が全イベント。設計書の挙動と実態が乖離
- **ただし体感は正常だった**: `fast < 3` なので長アイドル後の初押しは自動的に Normal になり、結果的に設計意図通りに動いていた。偶然の一致
- **修正**: `InputHistory::last_gap(dir)` を追加(最新 2 件の同方向イベント間の間隔)。`classify` はこれを使用。意味論が明確になり、ログの `last_gap_ms` が実測データとして機能するように

#### キーリピート依存性(構造的性質、残課題)

`count_in_window` は頻度ではなく **件数で閾値判定**するため、リピート頻度に依存する:

| しきい値 | 必要頻度 | 到達手段 |
|---|---|---|
| Mid (fast≥3 / 500ms) | 6 Hz (167ms 間隔) | 手動連打でも可 |
| High (fast≥6 & sustain≥10) | 12.5 Hz (80ms 間隔) | ほぼキーリピート専用 |

OS デフォルトリピート(Linux 30Hz / macOS 66Hz / Windows 30Hz)は全て閾値通過するので実用上は動く。ただしカスタマイズで遅く設定したユーザーは High に到達不可。gap-based (「直近 N ms に空白がない」) 判定に切り替えれば rate 非依存にできるが未実装。

#### 実測データ(cell_h=28, キーリピート ~45Hz 環境)

3 回のログ解析で観察された挙動:

- **キーリピート実間隔**: High 時の gap median = 22-31ms (45-55Hz)
- **手動連打の gap**: Mid 時の median = 120-135ms (7-8Hz)。物理的限界に近い
- **状態遷移**: Normal→High の直接ジャンプは **0 回**、必ず Mid を経由(設計通り)
- **High→Normal 遷移**: DECAY_GATE 発動で fast=10, sustain=22 の状態でも即 Normal に落ちるケースを観測。Mario-jump 即停止が理屈通り機能している

#### パラメータ変更履歴

| パラメータ | 初期値 | このセッション後の値 | 根拠 |
|---|---|---|---|
| `scroll_step` (config) | 3 cells | 2 cells (※後日 3 に復帰) | 単発移動量が大きすぎた。base を抑えて multiplier の効きしろを作る |
| `MULTIPLIER_MID` | 1.3 | **1.5** | 1.3x だと Mid の加速感が薄く「Normal と変わらない」体感。+30% → +50% に引き上げ |
| `MULTIPLIER_HIGH` | 1.6 | **2.0** | Mid を上げたので High との段差を保つため |
| `DECAY_GATE` | 150ms | **200ms** | 手動連打時の Mid gap が 126-149ms に張り付き、境界ギリギリで維持されていた。50ms マージンを確保 |

> ※ `scroll_step=2` は 2026-04-13 の係数化リファクタで `scroll_step=3` に戻り、2/3 比は `COEFFICIENT_*` 内部定数に吸収された(後述)。ピクセル出力は変わっていない。

#### 先送りしたチューニング方向

- **MID_THRESHOLD を 3 → 2** に下げる案: Mid の入りを早める。未検証
- **rate-based 判定への移行**: 構造的なリピート依存を解消。実装コスト大
- **`DECAY_GATE` をさらに 250ms へ**: 200ms でも Mid が抜けやすい場合の手

### 2026-04-13: opt-in 化と戦略ディスパッチの導入

チューニングは促進的に進んだが、キーリピート依存性の構造的問題が未解決で、万人向けのデフォルトに昇格させるのは時期尚早と判断。**experimental として `--scroll=adaptive` フラグ下に隔離**。

#### 命名: `fixed` vs `normal`

`--scroll` のデフォルト側を当初 `normal` と想定したが、adaptive 内部ステートに既に `ScrollState::Normal` があり名前衝突で混乱を招く(「Normal mode の Normal state」)。**`fixed`** に変更:

- 物理的性質(定量スクロール)を記述する命名
- `adaptive`(変動)と意味的に対になる
- 内部ステート名との衝突なし

#### アーキテクチャ: enum-based strategy dispatch

実装方式の候補を検討:

| 方式 | 採否 | 理由 |
|---|---|---|
| 呼び出し側で分岐 | ✗ | state(`InputHistory`/`ScrollPolicy`)が Fixed モードでも構築されてしまう |
| `Box<dyn ScrollStrategy>` | ✗ | クローズドな 2 variant に動的ディスパッチはオーバーキル。ヒープ確保・vtable は無駄 |
| **enum + メソッド** | ✅ | Rust idiom。静的ディスパッチ、variant ごとに state を所有できる |
| ジェネリクス `<S>` | ✗ | 戦略は CLI = 実行時選択なので型パラメータは不適 |

結果の配置:

- **`config::ScrollMode`** (pub enum `Fixed`/`Adaptive`) — ユーザーの選択を表す値型。config.rs は「fixed/adaptive のどちらかである」ことを担保するだけで、実装を知らない
- **`viewer::scroll::ScrollStrategy`** (enum `Fixed` / `Adaptive { history, policy }`) — ランタイム側の戦略ディスパッチ。`Fixed` は無状態、`Adaptive` のみ `InputHistory` + `ScrollPolicy` を保持
- **`main::ScrollModeArg`** (clap `ValueEnum`) — CLI 層のミラー。`From<ScrollModeArg> for config::ScrollMode` で変換。config.rs に clap 依存を侵入させない

依存方向は `main → config` と `viewer → config` で一貫。config.rs は domain 値のみ、clap 由来の derive 属性は含まない。

#### 係数化リファクタ — `scroll_step` の coerce を廃止

当初の実装では `apply_cli` で `--scroll=adaptive` 選択時に `scroll_step = 2` を強制していた。コードレビューで **「surprising action-at-a-distance」**(ユーザー設定値がフラグ選択で黙って書き換わる)として flag され、**2 つの概念が同じフィールドに同居している**ことが根本原因と判明:

- `scroll_step = 3` はユーザーの基準ステップ量(単発移動の感覚値)
- `scroll_step = 2` は adaptive アルゴリズム内部のチューニング基準

両者を分離。**「2」は `ScrollPolicy` 内部定数に移管**し、各ステートの係数として畳み込んだ:

```rust
// 内部 2-cell 基準 ÷ fixed 3-cell = 2/3 を、状態別 multiplier (1.0/1.5/2.0) と
// まとめて「ユーザー base への最終係数」として表現。
const COEFFICIENT_NORMAL: f32 = 2.0 / 3.0; // 旧: MULTIPLIER_NORMAL=1.0 を適用、base=2cell
const COEFFICIENT_MID:    f32 = 1.0;       // 旧: MULTIPLIER_MID=1.5
const COEFFICIENT_HIGH:   f32 = 4.0 / 3.0; // 旧: MULTIPLIER_HIGH=2.0
```

得られた性質:

1. **config の単一責任**: `scroll_step` は常にユーザーの基準値。`apply_cli` の coerce 分岐は削除
2. **ピクセル出力は不変**: cell_h=28 の実機で Normal=56, Mid=84, High=112 (旧実装と一致)
3. **比例スケール**: ユーザーが将来 `scroll_step=5` を指定すれば Fixed=5 cells、Adaptive は Normal=3.3 / Mid=5 / High=6.7 cells と自動的に比例
4. **意味の明瞭化**: Normal (2/3) は fixed より小さい → 単発タップがより精密、Mid (1.0) は fixed に一致、High (4/3) は fixed を超える。設計意図(単発=精密、連打=高速)が数字に直接表れる

命名も `MULTIPLIER_*` → `COEFFICIENT_*`、method `multiplier()` → `coefficient()` に改名。debug ログは `mult=1.50` → `coeff=0.67` のキーで出力。

## 2026-04-13 再設計: 時間ゲート廃止、純粋な窓密度ベースへ

チューニング後のログ解析で、`DECAY_GATE` (単一時間しきい値) が原因の **Mid ↔ Normal 振動**が判明。300-400ms cadence で手打ちタップしていると、`last_gap` が 300ms 境界を跨ぐ度に状態が反転していた:

```
time:  36.215  36.522  36.897  37.189
gap:    255ms   307ms   375ms   291ms
state:  Mid     Normal  Normal  Mid     ← 7ms 超過/9ms 不足で反転
```

### 問題の本質

単一 `last_gap` しきい値では以下 2 要件が物理的に両立しない:

1. **精度要件**: バースト直後 ~300ms の単発タップは Normal に落としたい
2. **安定要件**: ~300ms cadence の定常タップは一貫した状態に保ちたい

両者は同じ信号 (`gap ≈ 300ms`) で判定しており、どこにしきい値を置いても境界付近で必ず振動する。`DECAY_GATE` を伸ばすと振動域が別の cadence に移るだけで、かつバースト後の精度復帰が鈍くなる (実機で確認: "細かく移動したい時に暴発する感触" のフィードバック)。

### 解決: 時間ゲートを廃止し、窓の密度だけで分類

`DECAY_GATE` 定数を削除。`classify()` から `last_gap` による分岐を撤去。Normal/Mid の判定は **`DENSITY_WINDOW` 内のイベント数**だけで決める:

```rust
let density = history.count_in_window(dir, DENSITY_WINDOW);
if density < MID_THRESHOLD { return Normal; }
// ... High 判定 ...
Mid
```

### なぜ振動が消えるか

**時間境界が存在しないため** (連続量)。窓を跨いでイベントが落ちるのは自然な現象で、「ちょうど境界」が生まれない。`gap` 値ではなく「イベントが窓に居るか」で判定するので、状態の移り変わりが滑らかになる。

350ms cadence での例 (以前: 振動、現在: 安定):

```
time(ms):     0   350  700  1050  1400
event:        ●    ●    ●    ●    ●
density(300): 1    1    1    1    1    ← 常に1（前イベントが窓外）
state:        N    N    N    N    N    ← Normal で安定
```

### DENSITY_WINDOW の意味は二重化される

この窓幅は同時に 2 つの意味を持つ:

1. **Mid 判定窓**: この幅に 2 発以上 → Mid
2. **精度復帰時間**: 最後のタップからこの幅以上静止 → Normal

→ **チューニングポイントが 1 つに減る**。旧版では `DECAY_GATE` と `FAST_WINDOW` を別々に調整する必要があったが、今は `DENSITY_WINDOW` 1 つだけ。

実機チューニング: 400ms → 300ms に縮小。「Mid で段落スクロール、Normal で精密移動」を使い分ける際、Mid 直後の 1 発が Mid 判定されてしまう感覚があったため。

### High の扱い

High の判定条件 (`fast>=6 AND sustain>=10`) は不変。これはもともと手打ち不可能な密度が必要でキーリピート専用として機能しており、振動問題とは別系統。`FAST_WINDOW` は `HIGH_WINDOW` に改名して役割を明示。

### ステートマシン図 (完成形)

```
                     ┌─── 300ms 以上 無入力 ────┐
                     │   (density < 2 になる)   │
                     │                          │
                     ▼                          │
         ┌─────────────────────┐                │
         │      NORMAL         │                │
         │  ×1.0 (精密モード)  │                │
         └──────────┬──────────┘                │
                    │                           │
          2 発目のタップが                      │
          300ms 以内に来た                      │
          (density ≥ 2)                         │
                    ▼                           │
         ┌─────────────────────┐                │
         │        MID          │────────────────┘
         │ ×1.6 (段落スクロール)│  300ms 静止で戻る
         └──────────┬──────────┘
                    │                           ▲
         キーを押しっぱなし                     │
         (fast≥6 AND sustain≥10)                │
                    ▼                           │
         ┌─────────────────────┐                │
         │        HIGH         │────────────────┘
         │  ×1.8 (キーリピート)│  リピート停止で Mid 経由 Normal
         └─────────────────────┘
```

**重要な非対称性**:
- Mid への入口は 1 つ (短時間に 2 発) — 軽い
- High への入口は 2 重条件 (手打ち不可、キーリピート専用)
- どの状態からも Normal に戻る条件は 1 つ: `density < 2` のみ

### 決定木 (実装の正確なモデル)

分類器はステートレス。毎イベント、3 つの窓のイベント数を数えて判定:

```
イベント到来
  │
  │  density = 過去 300ms のイベント数 (DENSITY_WINDOW)
  │  fast    = 過去 600ms のイベント数 (HIGH_WINDOW)
  │  sustain = 過去 800ms のイベント数 (SUSTAIN_WINDOW)
  ▼
density < 2 ?  ──yes──► NORMAL (×1.0)
  │no
  ▼
fast ≥ 6 AND sustain ≥ 10 ? ──yes──► HIGH (×1.8)
  │no
  ▼
MID (×1.6)
```

### チートシート

| 知りたいこと | 答え |
|---|---|
| Mid に入る条件 | 直近 300ms に 2 発以上 |
| Normal に戻る条件 | 直近 300ms に 1 発以下 (= 300ms 無入力) |
| High に入る条件 | 600ms に 6 発 AND 800ms に 10 発 (手打ち不可) |
| 係数 | Normal ×1.0 / Mid ×1.6 / High ×1.8 |
| 実効ステップ (cell_h=28) | 56 / 90 / 101 px |

## Known Open Issues (not in scope for tuning)

コードレビューで指摘されたが、今回のアルゴリズム刷新の対象外で先送りしたもの:

1. **外側 5 秒窓の config 化** — 現在は `mod.rs` でハードコード。`Config` システム経由で設定できるようにする案
2. **half-page の入力記録扱い** — `Ctrl+D`/`Ctrl+U` も `input_history` に記録されており、後続の `j`/`k` の状態判定に影響する。意図的かどうか未整理
3. **debug ログの扱い** — 現状 `effective_step` が毎イベント debug ログを出す。`--log` オプション有効時のみ出力されるので本番影響はないが、チューニング完了後は trace レベルに落とすか削除検討

(2026-04-13 解消済: `ScrollPolicy` の `_private: ()` はユニット構造体化で削除、`scroll_step` 混線は Fixed/Adaptive 分離で解消、Mid ↔ Normal 振動は時間ゲート廃止で解消)

## Verification

```bash
cargo fmt
cargo clippy --all-targets     # 0 warning
cargo test                     # 450 unit + 62 integration
cargo run -- <長めの markdown>   # 実機で j/k 単発/連打/急停止
```
