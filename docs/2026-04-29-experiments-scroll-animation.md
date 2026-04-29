# スクロールアニメーション実験ジャーナル (2026-04-18 〜 2026-04-29)

`docs/2026-04-18-design-scroll-animation.md` (HCI 文献サーベイ) を出発点に、
TUI Markdown ビューア mlux のスクロール下流補間層 (`current → target` 追従)
を 3 段階で実装・試行した記録。採択された現状仕様は
`docs/design/scroll-animation.md` に集約されている。本書は「なぜそこに至ったか」
と各試行で得た raw な観察・診断を時系列で残す。

各 Phase は独立して読めるよう書いてあるが、論点は連鎖している:
**Phase 1** で sub-cell + ExpDecay の体感ベースラインを取り、
**Phase 2** で pursuit onset latency を ease-in ramp で補正しつつ DampedSpring
を試したが構造的限界に当たり、
**Phase 3** で iOS 流 Kinetic に置き換えて creep を解消した。

---

## Phase 1 — Sub-cell scroll + ExpDecay (2026-04-18)

設計文書 (`2026-04-18-design-scroll-animation.md`) の下流補間層の**最小実装**を
作り、体感を取ったセッション。「セル単位離散ジャンプ → サブセル(ピクセル)粒度」
の現実性を実証することが目的。

### 実装したもの

下流補間層 (`target → current` 追従) の最小版。

#### 補間コア

`src/viewer/layout.rs::interpolate_step`:

```
current = target + (current - target) * 0.5^(dt / half_life)
```

- フレームレート非依存の閉形式 (設計文書 §3.2 必須要件)
- `SCROLL_HALF_LIFE_MS = 40` (設計文書の推奨 80–150ms より攻めた値、根拠は後述)
- 残距離 <0.5px でスナップ、`velocity=0` 相当 (§3.5)

#### 状態 (`ScrollState`)

| フィールド | 型 | 役割 |
|---|---|---|
| `target_y` | `u32` | 目標位置。`ScrollTo` 効果で即値更新 |
| `current_y` | `f64` | アニメ中の連続位置、毎フレーム `tick` で進む |
| `y_offset` | `u32` | 描画時の離散位置 = `current_y.round() as u32` |

設計文書 §0 の「target は離散イベント駆動で即値」「current は連続時間
ステートマシンで追従」という 2 層モデルをそのまま実装。

#### 毎 tick の処理

`src/viewer/mod.rs` inner loop 冒頭:

```rust
let dt = now.duration_since(last_tick);
if vp.scroll.tick(dt) {
    vp.dirty = true;
}
```

- `is_animating()` が true の間は `frame_budget` (32ms) で回す
- 停止したら `watch_interval` (200ms) ないし無限長に戻り、再描画ループが
  消える (省電力条件、§3.5)

#### KGP 配置の変更

`src/viewer/viewport.rs` の `ScrollTo` 効果から cell 境界 snap を削除。
`y=src_y` に非セル整数値が入るようになった。

ただし **Split ケース (ビューポートがタイル境界を跨ぐ瞬間) だけは snap
フォールバック** を残している。`top_src_h` が `cell_h` の倍数でないと
KGP の `r=行数` と `h=ピクセル` が一致せず、端末が画像を縦方向に圧縮
(伸縮アーティファクト) する。これは設計文書に記載がない mlux 固有の罠で、
`display_state.rs::visible_tiles_for_render` に集約。

#### 連打時の累積挙動

`src/viewer/mode_normal.rs` の累積系スクロール (`j`, `k`, `Ctrl-D`,
`Ctrl-U`) は、**次の target を `y_offset` ではなく `target_y` から積み上げる**
ように変更 (設計文書 §3.3)。

- 変更前: 連打中、各キーで「見えている位置から step」を計算するため、
  前のアニメの遅れぶんだけ累積が過小になる
- 変更後: 連打した回数ぶん target がきっちり積み上がる

