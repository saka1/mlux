# 履歴駆動 Kinetic アニメータ設計

スクロールアニメータ層を「永続派生状態を持たず、入力履歴から毎フレーム導出する」
方式へ書き直す設計判断の記録。次セッションで実装プランを起こすための土台。

関連:
- 現行設計: `docs/design/scroll-animation.md`
- 試行ログ: `docs/2026-04-29-experiments-scroll-animation.md`
- 上流設計: `docs/2026-04-12-design-scroll-acceleration.md`

---

## 1. 動機

### 1.1 観察: `Effect::ScrollBy` は default 経路ではほぼ死にコード

`add-scroll-animation` ブランチの差分を辿ると、`Effect` が以下の二項に分かれている。

```rust
ScrollTo(u32),                              // gg/G/search/TOC
ScrollBy { target: u32, impulse_px: i32 },  // j/k/Ctrl-D/Ctrl-U
```

両者の `apply()` 上の違いは `add_impulse(impulse_px)` を呼ぶか
`set_landing(target)` を呼ぶかだけで、両 method は `ExpDecay` /
`ExpDecayAdaptive` (= default) で no-op。
つまり **default config では `impulse_px` は計算されては捨てられている**。

5 箇所 (`mode_normal.rs:121-167`) で `impulse = y_new - y_old` を計算し、
`Effect` ヴァリアントを 2 つに分け、`apply()` で分岐し、3 variant ぶんの
`add_impulse` / `set_landing` を no-op 含めて実装する — これらは
**Kinetic 一個のためだけにある配管**。

### 1.2 根本原因: Kinetic の velocity は唯一の "位置以外の永続状態"

`ScrollState` のフィールドは現状すべて position-state:

| フィールド | 種別 |
|---|---|
| `y_offset` | 描画位置 (rounded) |
| `target_y` | 目標位置 |
| `animator.current` | サブピクセル位置 |
| `animator.velocity` (Kinetic のみ) | **過去 impulse の指数加重和** ← 異質 |

`velocity` は「ユーザがどれだけ激しく投げたかの記憶」であり、構造的に
他のフィールドとは性質が違う。これが `ScrollBy { impulse_px }` という
"状態 (target) + 遷移イベント (impulse) の混在" を Effect 層に滲ませている。

### 1.3 設計思想との衝突

このコードベースは **「永続派生状態を持たない」** 方向に揃っている:

- `Viewport::apply(mut self, ...) -> (Self, Vec<RenderOp>)` (move-style snapshot)
- `ScrollPolicy` は完全 stateless (`scroll_policy.rs:42-45`):
  > "Stateless — all decisions are derived from the history snapshot at call time."
- そして `scroll_policy.rs:14-20` には戦訓のコメントがある — 過去には
  `last_gap > DECAY_GATE` という永続状態 (state machine) を持っていたが、
  Mid↔Normal の境界で振動が起きたので「履歴ウィンドウから毎回 classify する」
  方式に転換した。

**Kinetic の `velocity` はまさにこの戦訓が指す類の永続派生状態**。

---

## 2. 中核アイデア: velocity は履歴の閉形式関数

Kinetic の物理 `dv/dt = -v/τ` に impulse 列 `(tᵢ, δᵢ)` を加えた系の解は:

```
v(t) = Σᵢ (δᵢ / τ) · e^(-(t - tᵢ)/τ)

x(t) = anchor + Σᵢ δᵢ · (1 - e^(-(t - tᵢ)/τ))
```

- 5τ ≈ 250ms より古い impulse は `1 - e^(-5)` ≈ 99.3% まで寄与しきる
  → 残差 1% 未満は `anchor` に畳み込んで履歴から落としてよい
- 各フレームで履歴を走査して上式を直接評価すれば、velocity の永続化は不要

**Kinetic アルゴリズムは保持、状態表現だけ変更**。HCI 上の体感 (momentum,
throw, scroll-to-top キャンセル) はビット単位で同一になる。

