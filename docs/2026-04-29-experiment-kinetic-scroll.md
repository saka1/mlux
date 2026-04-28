# 実験ログ: DampedSpring → Kinetic 置換

`docs/2026-04-20-experiment-scroll-animation-tuning.md`(DampedSpring 試行) の
続編。critical damping spring の体感の歪さを HCI 文献に照らして診断し、
**iOS 流の kinetic (momentum + 摩擦)** に置き換えた。包括的な現状仕様は
`design/scroll-animation.md` を参照。

## 動機: DampedSpring の体感問題

実装後の試用で「アニメーション末期にスッと止まらず、最後に微小ピクセルを
刻んで止まる」現象が報告された。原因は以下の 2 つが重なっていた。

### 構造的問題: critical damping の slow tail

ステップ応答 `x(t) = T·(1 - (1 + ωt)·e^(-ωt))` の末期は (1+ωt) の多項式
因子に支配されて指数より遅く減衰する。`ω = 10`(pursuit onset 100ms に
合わせた値) を固定する限り、tail を縮める自由度がない。

| 補間方式 | tail decay rate (settle constant) |
|---|---|
| ExpDecay (post-ramp) | `ln(2) / 40ms ≈ 17.3 /s` |
| Kinetic (impulse 経由) | `1 / τ = 20 /s` (τ=50ms) |
| DampedSpring (impulse 経由, gain=ω) | `ω = 10 /s` |
| DampedSpring (set_target, no impulse) | `(1+ωt)·e^(-ωt)` 多項式 tail |

DampedSpring tail は **ExpDecay の 1.7× 遅い**。critical damping は ω 一個で
ease-in と settle time が結合する制約があり、両者を独立にチューニングできる
ExpDecay の smoothstep ramp + half_life より構造的に不利。

### スナップ条件の knock-on: pixel ticking

`y_offset = current.round() as u32` で整数化するため、velocity が
`SPRING_SNAP_VELOCITY = 5 px/s` ぎりぎりまで残ると **1 ピクセル更新が
~200ms に 1 回**という creep 域に入る。これが「微小ピクセルを刻む」体感の
直接原因。ExpDecay は `residual < 0.5px` 単独で snap するためこの域に
入らない。

## HCI 文献: 終端の作法

- **非対称イージングの普遍性**: Material Design `fast-out-slow-in`、Apple
  HIG easeOut、CSS `ease-out` ——いずれも入力応答 (incoming motion) は
  減速側に重みを置く。ただし「鋭く減速」というより「長めの減速プロファイル
  + 固定時長で打ち切り」で creep が見える前に終わらせる設計。
- **ballistic + corrective モデル**: Fitts 系の到達運動研究で、人間の動きは
  「速い ballistic → 遅い corrective」の 2 相。アニメーションがこの構造に
  従うと「自然」と感じられる。iOS kinetic scroll はこの素直なモデル。
- **smooth pursuit 終了の生理**: target 停止後 ~100ms 視覚追跡が慣性で動く。
  急停止 (1 次微分連続な急減速) は「到着の手応え」として好まれ、緩い tail は
  「まだ動いてる?」の認知未完了感を生む。
- **"急減速は気持ちいいか"**: 一般論 yes。ただし(a) 速度の不連続でなく 1 次
  微分連続な急減速、(b) 中盤の減速プロファイルから終端到来が予測可能、
  (c) "slow-in with long body, then last 15-20% steepens" の j 字に近い
  カーブが好まれる傾向 (Hoffmann & Hilliges)。

→ critical damping は HCI 文献の支持と逆方向 (tail が緩む)。物理的優美さは
あるが scroll 用途では価値にならない。

## Kinetic の選択

iOS UIScrollView (Bas Ording, 2007) で確立し、ほぼ全 OS の標準となった
方式。一階の摩擦のみの ODE:

```
dv/dt = -v / τ        →  v(t) = v₀ · e^(-t/τ)
dx/dt = v             →  x(t) = x₀ + v₀·τ·(1 - e^(-t/τ))
```

**着地点 `x_∞ = x₀ + v₀·τ` が瞬時に確定する** のが知覚的特徴。長い指数 tail
でも creep 感が出ないのは「あそこで止まる」と既に予測できているから(暗黙
学習: v₀·τ → 着地距離)。

ExpDecay と数学的にはほぼ同じ一階指数減衰だが、**主語が違う**:

| | 主語 | 入力 | 着地点 |
|---|---|---|---|
| ExpDecay | position が target を追う | target を動かす | target そのもの |
| Kinetic | velocity が摩擦で減衰 | velocity を蹴る | x₀ + v₀·τ(計算で決まる) |

### キーボード入力との橋渡し

iOS は指のフリック (連続値の初速) が前提。キーボードのキー押下 (離散
インパルス) は入力分布が違うので、**この相性は HCI 文献では未開拓**。

mlux では target_y を upstream で常に維持(status bar / clamping のため)
しつつ、不変条件 `target == current + velocity·τ` を保つよう 2 系統の
velocity 注入経路を設計:

| Effect | API | 操作 |
|---|---|---|
| `ScrollBy { impulse_px }` (j/k) | `add_impulse(d)` | `v += d/τ` (累積 → momentum) |
| `ScrollTo(y)` (gg/G/search) | `set_landing(y)` | `v = (y-current)/τ` (置換 → 過去 momentum 破棄) |

絶対ジャンプで momentum を上書きするのは iOS の "scroll-to-top" タップと
同じ挙動。

### 初期パラメータ

```rust
const DEFAULT_KINETIC_TAU_MS: f64 = 50.0;       // settle ~5τ = 250ms
const KINETIC_SNAP_VELOCITY:  f64 = 30.0;        // px/s, ≈ 1px/33ms
```

- τ=50ms: 単発 j (72px) で settle ~250ms。ExpDecay の 320ms と同オーダー、
  iOS の 330ms 相当より速い設定。
- snap velocity 30 px/s: 1 frame@30fps で 1 px の境界。これより遅い動きは
  整数丸めで visible creep になるので強制 snap。DampedSpring の 5 px/s は
  逆に creep 域を許容してしまっていた。
- ease-in は **入れない**。iOS 的に「キー押下 = ユーザーが初速を与えた」と
  解釈する割り切り。pursuit onset latency 100ms は HCI 仮説として検証対象。

## 体感観察 (2026-04-29)

実機で `--scroll-animation=kinetic` を ExpDecayAdaptive と比較。

| 観察 | 設計予測との一致 |
|---|---|
| 単発 j の「2 段階感」が ExpDecayAdaptive で気になっていたが、Kinetic で解消 | 部分的: 同じ exp 型カーブだが kinetic τ=50ms < adaptive の effective τ ≈ 65ms (at d=72px)。settle 250ms vs 360ms で **30% 圧縮**され、ballistic + corrective の 2 相に分離知覚する余裕がなくなる。意図したというより副次効果 |
| キー連打を離した後の停止感は Kinetic と ExpDecay でほぼ同じ | 数学的にそう: ExpDecay 5×hl ≈ 200ms、Kinetic 5τ ≈ 250ms。知覚閾値内 |
| 加速・中間段階で ExpDecay は「ふわっ」、Kinetic は「キビキビ」 | 設計差そのもの: ExpDecay の 100ms smoothstep ramp が "ふわっ" の正体。Kinetic は ramp なしで瞬時最高速 → "キビキビ" |

### 副次的な発見: pursuit onset の HCI 仮説

「Kinetic も悪くない」(キーボードでの ease-in 無し許容) という観察は、
HCI 文献の **pursuit onset latency 100ms ease-in 必須**仮説が、少なくとも
キーボード scroll では緩い可能性を示唆する。

オリジナルの pursuit onset 研究 (Schütz et al. 2011, Gegenfurtner 2016) は
連続的に動く目標を追う smooth pursuit の文脈で測定された値。**キー押下の
離散 scroll では、目標の動きが事前に予測可能** (押した瞬間に "下に動く" と
いう contract が確立) なので、pursuit onset の影響が緩い可能性がある。
touch/mouse で必須とされてた ease-in が、キーボードでは省略しても許容
される——empirical hypothesis として面白い。

## 設定変更

- `--scroll-animation=damped-spring` → `--scroll-animation=kinetic`
- `--exp-preset=adaptive` の補間器: DampedSpring → Kinetic
- ExpDecay / ExpDecayAdaptive は無変更

## 残課題候補

- **ease-in の選択肢**: 現状の Kinetic は ramp なし。希望すれば smoothstep
  ramp を impulse の velocity kick に被せて 100ms 分散させられる。
  「キビキビが悪くない」感触なら触らない方が良い。希望に応じて分岐。
- **大ジャンプ時の初速暴走**: gg/G で `v = (target - current) / τ` がそのまま
  注入されるため、長距離ジャンプで初速が極端に大きくなる。viewport 単位で
  クランプするか、別系統の designed easing curve に委譲する選択肢あり。
  現状は ExpDecayAdaptive で代替可能なので保留。
- **ExpDecayAdaptive と Kinetic の住み分け**: 両者とも一階指数減衰族。
  「ramp あり/なし」「target chase / velocity」「distance-adaptive あり/なし」の
  3 軸の組み合わせのうち、現状は (ramp+chase+adaptive)、(ramp+chase+fixed)、
  (no-ramp+velocity+fixed) の 3 点。残る組み合わせの実験余地あり。

## 参照

- `docs/2026-04-18-design-scroll-animation.md` — オリジナル設計文書
  (pursuit onset, ease-in 知見の根拠)
- `docs/2026-04-18-experiment-subcell-scroll.md` — sub-cell 実装と
  half_life=40ms の根拠
- `docs/2026-04-19-plan-scroll-animator-extraction.md` — variant ベース
  リファクタ
- `docs/2026-04-20-experiment-scroll-animation-tuning.md` — DampedSpring の
  詳細試行記録
- `docs/design/scroll-animation.md` — 現在仕様の包括的記述