回帰テスト: `scroll_accumulates_onto_target_not_render_position`。
絶対 target の `gg`/`G`/`:<n>G`/検索開始位置は意味が違うのでそのまま。

### ユーザ観察

- **スクロール中に文字が読みやすくなった** (= 明確な改善の主観)。
  設計文書 §4.1 の網膜速度 2.5°/s 閾値と整合。離散ジャンプはブラーで
  読めなかった領域が、サブセル補間で読める領域に入った結果、スクロール中
  にも情報処理が走るようになった。§4.2 のドリフトテキスト読字モデル
  (fixation → smooth pursuit) にも合致。
- **半減期 80ms は「遅く」感じた** → 40ms に下げて「軽くなった」。設計
  文書推奨初期値 (120ms) より攻めた設定。「読む」を軽視せず、かつ即応性を
  優先した現実解として 40ms が着地点。
- **知覚的遅延の存在**: 物理的な到達時間 (0 → ~240ms) が延びたのに加えて、
  「スクロール中も情報処理が走る」ことによる主観時間の延びが乗っている。
  後者は改善の副作用で、消そうとするとサブセル補間のメリットが失われる。

### 解けたこと / 残っていたこと

- ✅ サブセルスクロールは視覚的にメリットを生み、実装コストは中程度
  (変更 ~200 行、テスト追加含めて) で収まった
- ✅ 指数減衰 + half_life=40ms で「ひどくない」ベースラインは取れた
- ⏭ §3.4 距離適応: `j` 1 発と `gg`/`G` を同じ half_life で動かしている
- ⏭ §5.1 初期 ramp-up: 純粋指数減衰は出だしが一番速い (=初速無限大)。
  pursuit 立ち上がり潜時 100ms に合わせた ease-in を入れるかどうか
- ⏭ §5.3 減速末尾の延長: 末尾でさらに half_life を短くして「ピタッと止まる」
  感覚を作る
- ⏭ §4.6 SDAZ 近似: 高速スクロール中の描画簡略化
- ⏭ 上流層全般: EWMA 速度推定、velocity-dependent gain

### この時点でのチューニング表

| 変数 | 値 | 位置 |
|---|---|---|
| `SCROLL_HALF_LIFE_MS` | 40.0 | `src/viewer/layout.rs` |
| スナップ位置閾値 | 0.5px | `ScrollState::tick` |
| `frame_budget` | 32ms (~31fps) | `src/config.rs` |

---

## Phase 2 — ease-in ramp + DampedSpring (2026-04-19 〜 2026-04-20)

設計文書 §4.3 の "pursuit onset latency 100ms" (Schütz 2011, Gegenfurtner
2016) を ExpDecay に取り込むため ease-in ramp を後付けし、さらに critical
damping spring を導入した試行。**最終的に DampedSpring は採用しなかった**。

### 前提状態

- `ScrollAnimator` を `enum` の closed variant 集合へリファクタ済み
  (`ExpDecay` のみ→`ExpDecayAdaptive` を追加済)
- 上流層 (`scroll_policy.rs`) は Normal/Mid/High の 3 段加速を持ち、キー連打で
  1 キーあたりの target 増分が 48→77→86px と拡大

### 試行 2.1 — ExpDecay に ease-in ramp を後付け

#### 実装

`ExpDecay { ..., ramp_elapsed_ms: f64 }` を追加し、`set_target(restart_ramp:
bool)` の `true` ケースで `ramp_elapsed_ms = 0.0` にリセット。`tick` 内で
effective half-life を smoothstep で `120ms → 40ms` に滑らかに変化させる。

#### 体感

「ふわっと動き出す」のは好感触。**ただし次の問題が発覚:**

#### 問題 — 階段状の apparent-speed 急変

`j` 長押し時、key-repeat で target が毎 ~30ms ごとに 48→125→211→297px と
積み上がる。ease-in (effective hl = 120ms) で current が遅い間に target が
遥か先へ行き、ramp 完了時に「一気に追いかける」挙動になる。