---

## 3. 採択する設計

### 3.1 一次資料の解像度を上げる: `delta_px` を記録

`InputHistory` のレコードを拡張する:

```rust
struct InputRecord {
    delta_px: i32,        // 符号付き = 方向 + マグニチュード
    timestamp: Instant,
}
```

`direction` 単独は `delta_px.signum()` で代替できるので、`scroll_policy`
側の `count_in_window` / `last_gap` も同じバッファを使い続けられる。

### 3.2 なぜ direction のみではなく magnitude も持つのか

「ウィンドウ内の direction-only 履歴 + 現在の `scroll_step` から近似」
する選択肢もある (実際に検討した)。3 つの選択肢の比較:

| 案 | 方式 | 失うもの |
|---|---|---|
| (a) ナイーブ近似 | `δ ≈ dir · current_scroll_step` | `5j` の count、文書端 clamp、Adaptive モードの履歴整合 |
| (b) 厳密リプレイ | フレーム時に `scroll_policy` を全履歴で再走査し当時の clamp も再構成 | 計算量は問題ないが実装が再帰的 (位置←δ←policy←履歴) |
| **(c) 記録時メモ化** | keypress 時の `δ_px` をそのまま記録 | i32 一個ぶんレコード太る (数十イベント = 数百バイト) |

採択は **(c)**。理由:

- `δ_px` は **ユーザ意図のサンプル (一次観測値)** であって、過去 impulse の
  累積結果ではない。`scroll_policy` が `direction` を観測値として記録して
  いるのと同種で、ただ解像度を上げた拡張に相当する
- (a) は `5j` と端 clamp で Kinetic の HCI 利点 (count に応じた throw 量)
  を毀損する
- (b) は `scroll_policy` の式変更が履歴の意味を遡及的に変えるので、再現性
  あるデバッグがむしろ困難になる
- 「永続派生状態を持たない」原則は **「観測値を記録するな」ではない**。
  `scroll_policy` の `count_in_window` も履歴を直接読む。`δ_px` 記録も
  この原則の系譜にある

### 3.3 アニメータは純関数

```rust
struct KineticParams { tau_ms: f64 }

impl KineticParams {
    fn position_at(&self, anchor: f64, history: &[InputRecord], now: Instant) -> f64 {
        let tau_s = self.tau_ms / 1000.0;
        anchor + history.iter()
            .map(|r| {
                let elapsed = now.duration_since(r.timestamp).as_secs_f64();
                r.delta_px as f64 * (1.0 - (-elapsed / tau_s).exp())
            })
            .sum::<f64>()
    }

    fn velocity_at(&self, history: &[InputRecord], now: Instant) -> f64 {
        let tau_s = self.tau_ms / 1000.0;
        history.iter()
            .map(|r| {
                let elapsed = now.duration_since(r.timestamp).as_secs_f64();
                (r.delta_px as f64 / tau_s) * (-elapsed / tau_s).exp()
            })
            .sum()
    }
}
```

`&self` で済む。フレームごとに `position_at` を呼んで `y_offset` を更新
する。`velocity` フィールドは消える。

### 3.4 anchor の進行ルール (履歴の prune)

履歴は無限に伸ばせないので prune するが、Kinetic では古い impulse でも
asymptotic に位置寄与を持つ。**捨てるときは `anchor` に畳み込む**:

```
on_prune(record):
    elapsed = now - record.timestamp
    anchor += record.delta_px * (1 - exp(-elapsed / tau_s))
```

5τ より古い impulse なら `(1 - exp(-5))` ≈ 0.993 ≈ 1 なので実質
`anchor += δ` でよい。これは ExpDecay の `SNAP_THRESHOLD_PX` と同種の
「サブピクセル化したら畳み込む」ロジックの再利用。

