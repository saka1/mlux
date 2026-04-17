# スクロール性能調査：KGP構造の限界と実ボトルネック

日付: 2026-04-17
対象: `main` ブランチ時点のビューア
目的: ビューアのスクロール性能を改善するとしたら、どこに手を入れるのが本質的かを整理する。

## KGP の「スクロール」とは実質何をやっているか

Kitty Graphics Protocol の画像表示は **upload (`a=T` / `a=t`)** と **placement (`a=p`)** の二段構え。両者のコスト差が大きい。

| 操作 | コスト | 頻度 | 備考 |
|---|---|---|---|
| PNG upload | 重い（KB〜MB転送 + 端末側デコード） | タイル初回のみ | mlux は `t=t` file transfer で PTY 迂回済み |
| Placement `a=p i=N x=.. y=.. w=.. h=..` | 極軽（数百B + 端末側コンポジット） | スクロール毎フレーム | **source-rect で画像の窓を切り替えるだけ** |
| Atomic in-place move（同じ `(i,p)` 再発行） | 極軽、無フリッカ | 理屈上は毎フレーム可 | mainでは未活用（後述） |
| Delete placement `a=d,d=i,i=,p=` | 極軽 | 可視範囲外スロットのみ | 画像本体は残す |
| Delete image `a=d,d=i,i=` | 軽 | evict 時 | 端末側メモリ解放 |

画像が端末側に常駐していて source-rect の `y` を毎フレーム動かす限り、**KGP 通信コストはスクロール速度の上限をほぼ決めない**。1フレームあたり数百バイト、`a=p` 2〜4本。

main が既にやっていること：

- `render_png.rs` で `png::Compression::Fastest`（viewer 経路）
- `t=t` file transfer（PTY バイパス）
- source-rect による窓切替（再 upload なし）
- 可視外タイルの eviction（`evict_distance = 4`）
- タイル hash ベースのリビルド間キャッシュ併合（BLAKE3）

main がまだやっていない KGP 活用：

- `(i,p)` の安定割り当てによる atomic in-place move。現状は `a=p` を毎フレーム発行する前に可視スロットへ delete-all 相当を行うため、原理上は一瞬の blink 余地がある。ただしこれは**描画品質（flicker）の問題であって、スクロールの throughput/latency ではない**。

**結論：KGP プロトコル層でスクロール性能をさらに絞り出せる余地はほぼ無い。**

## 実際の遅延発生地点

### 1. `ensure_loaded` がメインループを同期ブロックする（最大要因）

`src/viewer/display_state.rs:161-...`、コア部分 L172 付近：

```rust
while !cache.contains(idx) {
    match rh.renderer.recv()? { ... }   // blocking
}
```

prefetch 外のタイルに入った瞬間、UI 全体が child の応答まで凍結する。しかも child のキューに先行タスクがあれば、それらが終わるまで待たされる（child は単一ループ、`src/renderer.rs:303-339` の request loop が `RenderTile` を1件ずつ直列処理）。これが体感上の「カクつき」の正体。

### 2. Child renderer は完全に直列

`src/renderer.rs:303-339`: 単一 child プロセスが `RenderTile` を1リクエストずつ処理。`typst_render::render` 自体はスレッドセーフ（`src/frame/tile.rs:445-447` のコメント参照）なので原理的に並列化できるが、現状は利用していない。タイル3枚分スクロールすると `3 × render_tile_pair_time` 待たされる。

### 3. Prefetch が弱い

`src/viewer/display_state.rs:381-` の `send_prefetch`：`current+1, current+2, current-1` の3枚のみ、**方向無視**。

一方 `src/viewer/scroll_policy.rs` には方向（`ScrollDirection`）・速度（Normal/Mid/High）のトラッキングと倍率制御が既に存在する。高速スクロール中こそ前方を深く・後方を薄くすべきだが、prefetch 側は一切参照していない。

### 4. 再描画の起動粒度が粗い

- `frame_budget = 32ms`（`src/config.rs:54`）→ 上限 ~30fps。KGP ではなく mlux 側の設計選択。
- `vp.dirty` フラグ1本しかなく、dirty の理由が分からない → `redraw_and_prefetch` (`display_state.rs:484`) は常に全フェーズ走る（可視タイル確認 → place × 2 → overlay → status bar → prefetch）。flash 消去や acc_peek 更新だけでも全経路が動く。
- 毎フレームの `a=p` 再発行に先行して可視スロットの delete が挟まるため、描画品質面で blink 余地がある（これは KGP 的には `(i,p)` atomic move で解決可能）。

### 5. 新タイル到着を即座に画面へ反映する経路がない

child が新タイルを返しても、反映されるのは次の redraw 起動時まで（`drain_responses` が redraw 冒頭で呼ばれる）。イベントが来なければ `event::poll(watch_interval = 200ms)`（`src/config.rs:58`）で待つ。**タイル描画完了 → 画面反映に最大 200ms の遅延**。child 応答 fd を `event::poll` と多重化する仕組みが無い。

## 改善の優先順

| 改善項目 | 想定効果 | KGP関係 |
|---|---|---|
| `ensure_loaded` の非同期化（キャッシュ未ヒット時は旧フレーム継続、タイル到着で再 redraw） | 大：体感カクつきがほぼ消える | なし |
| Prefetch を方向・速度ベースに（`ScrollStrategy` 参照、前方を深く取る） | 大 | なし |
| Child renderer の並列化（worker プール or 追加 child） | 中〜大（連続スクロールで効く） | なし |
| タイル到着イベントを main loop に配送（child fd を `event::poll` と多重化） | 中（到着→反映の遅延解消） | なし |
| dirty フラグ細分化（ScrollDelta / FlashOnly / StatusOnly / OverlayOnly）→ 部分再描画 | 小〜中 | なし |
| `(i,p)` 安定化による atomic in-place move（flicker 解消） | 品質改善、スクロール速度は不変 | あり |
| KGP プロトコル面でのスクロール速度改善 | ほぼ0 | 現状でほぼ理論限界 |

## 結論

問いは **「KGP のどこをいじればスクロールが速くなるか」ではなく「KGP に叩き込む直前で詰まっているタイルを、どうメインループを止めずに用意しておくか」**。

KGP の emit 層は既に軽く、ここを磨いても体感速度は動かない。詰まっている層は：

- child renderer のシリアル性
- `ensure_loaded` の同期ブロック
- 方向無視の prefetch
- タイル到着と redraw の非同期結合欠如

これらはいずれも KGP の外側、ビューア側の I/O・並列化・スケジューリング設計の話。

次の実作業候補（独立した変更として進められる）：

1. `ensure_loaded` の非同期化
2. 方向性 prefetch（`ScrollStrategy` 連携）
3. Child renderer 並列化
4. （品質改善として別枠）`(i,p)` 安定化による atomic in-place move 化

1〜3 はスクロール性能に直接効く。4 は flicker 解消の品質改善であり、性能改善と混同して同一ブランチに詰めると評価が歪む。