Python シミュレーション (`dt=16ms, 60fps`):

```
t=  0ms  tgt=  48  cur=   4.2  +4.2 px/f  ramp あり
t= 32ms  tgt= 125  cur=  20.5  +12.2 px/f ramp あり
t= 64ms  tgt= 211  cur=  62.0  +28.4 px/f ramp あり
t= 96ms  tgt= 297  cur= 141.7  +49.1 px/f 全速  ← 急変
```

体感は「最初ほぼ止まって見え、突然動き出す」。原因は ease-in そのもの
ではなく、**target が離散的に跳ぶ × 位置ベース補間の組み合わせ**。
ExpDecay は毎フレーム残差から velocity を導出するので、target の段差が
そのまま apparent-speed の段差として現れる。

### 試行 2.2 — DampedSpring (臨界減衰ばね) variant の追加

#### 動機

velocity を**永続状態**として保持すれば、target 更新の離散性は velocity
への積分でならされる。Edge の impulse スクロール (設計文書 §2.2) と同じ
考え方。

#### 数学モデル

```
dv/dt = ω²·(target - current) - 2ω·v
dx/dt = v
```

臨界減衰 (damping ratio = 1.0) に固定。初期値 ω = 10 rad/s
(`1/ω = 100ms` が pursuit onset に整合)。semi-implicit Euler で積分。

ステップ応答:

```
x(t) = T · (1 - (1 + ωt)·e^(-ωt))
v(t) = T · ω² · t · e^(-ωt)
```

- 初速 0 (ease-in が構造的に組み込まれる)
- オーバーシュートなし (臨界減衰の定義)
- ピーク velocity が `t = 1/ω = 100ms` で起きる

#### Effect の分離

単一の `Effect::ScrollTo(u32)` を incremental / absolute 2 種に分離:

- `Effect::ScrollTo(y)` — absolute (`gg`, `G`, `Ngg`, TOC, 検索): target 更新のみ
- `Effect::ScrollBy { target, impulse_px }` — incremental (`j/k/Ctrl-D/Ctrl-U`):
  target 更新 + `add_impulse`

`add_impulse` は `ExpDecay*` では no-op、`DampedSpring` では velocity に加算。
既存 variant の挙動は保全。

#### 問題 — 単発でもオーバーシュート

単発 `j` 押下でも、**target を通過してから戻る**挙動が視認できる。
ユーザが「止まる瞬間にぴくっとわずかに動く」と報告。

##### 解析解

初期条件 `x(0) = 0, v(0) = v₀, target = T` での臨界減衰解:

```
x(t) = T + (-T + (v₀ - ωT)·t) · e^(-ωt)
```

オーバーシュートしないための条件は `v₀ + ω·(x₀ - T) ≤ 0`。`x₀ = 0` の
場合 `v₀ ≤ ωT`。impulse を `gain·ΔT` で表すと:

```
gain ≤ ω    ← オーバーシュート不可の閾値
```

採用していた `gain = 2ω = 20` は閾値の 2 倍。単発 `j` (ΔT=48px) での
overshoot = 解析的に 6.49px (at t=200ms, ≈ 14%)。

##### gain 別の挙動

| gain | 単発応答 | 初速 | 体感 |
|---|---|---|---|
| `0` | `T·(1-(1+ωt)e^(-ωt))` (純粋臨界減衰) | `v(0)=0` | 完全な ease-in、settle ~500ms |
| `ω` | `T·(1-e^(-ωt))` (1 次指数) | `v(0)=ωT` | ease-in なし、settle ~300ms |
| `2ω` (試行時) | 6.49px 行き過ぎ戻り | `v(0)=2ωT` | 「ぴくっ」と感じる |

`gain = 0` と `gain = ω` はどちらも closed-form、オーバーシュートなしで
単調収束。中間値も代数的に扱える。