prune 条件は時刻ベース (5τ 経過) でも残差ベース (`|δ·exp(-elapsed/τ)| <
threshold`) でも構わない。実装側で素直なほうを選ぶ。

### 3.5 Effect の縮約

```rust
// Before
ScrollTo(u32),
ScrollBy { target: u32, impulse_px: i32 },

// After
ScrollImpulse(i32),    // j/k/Ctrl-D/Ctrl-U: history に push
ScrollAnchor(u32),     // gg/G/search/TOC: history flush + anchor 設定
```

- `target_y` フィールドは `anchor + history.iter().map(|r| r.delta_px).sum()`
  で導出可能。**フィールドとしては廃止してよい**(導出関数だけ残す)
- `set_landing` は `ScrollAnchor` の意味論に統合される
  - 「即着地」: `flush(); anchor = target;`
  - 「滑らかに着地」: `flush(); anchor = current; push((target - current))`
  どちらを既定にするかは UX 判断。現行 `set_landing` は後者と等価。

### 3.6 ExpDecay 系は今回触らない

ExpDecay の `current` は 1 スカラのみで、これも数式上は履歴畳み込みに
書き直せるが、得るものがない (今でも完全 Markovian、永続派生状態と呼べる
代物ではない)。**揃えるべきは Kinetic だけ**。

3 variant 共通の `tick(target, dt)` API は `position_at(now)` 系に揃える
か、enum dispatch のまま `Kinetic` 内部だけ履歴駆動にする。
これは実装時の判断。

---

## 4. 期待される効果

### 4.1 構造的に消えるバグクラス

`scroll_policy` の戦訓 — 永続状態が境界で振動する — がアニメータ層にも
適用される。たとえば:

- Kinetic snap 条件 (`residual<0.5 && |v|<30`) の境界 flicker 系バグ
- velocity 上書き / 加算の組み合わせ間違い (`add_impulse` を呼ぶべき場面で
  `set_landing` を呼ぶ等)

これらは「velocity は履歴の和として毎フレーム再計算される」設計では
構造的に発生しなくなる。境界条件の議論自体は残るが、状態スティック起因の
クラスは消える。

### 4.2 デバッグ容易性

velocity は内部的な要約量で、ログに出しても再現できない。履歴駆動にすると
**`history` を dump すれば任意のフレームを bit-exact で再現できる**。
試行ログ (`docs/2026-04-29-experiments-scroll-animation.md`) で多数試行する
ようなチューニング作業ではこれが効く。

### 4.3 API の意図伝達

`set_landing` が「velocity を破壊的上書きする」ことは
`scroll_animator.rs:227-244` のコメントを読まないとわからない。
`ScrollAnchor` (history flush + anchor 設定) は **意図がそのまま型に出る**。

---

## 5. トレードオフと懸念

### 5.1 計算量

フレームあたり O(events_in_window)。人間の入力速度 ~10 keypress/s、
window = 5τ ≈ 250ms なので window 内イベントは最大でも数件。
連射の極端なケースで数十。**実害なし**。

### 5.2 InputHistory の共有 vs 分離

`scroll_policy` も `InputHistory` を参照しているので、レコード拡張の影響を
受ける。選択肢:

- **共有**: `InputRecord` に `delta_px: i32` を持たせ、`scroll_policy` は
  `delta_px.signum()` を `direction` 相当として使う。バッファは一つ
- **分離**: アニメータ用 `ImpulseHistory` を別に持つ

共有が単純。`scroll_policy` の API (`count_in_window`, `last_gap`) は
`direction` 引数を取るままで内部実装だけ書き換える。両者は同じイベント列
を見ているという unification は意味的にも正しい (キーストロークは一つ)。

### 5.3 ExpDecay 系との API 一致

3 variant とも同じ `tick`/`current` API を出している現状の対称性は崩れる。
選択肢:

- 全 variant を `position_at(history, now)` API に揃える
  (ExpDecay 側は履歴を使わず target ベースで実装)
