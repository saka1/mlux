# mlux Scroll Animation Design

## Overview

mlux のターミナルビューアにおけるスクロール補間 (current 位置を target に
向けて時間発展させる下流層) の設計判断を、なぜそうなっているかの根拠と
共に記録する。コードから直接読み取れない設計判断 (試したが捨てた選択肢、
参照した規範、トレードオフの構造) を残すのが目的。

オリジナル設計文書 (HCI 一次資料調査): `../2026-04-18-design-scroll-animation.md`
を参照。各 variant の試行ログは `../2026-04-*-experiment-*.md` 系。
本ドキュメントは「採択した方式」の側に寄る。

## Two-layer scroll architecture

スクロール体験は 2 層に分離されている。本ドキュメントは下流層のみを扱う。

| 層 | 入力 → 出力 | モジュール |
|---|---|---|
| **上流** (target accumulation) | キー入力 → `target_y` 更新 | `mode_normal.rs`, `scroll_policy.rs` |
| **下流** (interpolation) | `target_y` → 毎フレームの `current` | `scroll_animator.rs` |

接点は `target_y: u32` の単一スカラのみ。上流は離散イベント駆動、下流は
連続時間ステートマシンとして独立にテストできる。

## Pluggable animator: closed enum

`ScrollAnimator` は `dyn Trait` でなく **closed enum** で 3 variant を持つ。

```rust
pub(super) enum ScrollAnimator {
    ExpDecay { current, half_life_ms, ramp_elapsed_ms },
    ExpDecayAdaptive { current, base_half_life_ms },
    Kinetic { current, velocity, tau_ms },
}
```

**enum を選んだ理由**: variant 追加時にコンパイラが全 `match` arm の網羅性を
強制する。`dyn Trait` だと `add_impulse` の no-op など variant 固有の
no-op を黙って忘れるリスクがある。`scroll_policy.rs` の `ScrollStrategy` も
同じ規約。

CLI / config からの選択は `--scroll-animation={exp-decay,exp-decay-adaptive,
kinetic}` で行う。`from_config` が enum dispatch する。

## Animator API

各 variant が実装する 5 つの method:

| method | 役割 | ExpDecay | ExpDecayAdaptive | Kinetic |
|---|---|---|---|---|
| `current()` | 現在のサブピクセル位置を取得 | ✓ | ✓ | ✓ |
| `is_animating(target)` | 動きが残っているか | residual<0.5 | residual<0.5 | residual<0.5 AND \|v\|<30 |
| `set_target(restart_ramp)` | target が変化したことを通知 | ramp リセット (条件付き) | no-op | no-op |
| `add_impulse(delta_px)` | velocity に impulse 加算 (累積) | no-op | no-op | `v += δ/τ` |
| `set_landing(target_px)` | velocity を target 着地速度に置換 | no-op | no-op | `v = (T-cur)/τ` |
| `tick(target, vp, dt)` | 1 フレーム時間発展 | exp 減衰 + ramp | distance-adaptive 減衰 | velocity 摩擦積分 |

`add_impulse` と `set_landing` は両方とも Kinetic 専用だが、no-op として他
variant にも実装することで、上流の `viewport.rs` が animator variant を
意識せず単一のコードパスで動く。

### Effect → API の対応

```
Effect::ScrollTo(y)              gg/G/search/TOC (絶対ジャンプ)
  → set_target(restart_ramp)     ExpDecay の ramp 補正
  → set_landing(y)               Kinetic の velocity 上書き

Effect::ScrollBy { target, impulse_px }   j/k/Ctrl-D/Ctrl-U (incremental)
  → set_target(restart_ramp)               ExpDecay の ramp 補正
  → add_impulse(impulse_px)                Kinetic の momentum 累積
```

**`set_landing` を `add_impulse` から分離した理由**: 絶対ジャンプは過去の
momentum を破棄したい (iOS scroll-to-top 流)。incremental は momentum を
累積したい。両者を同じ method にすると Kinetic の意味論が決められない。

## Variants in detail

### ExpDecay (default)

**数学**: `current += (target - current) · α`、ただし
`α = 1 - 0.5^(dt/hl)` で `hl = 40ms` (半減期)。閉形式で frame-rate independent。

**ease-in ramp**: 設定が rest → motion に遷移するときだけ、effective half-life
を `120ms → 40ms` に **smoothstep で 100ms かけて減衰**させる。pursuit onset
latency (Schütz et al. 2011) 100ms の補償。`set_target(true)` で trigger、
継続スクロール中の `set_target(false)` ではリセットしない (eye が既に
追跡中のため不要)。