#### 解析的アプローチ vs. ad-hoc 工夫

「臨界減衰 2 階 ODE + 線形 impulse 加算」という閉じた系の枠内で最適 gain
を選ぶことは可能。ただし:

- 臨界減衰は settling を遅らせる。`gain ≤ ω` を守る限り、単発応答は 1 次
  指数と同等の rate ω。settle 時間は `~3/ω` で、ω をこれ以上大きくすると
  ease-in 窓が 100ms より縮む。
- 「ease-in が長い × 応答が速い」を両立するには、**時変 ω(t)** や
  **非線形 damping** が必要だが、それはもはや closed-form ではなく ad-hoc。

検討した ad-hoc 候補 (いずれも未採用):

- **Position-dependent damping**: target 近傍で damping 強化
- **Velocity clamping**: `|v| > v_max` で強制減衰
- **Two-phase 軌道**: ease-in + ease-out の bezier/tanh を duration ベースで
  貼り合わせる (Chrome cubic-bezier 式)
- **Arrival steering** (Reynolds、ゲーム AI 由来)
- **Time-varying ω**: 初期 100ms は ω=10、以降 ω=20 と切り替え (piecewise)
- **Snap hysteresis**: `|x - target| < R` に入ったら spring を切って 2 次
  ease-out に引き継ぐ

### Phase 2 終了時のパラメータ

```rust
const SPRING_OMEGA: f64 = 10.0;                       // rad/s, peak at 100ms
const SPRING_IMPULSE_GAIN: f64 = 2.0 * SPRING_OMEGA;  // overshoot する設定
const SPRING_SNAP_VELOCITY: f64 = 5.0;                // px/s
```

`SNAP_THRESHOLD_PX = 0.5` (他 variant と共有)。

### この時点での選択肢

1. `SPRING_IMPULSE_GAIN = SPRING_OMEGA` に下げ、overshoot 排除を確認
2. ad-hoc 工夫 (velocity clamping or snap hysteresis) を導入
3. CLI/config で `omega`, `impulse_gain` を runtime 調整可能に
4. 距離適応版 `omega(d) = base / (1 + ln(1 + d/viewport))`

実際には Phase 3 で別方式 (Kinetic) に置換することになり、これらは未着手。

---

## Phase 3 — Kinetic への置換 (2026-04-29)

DampedSpring の体感の歪さを HCI 文献に照らして診断し、**iOS 流の kinetic
(momentum + 摩擦)** に置き換えた。これが採用された現状方式。

### DampedSpring の体感問題 (2 つの原因)

実装後の試用で「アニメーション末期にスッと止まらず、最後に微小ピクセルを
刻んで止まる」現象が報告された。

#### 構造的問題: critical damping の slow tail

ステップ応答 `x(t) = T·(1 - (1 + ωt)·e^(-ωt))` の末期は `(1+ωt)` の多項式
因子に支配されて指数より遅く減衰する。`ω = 10` (pursuit onset 100ms に
合わせた値) を固定する限り、tail を縮める自由度がない。

| 補間方式 | tail decay rate (settle constant) |
|---|---|
| ExpDecay (post-ramp) | `ln(2) / 40ms ≈ 17.3 /s` |
| Kinetic (impulse 経由) | `1 / τ = 20 /s` (τ=50ms) |
| DampedSpring (impulse 経由, gain=ω) | `ω = 10 /s` |
| DampedSpring (set_target, no impulse) | `(1+ωt)·e^(-ωt)` 多項式 tail |

DampedSpring tail は ExpDecay の 1.7× 遅い。critical damping は ω 一個で
ease-in と settle time が結合する制約があり、両者を独立にチューニングできる
ExpDecay の smoothstep ramp + half_life より構造的に不利。

#### スナップ条件の knock-on: pixel ticking

