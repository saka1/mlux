# スクロールアニメーション試行ログ: ExpDecay → ease-in ramp → DampedSpring

本メモは、`docs/2026-04-18-design-scroll-animation.md`（設計文書）および `docs/2026-04-18-experiment-subcell-scroll.md`（sub-cell 実験）の続編として、
2026-04-19〜20 の実装試行で判明した挙動・限界・未解決論点を記録する。

## 前提

- `ScrollAnimator` は `enum` ベースの閉じた variant 集合。元は `ExpDecay` のみ、その後 `ExpDecayAdaptive` を追加済み。
- Upstream 層（`scroll_policy.rs`）は Normal/Mid/High の 3 段加速を持ち、キー連打で 1 キーあたりの target 増分が 48→77→86px と拡大。
- Downstream 補間層は `current → target` の時間発展のみ扱う。

## 試行 1: ExpDecay に ease-in ramp を後付け

### 動機
設計文書 §4.3 の "pursuit onset latency 100ms"（Schütz 2011, Gegenfurtner 2016）を ExpDecay に取り込むため、半減期を smoothstep で `3× → 1×` に補間する ramp を実装。

### 実装
`ExpDecay { ..., ramp_elapsed_ms: f64 }` を追加し、`set_target(restart_ramp: bool)` の `true` ケースで `ramp_elapsed_ms = 0.0` にリセット。`tick` 内で effective half-life を `120ms → 40ms` に滑らかに変化させる。

### 体感
**「ふわっと動き出す」のは好感触**。ただし次の問題が発覚。

### 問題: 階段状の apparent-speed 急変
`j` 長押し時、key-repeat で target が毎 ~30ms ごとに 48→125→211→297px... と積み上がる。ease-in（effective hl = 120ms）で current が遅い間に target が遥か先へ行き、ramp 完了時に「一気に追いかける」挙動になる。

Python シミュレーション（`dt=16ms, 60fps`）:

```
t=  0ms  tgt=  48  cur=   4.2  +4.2px/f  rampあり
t= 32ms  tgt= 125  cur=  20.5  +12.2px/f rampあり
t= 64ms  tgt= 211  cur=  62.0  +28.4px/f rampあり
t= 96ms  tgt= 297  cur= 141.7  +49.1px/f 全速  ← 急変
```

体感は「最初ほぼ止まって見え、突然動き出す」。原因は ease-in そのものではなく、**target が離散的に跳ぶ×位置ベース補間の組み合わせ**。ExpDecay は毎フレーム残差から velocity を導出するので、target の段差がそのまま apparent-speed の段差として現れる。

## 試行 2: DampedSpring（臨界減衰ばね）variant の追加

### 動機
velocity を **永続状態**として保持すれば、target 更新の離散性は velocity への積分でならされる。Edge の impulse スクロール（設計文書 §2.2）と同じ考え方。

### 数学モデル
```
dv/dt = ω²·(target - current) - 2ω·v
dx/dt = v
```

臨界減衰（damping ratio = 1.0）に固定。初期値 ω = 10 rad/s（`1/ω = 100ms` が pursuit onset に整合）。semi-implicit Euler で積分。

### ステップ応答
```
x(t) = T · (1 - (1 + ωt)·e^(-ωt))
v(t) = T · ω² · t · e^(-ωt)
```

- 初速 0（ease-in が構造的に組み込まれる）
- オーバーシュートなし（臨界減衰の定義）
- ピーク velocity が `t = 1/ω = 100ms` で起きる

### Effect の分離
単一の `Effect::ScrollTo(u32)` を incremental / absolute 2 種に分離：
- `Effect::ScrollTo(y)` — absolute（`gg`, `G`, `Ngg`, TOC, 検索）：target 更新のみ
- `Effect::ScrollBy { target, impulse_px }` — incremental（`j/k/Ctrl-D/Ctrl-U`）：target 更新 + `add_impulse`

`add_impulse` は `ExpDecay*` では no-op、`DampedSpring` では velocity に加算。既存 variant の挙動は保全。

### impulse_gain の選定
`add_impulse(N)` を "N px 分余計に進ませたい" と解釈した場合、ゼロ初速からの臨界減衰で `add_impulse(v₀)` がもたらす最大偏差は `v₀ / (ω·e) ≈ v₀ / 27.18`。しかしここで採用したのは `gain = 2ω` で「単位 impulse が `v₀ / (2ω)` の最終変位に相当」という別解釈（これは誤り、後述）。

## 試行 3: オーバーシュート問題

### 現象
単発 `j` 押下でも、**target を通過してから戻る**挙動が視認できる。ユーザが「止まる瞬間にぴくっとわずかに動く」と報告。

### 解析解
初期条件 `x(0) = 0, v(0) = v₀, target = T` での臨界減衰解:
```
x(t) = T + (-T + (v₀ - ωT)·t) · e^(-ωt)
```

オーバーシュートしないための条件は `v₀ + ω·(x₀ - T) ≤ 0`。
`x₀ = 0` の場合、`v₀ ≤ ωT`。

impulse を `gain·ΔT` で表すと:
```
gain ≤ ω    ← オーバーシュート不可の閾値
```

現在の `gain = 2ω = 20` は閾値の 2 倍。単発 `j` (ΔT=48px) での overshoot = 解析的に 6.49px（at t=200ms, ≈ 14%）。