**snap**: `residual < 0.5px` のみ。

**特徴**: 「ふわっと立ち上がり、指数で settle」。読み物 UI の baseline。

### ExpDecayAdaptive

**数学**: `hl(d) = base × (1 + ln(1 + d/viewport))`、`base = 40ms`。
近距離 (d ≪ viewport) では base と同じ、遠距離は対数で stretch。Stevens の
冪則 (`T ∝ √d` 〜 `log d`) と整合 (§4.7 of design-scroll-animation.md)。

**ease-in ramp**: 持たない (距離適応で間接的に同等の効果)。

**snap**: `residual < 0.5px` のみ。

**特徴**: 単発 j〜PgDn は ExpDecay と同じ感触。`gg` / `G` の大ジャンプで
settle 時間が伸びるので、視線追従が間に合う。読み物用途で「短距離は速く、
長距離はゆっくり」という人間の期待に合う。

### Kinetic

**数学**: 一階摩擦 ODE の閉形式積分。

```
decay  = exp(-dt/τ)
x(t+dt) = x(t) + v(t) · τ · (1 - decay)
v(t+dt) = v(t) · decay
```

`τ = 50ms`、settle ~5τ = 250ms。Euler 不要 (closed-form は任意 dt で正確)。

**velocity 注入の 2 経路**:
- `add_impulse(δ)`: `v += δ/τ` (累積) — 連打で momentum が伸びる
- `set_landing(T)`: `v = (T-cur)/τ` (置換) — 過去 momentum を捨てて T に着地

不変条件 `target == current + velocity·τ` を上流が維持する設計。

**ease-in ramp**: 持たない (iOS 流: ユーザーが初速を与える前提)。
**キーボード入力との相性は HCI 文献では未開拓** (touch/mouse の文献値を
そのまま適用できない)。

**snap**: `residual < 0.5px` AND `|velocity| < 30 px/s` (両方必要)。
velocity 閾値が必要な理由: ピクセル整数化 (`y_offset = current.round()`) で、
低 velocity が長時間残ると「1 px / 数十 ms」の visible creep になるため、
強制 snap で打ち切る。30 px/s = 1 px/33ms (≈ 30fps) が境界。

**特徴**: 「キビキビ立ち上がり、摩擦で減衰」。瞬時最高速 → glide → snap。
連打の momentum 累積が物理的に直感的。

## HCI grounding

採択判断の根拠となる知覚研究の要点。詳細は
`../2026-04-18-design-scroll-animation.md` §4 を参照。

### 立ち上がり: pursuit onset latency

- smooth pursuit の開始潜時 100-200ms (Schütz, Braun, & Gegenfurtner 2011)
- 目標が動き始めても眼球が追従しはじめるまでに遅延がある
- 急激な動き出しは「目で追えない」感覚

→ ExpDecay の smoothstep ramp は直接これを補償。Kinetic の no-ramp は
キーボード入力を「ユーザーが事前に予測している離散インパルス」と解釈する
ことで省略 (経験則: 「キビキビが悪くない」体感観察)。

### 終端: predictability of endpoint

「滑らかさ」より「到着の予測可能性」が優先される、というのが HCI 文献の
共通了解。

- **非対称イージング**: Material `fast-out-slow-in`、Apple HIG `easeOut` は
  減速側に重みを置く。ただし「鋭く減速」というより「減速プロファイル
  + 固定時長で打ち切り」で creep を見せない設計。
- **ballistic + corrective モデル** (Fitts 系): 人間の到達運動の自然な構造。
  iOS kinetic はこれに素直に従う。
- **smooth pursuit 終了の生理**: target 停止後 ~100ms 慣性で目が動く。
  急停止 (1 次微分連続な急減速) は「到着の手応え」、緩い tail は「まだ
  動いてる?」の認知未完了感を生む。

→ critical damping spring (DampedSpring) は ω 一個で ease-in と settle を
結合する制約があり、tail が `(1+ωt)·e^(-ωt)` の多項式因子で構造的に遅い。
HCI の方向と逆になるため `2026-04-29-experiment-kinetic-scroll.md` で削除。

### Pixel ticking と snap 閾値

整数ピクセル丸めの下では、低 velocity 域 (`|v| < ~30 px/s`) は visible creep
として見える。snap 閾値で強制終了する必要がある:

| variant | snap 条件 | 理由 |
|---|---|---|
| ExpDecay / Adaptive | residual<0.5px | velocity 概念なし、residual だけで十分 |
| Kinetic | residual<0.5 AND \|v\|<30 px/s | velocity 持続するので両方必要、creep 排除 |