`y_offset = current.round() as u32` で整数化するため、velocity が
`SPRING_SNAP_VELOCITY = 5 px/s` ぎりぎりまで残ると **1 ピクセル更新が
~200ms に 1 回**という creep 域に入る。これが「微小ピクセルを刻む」体感の
直接原因。ExpDecay は `residual < 0.5px` 単独で snap するためこの域に入らない。

### HCI 文献: 終端の作法

- **非対称イージングの普遍性**: Material Design `fast-out-slow-in`、
  Apple HIG easeOut、CSS `ease-out` ——いずれも入力応答 (incoming motion)
  は減速側に重みを置く。ただし「鋭く減速」というより「長めの減速プロファイル
  + 固定時長で打ち切り」で creep が見える前に終わらせる設計。
- **ballistic + corrective モデル**: Fitts 系の到達運動研究で、人間の動きは
  「速い ballistic → 遅い corrective」の 2 相。アニメーションがこの構造に
  従うと「自然」と感じられる。iOS kinetic scroll はこの素直なモデル。
- **smooth pursuit 終了の生理**: target 停止後 ~100ms 視覚追跡が慣性で動く。
  急停止 (1 次微分連続な急減速) は「到着の手応え」として好まれ、緩い tail は
  「まだ動いてる?」の認知未完了感を生む。
- **「急減速は気持ちいいか」**: 一般論 yes。ただし (a) 速度の不連続でなく 1 次
  微分連続な急減速、(b) 中盤の減速プロファイルから終端到来が予測可能、
  (c) "slow-in with long body, then last 15-20% steepens" の j 字に近い
  カーブが好まれる傾向 (Hoffmann & Hilliges)。

→ critical damping は HCI 文献の支持と逆方向 (tail が緩む)。物理的優美さは
あるが scroll 用途では価値にならない。

### Kinetic の選択

iOS UIScrollView (Bas Ording, 2007) で確立し、ほぼ全 OS の標準となった
方式。一階の摩擦のみの ODE:

```
dv/dt = -v / τ        →  v(t) = v₀ · e^(-t/τ)
dx/dt = v             →  x(t) = x₀ + v₀·τ·(1 - e^(-t/τ))
```

**着地点 `x_∞ = x₀ + v₀·τ` が瞬時に確定する** のが知覚的特徴。長い指数
tail でも creep 感が出ないのは「あそこで止まる」と既に予測できているから
(暗黙学習: v₀·τ → 着地距離)。

ExpDecay と数学的にはほぼ同じ一階指数減衰だが、**主語が違う**:

|  | 主語 | 入力 | 着地点 |
|---|---|---|---|
| ExpDecay | position が target を追う | target を動かす | target そのもの |
| Kinetic | velocity が摩擦で減衰 | velocity を蹴る | x₀ + v₀·τ (計算で決まる) |

#### キーボード入力との橋渡し

iOS は指のフリック (連続値の初速) が前提。キーボードのキー押下 (離散
インパルス) は入力分布が違うので、**この相性は HCI 文献では未開拓**。

mlux では target_y を上流で常に維持 (status bar / clamping のため) しつつ、
不変条件 `target == current + velocity·τ` を保つよう 2 系統の velocity
注入経路を設計:

| Effect | API | 操作 |
|---|---|---|
| `ScrollBy { impulse_px }` (j/k) | `add_impulse(d)` | `v += d/τ` (累積 → momentum) |
| `ScrollTo(y)` (gg/G/search) | `set_landing(y)` | `v = (y-current)/τ` (置換 → 過去 momentum 破棄) |

絶対ジャンプで momentum を上書きするのは iOS の "scroll-to-top" タップと
同じ挙動。

#### 初期パラメータ

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

### 体感観察 (2026-04-29)

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

### 設定変更

- `--scroll-animation=damped-spring` → `--scroll-animation=kinetic`
- `--exp-preset=adaptive` の補間器: DampedSpring → Kinetic
- ExpDecay / ExpDecayAdaptive は無変更