### gain 別の挙動
| gain | 単発応答 | 初速 | 体感 |
|---|---|---|---|
| `0` | `T·(1-(1+ωt)e^(-ωt))`（純粋臨界減衰） | `v(0)=0` | 完全な ease-in、settle ~500ms |
| `ω` | `T·(1-e^(-ωt))`（1 次指数） | `v(0)=ωT` | ease-in なし、settle ~300ms |
| `2ω`（現状） | 6.49px 行き過ぎ戻り | `v(0)=2ωT` | 「ぴくっ」と感じる |

`gain = 0` と `gain = ω` はどちらも closed-form、オーバーシュートなしで単調収束。中間値も代数的に扱える。

## 未解決: 解析的解 vs. アドホック数式

上記は「臨界減衰 2 階 ODE + 線形 impulse 加算」という**閉じた系**の枠内で最適 gain を選ぶ議論。しかし本当にこの枠が最適かは別問題である。

### 解析的アプローチが失っているもの
- 臨界減衰は settling を遅らせる。`gain ≤ ω` を守る限り、単発応答は 1 次指数と同等の rate ω。
  settle 時間は `~3/ω` で、ω をこれ以上大きくすると ease-in 窓が 100ms より縮む。
- 「ease-in が長い × 応答が速い」を両立するには、**時変 ω(t)** や **非線形 damping** が必要だが、それは もはや closed-form ではなく ad-hoc。

### アドホック候補の設計空間
- **Position-dependent damping**: target 近傍で damping を強化（`c(x) = 2ζω · (1 + k/|x-target|)` 等）。数学的には扱いにくいが「止まる瞬間」だけ制御できる。
- **Velocity clamping**: `|v| > v_max` のとき強制減衰。impulse の暴走を構造的に抑える。
- **Two-phase 軌道**: 前半 ease-in、後半 ease-out の bezier / tanh を **duration ベース**で貼り合わせる（Chrome cubic-bezier 式）。target 更新で再計算。連続入力時の連続性が課題。
- **Arrival steering (Reynolds)**: target 近傍で減速率を距離依存に変化。ゲーム AI 由来。オーバーシュート構造的排除。
- **Time-varying ω**: 初期 100ms は ω = 10、以降 ω = 20 と切り替える。closed-form で書けるが piecewise。
- **Snap hysteresis**: `|x - target| < R` に入ったら spring を切って 2 次 ease-out に引き継ぐ。R を大きめに取れば overshoot 完全排除。

### 観点: どちらが「正しい」か
解析解は「誰にでも同じ軌道が出る」「パラメータが少ない」利点。しかし体感最適との一致は保証されない。実際、Chrome は fixed-time bezier (§2.2)、Edge は impulse + friction、多くのネイティブ UI は tanh ease-out など **ad-hoc 派が主流**。

一方で本プロジェクトのコンテキスト:
- 読み物ツールなので、軌道の「意外性」は嫌われる（Edge impulse のような慣性は「行きすぎ」に感じやすい）。
- 上流層（`scroll_policy.rs`）がすでにアドホック（3 段密度分類）。下流層まで ad-hoc だとチューニング面が広がりすぎる。
- 設計文書 §3 は「連続時間 ODE」を推奨。§5.1 は "current = target + (current - target)·exp(-λ(d,t)·dt)" と時変 λ を示唆。

**暫定の結論（確定していない）**: まず `gain ≤ ω` 範囲でチューニングし、それでも "ぴくっ" 以外の不満が出たら ad-hoc に踏み込む。`gain = ω` と `gain = 0` を切り替えて比較する余地を残す（両者とも closed-form なのでリグレッション解析がしやすい）。

## パラメータ現状

`src/viewer/scroll_animator.rs`:
```rust
const SPRING_OMEGA: f64 = 10.0;              // rad/s, peak at 100ms
const SPRING_IMPULSE_GAIN: f64 = 2.0 * SPRING_OMEGA;  // ← 現在 overshoot する設定
const SPRING_SNAP_VELOCITY: f64 = 5.0;        // px/s
```

`SNAP_THRESHOLD_PX = 0.5`（他 variant と共有）。

## 次のアクション候補

1. `SPRING_IMPULSE_GAIN = SPRING_OMEGA` に下げ、overshoot 排除の体感を確認。ease-in 感が不足するなら `0.5·ω` まで試す。
2. それでも不満があれば ad-hoc（velocity clamping or snap hysteresis）を検討。設計文書 §5.3「減速末尾の延長」とも整合。
3. CLI/config から `omega`, `impulse_gain` を調整可能にする（現在はハードコード）。A/B の粒度を上げる。
4. `ExpDecayAdaptive` 同様、距離に応じた `omega` の適応版（`omega(d) = base / (1 + ln(1 + d/viewport))`）— gg/G 等の大ジャンプで settle を延ばす。

## 参照

- `docs/2026-04-18-design-scroll-animation.md` — 本題の根拠（§4.3 pursuit onset, §5.1 ease-in, §2.2 Edge impulse）
- `docs/2026-04-18-experiment-subcell-scroll.md` — sub-cell 実装と 40ms half-life の根拠
- `docs/2026-04-19-plan-scroll-animator-extraction.md` — variant ベースへのリファクタ
- コミット: `6fcbf6c`（sub-cell）, `37fb95f`（モジュール分離）, `ea37025`（CLI flag）, `e132c47`（ExpDecayAdaptive）, 以降が本ログの変更