DampedSpring 時代の `5 px/s` 閾値は creep 域を許容してしまう設定で、
「微小ピクセルを刻む」体感の主因だった (`2026-04-29-experiment-kinetic-scroll.md`)。

### 距離適応: Stevens の冪則

知覚速度は物理速度と線形でなく冪則 (指数 0.6–0.8、Stevens 1957)。同じ
速度感を出すには遠距離で時間を伸ばす必要がある。サブ線形 (`T ∝ √d` or
`log d`) が実用バランス。

→ ExpDecayAdaptive の `(1 + ln(1 + d/viewport))` がこの実装。

## Parameter rationale

```rust
// 共通
const SNAP_THRESHOLD_PX: f64 = 0.5;          // pixel quantization の境界

// ExpDecay / Adaptive
const DEFAULT_HALF_LIFE_MS: f64 = 40.0;      // sub-cell 実験で導出 (注1)
const RAMP_DURATION_MS:    f64 = 100.0;      // pursuit onset latency
const RAMP_INITIAL_SCALE:  f64 = 3.0;        // 120ms → 40ms = ×3 → ×1

// Kinetic
const DEFAULT_KINETIC_TAU_MS: f64 = 50.0;    // settle ~250ms
const KINETIC_SNAP_VELOCITY:  f64 = 30.0;    // 1px/33ms (creep 境界)
```

**注 1**: half_life=40ms の根拠は `../2026-04-18-experiment-subcell-scroll.md`
§4。経験的な「ふわっと感じるが間延びしない」値。

## When to pick which

```
                         ┌────────────────────────────────┐
                         │ ExpDecay (default)             │
                         │ ・幅広いユースケース           │
                         │ ・ふわっと立ち上がる           │
                         │ ・短距離も長距離も同じ感触     │
                         └────────────────────────────────┘
                                       │
                            ┌──────────┴──────────┐
              「gg/G が忙しい」          「キビキビ momentum が欲しい」
                            ↓                       ↓
              ┌─────────────────────┐  ┌─────────────────────────┐
              │ ExpDecayAdaptive    │  │ Kinetic                 │
              │ ・大ジャンプを追える │  │ ・連打で慣性が伸びる    │
              │ ・短距離は ExpDecay  │  │ ・ease-in なし、瞬発     │
              │   と同じ            │  │ ・iOS-style              │
              └─────────────────────┘  └─────────────────────────┘
```

`--exp-preset=adaptive` は `scroll_mode=Adaptive` (上流の入力密度適応) +
`scroll_animation=Kinetic` のセット。「読み流し向け、momentum で滑らせる」
プリセット。

## 履歴と削除された選択肢

実装史と捨てた選択肢:

- **DampedSpring** (`2026-04-20-experiment-scroll-animation-tuning.md`):
  critical damping spring。tail が構造的に slow で「微小ピクセル creep」が
  解消できなかった (`2026-04-29-experiment-kinetic-scroll.md`)。`Kinetic`
  に置換し削除。

- **Bezier + 固定時間** (Chrome 流): 連打時に新入力が古いアニメをキャンセル
  → 速度不連続。連続 target 更新モデル (mlux の上流層) と相性が悪い。
  `2026-04-18-design-scroll-animation.md` §2.3 で却下。

- **Velocity clamping / position-dependent damping** など ad-hoc な工夫:
  パラメータ空間が広がりすぎてチューニング負債。閉形式優先で先送り
  (`2026-04-20-experiment-scroll-animation-tuning.md` §未解決)。

## 制約と未解決

- **Kinetic 大ジャンプの初速暴走**: gg/G で `v = (target-current)/τ` がそのまま
  注入されるため、長距離ジャンプで初速が極端 (例: 50000 px/s)。1 frame で
  数百 px 飛ぶ。ExpDecayAdaptive で代替できるので暫定保留。viewport 単位の
  velocity クランプか、絶対ジャンプ専用の designed easing curve に委譲する
  選択肢あり。
- **キーボードと kinetic の相性研究**: HCI 文献は touch/mouse 中心。
  キー押下の離散性と kinetic の連続性のミスマッチがどこで顕在化するかは
  empirical。
- **3 variant の冗長性**: ExpDecay / Adaptive / Kinetic は同じ一階指数族。
  「ramp あり/なし」「target chase / velocity」「distance adaptive あり/なし」の
  3 軸の組み合わせとして整理すれば、未試行の組み合わせ (e.g. Kinetic +
  distance adaptive、no-ramp ExpDecay) も探索可能。