---

## Phase 4 — Kinetic 末尾の "limbo フレーム" 修正 (2026-04-30)

`--exp-preset=adaptive --mouse` でマウスホイール 1 ノッチを回したときに
「カクカクと 2 段階で目標に着地する」感触が報告された。原因と修正の記録。

### 症状の特定

シミュレータで δ=48px (= `wheel_step=2 × cell_h=24`)、τ=50ms、
`frame_budget=32ms` の Kinetic glide を整数 y_offset (= `x.round()`)
に離散化すると次の軌跡:

```
F1=23, F2=35, F3=41, F4=44, F5=46, F6=47, F7=47 (Δ=0!), F8=48 (snap)
```

F7 で:
- `x_sub = 47.46`, `residual = 0.54`, `|v| = 10.88 px/s`
- 旧 snap 条件 `residual<0.5 AND |v|<30` → residual gate が外れる →
  まだ animating 扱い → `tick()` が前フレームと同じ `y_offset=47` を
  返し、`mod.rs:362` で false 判定 → 再描画スキップ
- F8 (32ms 後) で `x_sub=47.71`, `residual=0.29` が gate を抜けて snap

結果として **整数 y_offset が 32ms 静止 → snap で 1px ジャンプ** が
末尾に現れ、「2 段階の不連続移動」として知覚される。

### Limbo の幾何構造（パラメータ非依存）

整数丸め × 離散フレーム × 連続減衰の組み合わせから不可避に出る現象:

1. residual ∈ `[0.5, 1.0)` で `y_offset = target - 1` に張り付く
2. `|v| · dt < 0.5` で次フレームも整数 y が動かない
3. 上記が同時に成立 ⇒ Δ=0 フレーム = limbo

snap 条件 `residual<0.5 AND |v|<30` は (1) の境界(0.5)と (2) の境界
(`|v|·dt<0.5 ⇔ |v|<15.625` at dt=32ms) のどちらに対しても緩く、
ちょうど 1 フレームだけ取り残されるケースを起こしていた。
δ=72 (キーボード `j` の default 値) では residual の通過点と整数遷移が
ずれて偶然 limbo を踏まないだけで、δ=48 (マウスホイール default) や
δ=24 (`wheel_step=1`)、δ=56 (`cell_h=28` のターミナル) などでは踏む。

### 修正方針

snap 条件を **`(residual<R) AND (|v|·dt<S)` の二項**に組み直す。

| ゲート | 値 | 意味 |
|---|---|---|
| `R = KINETIC_SNAP_RESIDUAL_PX` | `1.0 px` | 整数遷移境界。「視認 1 px 内」。τ・δ・frame_budget から独立 |
| `S = SNAP_THRESHOLD_PX` | `0.5 px` | サブピクセル動きの境界 (既存) |
| `KINETIC_SNAP_VELOCITY` | `S / dt = 15.625 px/s` | dt=`frame_budget`=32ms から派生 |

意味付け:
- residual gate: target ピクセルにいる
- velocity gate: 次フレームの予測進行が 0.5 px 未満 ⇔ 次フレームでも
  整数 y は同じ ⇔ 待つ意味がない

線形外挿 `x_next ≈ x + v·dt` は exp 減衰の真値より過大評価する側
(`residual·(1 - e^(-dt/τ)) ≤ v·dt` がτ依存性なし)、なので「v·dt<0.5
⇒ snap」の判定は安全側に倒れて τ がどう変わっても limbo は出ない。
`R=1.0` も整数境界なので τ・δ から独立。

### 数値依存性

`KINETIC_SNAP_VELOCITY = 15.625` の値そのものは **`frame_budget=32ms`
依存**: `S / dt = 0.5 / 0.032 = 15.625`。`frame_budget` を変えたら
再計算が必要。`scroll_animator.rs` の定数コメントと
`kinetic_snap_velocity_matches_frame_budget` テストで
`ViewerConfig::default().frame_budget` から導出値を再計算してリテラルと
比較しているので、片方だけ変えるとビルド時に落ちる。