- enum 内部だけ流儀混在 (Kinetic は履歴駆動、他は target chase)、外側 API
  はそのまま

後者でも問題ないが、前者のほうが一貫性は高い。実装プラン時に判断。

### 5.4 既存テストの再構築

`scroll_animator.rs:646-849` の Kinetic テスト群 (`add_impulse`,
`set_landing`, frame-rate independence, snap 条件など) は新 API に
書き直しが必要。等価性は数学的に保証されるので、各テストは「同じ
入力シーケンスで同じ位置になる」形に再表現する。

---

## 6. スコープ外 / 後で考えるもの

- **ExpDecay 系の履歴駆動化**: 触らない (§3.6)
- **`scroll_policy` 自体のリファクタ**: 既に履歴駆動なので変更不要
- **新アニメータ追加**: この設計の上に追加する形で別タスク
- **viewer の他の永続状態**: アニメータ層に閉じる。`Viewport`、`Session`
  などは move-style のまま据え置き

---

## 7. 議論サマリ (次セッション用の前提)

1. **Kinetic は維持** (HCI 要請、削除なし)
2. **履歴駆動が優れる** (永続派生状態を持たないコードベース思想と整合、
   `scroll_policy` の戦訓と同じ)
3. **計算量は無視できる** (window 内 = 数十件)
4. **`InputHistory` に `delta_px` を加える** (記録時メモ化、(c) 案)
5. **Effect は `ScrollImpulse(i32)` / `ScrollAnchor(u32)` に縮約**
6. **`target_y` フィールドは導出関数化して廃止可能** (任意)
7. **anchor は履歴 prune 時に畳み込み**
8. **ExpDecay 系は今回触らない**

このドキュメントを基に次セッションで実装プランを起こす。

---

## 8. 実装結果 (2026-04-29)

このリファクタは `add-scroll-animation` ブランチ上で完了した。
プラン文書: `~/.claude/plans/docs-2026-04-29-design-history-driven-a-eventual-leaf.md`

着地点:

- `KineticParams` 純関数 (`scroll_animator.rs`):
  - `position_at(anchor, history, now) -> f64`
  - `velocity_at(history, now) -> f64`
- `ScrollAnimator` enum 統一 API:
  - `tick(anchor, history, viewport, now, dt) -> f64`
  - `is_animating(anchor, history, now) -> bool`
  - `current_position(anchor, history, now) -> f64`
  - `restart_ease_in_if_settled(settled)` (ExpDecay 専用)
- `ScrollState` (`layout.rs`):
  - `target_y: u32` を `anchor: f64` に置換
  - `derived_target(max_scroll) -> u32` ヘルパ
  - `InputHistory` を所有 (scroll_policy と animator が共有)
- `Effect` (`effect.rs`):
  - `ScrollTo(u32)` / `ScrollBy { target, impulse_px }` を撤去
  - `ScrollImpulse { delta_px, direction }` / `ScrollAnchor(u32)` に置換
- `scroll_policy.rs`:
  - classifier を **+1 投影** 方式に変更 (push を Effect 処理側に遅延)
- `viewport.rs::apply()`:
  - `ScrollImpulse`: history に push、evicted を anchor に畳み込み、
    ExpDecay の ease-in を再起動 (settled だった場合)
  - `ScrollAnchor`: 現在位置に anchor を pin、history flush、
    landing impulse を再 push (set_landing 互換)

ExpDecay 系の `current` フィールドは設計通り保持 (§3.6) だが、
tick API は `(anchor, history, ...)` に揃えた (§5.3)。

§4 の効果はすべて検証済:
- velocity 状態起因のバグクラスは構造的に存在しなくなった
- 履歴 dump で任意フレームを bit-exact 再現できる
- Effect の意図がそのまま型に出る
- 530 unit + 62 integration tests PASS、clippy zero warning
