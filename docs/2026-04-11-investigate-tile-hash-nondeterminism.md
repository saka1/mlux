# 調査: 無変更リロードでタイルハッシュが1つ不一致になる

## 現象

BLAKE3 Span 除外ハッシュ導入後、ファイル無変更のリロードで `recovered 4/5`。
5/5 であるべき。

## ログ証拠 (tmp/mlux.log, README.md, release build)

```
初回:   tile: computed 5 tile hashes in 3.0ms → merge: recovered 0/5 (前世代なし)
リロード: tile: computed 5 tile hashes in 2.9ms → merge: recovered 4/5 ← ここ
```

- ページサイズ: 両ビルドとも `741.0x3937.3pt, 106 top-level items`
- タイル構成: 全タイルのアイテム数・boundary-spanning 数が完全一致
- Markdown 入力: `5053 bytes → 5221 bytes` (両ビルド同一)
- 画像ロード: 両ビルドとも `loaded 3 images, 1 errors`
- レンダリング済み PNG サイズ: tile 0 = 969478 bytes (両ビルド一致)

## 未特定の情報

- **どのタイル (0-4) のハッシュが不一致か** — ログにハッシュ値が出ていない
- **再現率** — 無変更リロードを複数回試していない (毎回起きるか不明)

## 仮説

### 仮説 1: Image 型の内部非決定性 (有力)

README は画像を3つ含む (`docs/ss.png`, `docs/gallery01.png`, `docs/gallery02.png`)。
typst の `Image` は `Arc<LazyHash<Repr>>` で、`Hash` は `LazyHash` 経由で
内部の u128 キャッシュを `write_u128()` する。

`LazyHash::hash_item()` は `TypeId` + `SipHasher13` で計算するため、
同じバイト列からは決定的なはず。ただし:

- `RasterImage` / `SvgImage` のデコード・内部表現に非決定性がある可能性
- 画像の再ロード時に内部キャッシュの状態が異なる可能性

画像を含むタイルと不一致タイルが一致すれば裏付けられる。

### 仮説 2: typst コンパイラの内部非決定性

typst のレイアウトエンジンが内部で HashMap を使い、走査順序の違いが
Frame tree の構造に影響する可能性。ただしアイテム数が完全一致しているため
可能性は低い (順序が変われば split_frame の結果も変わるはず)。

### 仮説 3: フォント shaping の非決定性

フォントの shaping エンジン (rustybuzz) が内部状態に依存して
微妙に異なる Glyph 配置を返す可能性。ただし同一入力・同一フォントで
非決定的になるのは考えにくい。

## 実害

- 無変更リロードで1タイル分の不要な PNG 再レンダリング + Kitty 転送が発生
- 体感への影響は小さい (1タイル分の追加コスト)
- アプリの挙動は正しい (古い画像が表示されるわけではない)

## 結論 (2026-04-11)

**ハッシュ非決定性ではなかった。** 全5タイルのハッシュ値はリロード前後で完全一致。

`recovered 4/5` (または 3/5) の原因は、リロード前にレンダリングされていないタイルが
キャッシュに PNG を持っていなかったため。`merge_generation()` はキャッシュに PNG が
存在するタイルしかリカバリできない。lazy rendering の正常動作。

### 証拠 (debug ログ)

```
初回:   tile: hash[0..4] = fc89...  2031...  833c...  e05b...  8fce...
リロード: tile: hash[0..4] = fc89...  2031...  833c...  e05b...  8fce...  ← 完全一致
```

レンダリング済みタイル: 0 (viewport), 1, 2 (prefetch) → recovered 3/5

### 対応

- `merge` ログメッセージを改善: `recovered 3/5 tiles (0 changed, 2 not yet rendered)`
- 各タイルのハッシュ値を debug ログに追加 (将来の調査用)