### 検証

- `kinetic_no_limbo_frame_in_visible_trajectory`: δ ∈ {24, 40, 48,
  56, 72, 96, 120} で離散フレームを 30 個まで歩き、Δ=0 フレームが
  出ないことを確認
- `kinetic_snaps_in_legacy_limbo_band_at_t_224ms`: 旧条件で踏んでいた
  正規の limbo 点 (δ=48, t=224ms) で `is_animating=false` をピン留め
- `kinetic_snap_velocity_matches_frame_budget`: 定数と派生式の同期を保証
- 既存の `kinetic_does_not_snap_while_fast` 等は不変 (高速 glide や
  initial impulse では両 gate がそもそも立たない)

### 影響範囲

- ExpDecay / ExpDecayAdaptive の snap 条件は無変更 (`apply_step` 内の
  `SNAP_THRESHOLD_PX` 判定はそのまま)
- Kinetic は target-passing glide (multi-impulse, set_landing) でも
  従来通り pass-through で snap しない (velocity gate が両 sign で対称)

### キーボード j とマウスホイールの体感差の理由 (補足)

旧条件下で δ=72 (key `j`, scroll_step=3) は limbo を踏まないが δ=48
(wheel notch, wheel_step=2) は踏む、という差が報告内容の核心だった。
これは τ・dt から決まる integer 軌跡が偶然 δ=72 で滑らかに見えるための
ものであり、scroll_animation の本質的な品質差ではなかった。Phase 3 で
「Kinetic 良い」と判定したときキーボードが評価対象だったため見逃して
いた、と整理できる。

---

## 残課題 (Phase 3 終了時点)

- **ease-in の選択肢**: 現状の Kinetic は ramp なし。希望すれば smoothstep
  ramp を impulse の velocity kick に被せて 100ms 分散させられる。
  「キビキビが悪くない」感触なら触らない方が良い。
- **大ジャンプ時の初速暴走** (`design/scroll-animation.md` でも指摘): gg/G で
  `v = (target - current) / τ` がそのまま注入されるため、長距離ジャンプで
  初速が極端 (例: 50000 px/s)。1 frame で数百 px 飛ぶ。viewport 単位で
  クランプするか、別系統の designed easing curve に委譲する選択肢あり。
  現状は ExpDecayAdaptive で代替可能なので保留。
- **ExpDecayAdaptive と Kinetic の住み分け**: 両者とも一階指数減衰族。
  「ramp あり/なし」「target chase / velocity」「distance-adaptive あり/なし」の
  3 軸の組み合わせのうち、現状は (ramp+chase+adaptive)、(ramp+chase+fixed)、
  (no-ramp+velocity+fixed) の 3 点。残る組み合わせの実験余地あり。

---

## 参照

- `docs/2026-04-18-design-scroll-animation.md` — 出発点の HCI 文献サーベイ
  (pursuit onset, Stevens 冪則, smooth pursuit、ease-in 知見の根拠)
- `docs/design/scroll-animation.md` — 採択された現状仕様の包括的記述
  (closed enum API, Kinetic の式と pixel ticking 対策, when to pick which)
- `docs/2026-04-12-design-scroll-acceleration.md` — 上流層の累積戦略
  (Normal/Mid/High 3 段密度分類)。本ジャーナルの target 増分 48→77→86px
  の出所
- `docs/2026-04-17-investigate-scroll-perf.md` — KGP 性能ボトルネック調査
  (本実験とは独立)
- 関連コミット: `6fcbf6c` (sub-cell)、`37fb95f` (モジュール分離)、
  `ea37025` (CLI flag)、`e132c47` (ExpDecayAdaptive)、`7f38bb7`
  (DampedSpring)、`9eb8e94` (Kinetic 置換)
