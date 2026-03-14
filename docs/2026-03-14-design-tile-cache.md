# Content-Addressed Tile Cache

rebuild 時に変化しなかったタイルのキャッシュを保持し、再レンダリングを回避する仕組み。

## 背景

ビューワは reload/rebuild のたびにすべてのタイルキャッシュを破棄し、全タイルを再レンダリングしていた。1000行のドキュメントで1行だけ変更した場合でも、全タイルが再レンダリングされる。

## 設計

### TileHash — コンテンツ同一性

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct TileHash(u64);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct TilePairHash {
    pub content: TileHash,
    pub sidebar: TileHash,
}
```

`Frame` は typst が `#[derive(Hash)]` を実装しているため、`DefaultHasher` に直接渡せる。
Span も含まれるが、同じソーステキスト + 同じ `FileId`（`MluxWorld` では固定: `FileId::new(None, "main.typ")`）なら
`numberize()` が決定的に同じ Span を割り当てるため問題ない。

```rust
pub fn compute_tile_hash(frame: &Frame) -> TileHash {
    let mut h = DefaultHasher::new();
    frame.hash(&mut h);
    TileHash(h.finish())
}
```

### merge_tile_cache — standalone merge 関数

`TiledDocumentCache`（`HashMap<usize, TilePngs>`）をそのまま使い、standalone 関数でキャッシュを合流する。

```rust
pub fn merge_tile_cache(
    new_hashes: &[TilePairHash],
    old_hashes: &[TilePairHash],
    old_cache: &mut TiledDocumentCache,
) -> TiledDocumentCache
```

新旧ハッシュを線形探索で比較し、一致するタイルの PNG を old_cache から remove して new_cache に insert する。
5〜20 タイル程度なので線形探索で十分。

| シナリオ | 回収率 | 理由 |
|---------|--------|------|
| 1行編集（末尾） | ~95% | 編集箇所のタイルのみハッシュ変化 |
| 1行挿入（中央） | ~50% | 挿入点以降のタイル境界がずれる |
| テーマ変更 | 0% | 全タイルの描画が変わる |
| リサイズ | 0% | tile_height_px が変わり全ハッシュ変化 |
| 変更なし（保存のみ） | 100% | 全ハッシュ一致 |

### IPC プロトコル

`DocumentMeta` に `tile_hashes: Vec<TilePairHash>` フィールドを追加（`#[serde(default)]` で後方互換）。
子プロセスが `build_tiled_document()` 後にハッシュを計算して Meta に含める。

### Outer Loop の変更

```rust
let mut prev_cache: Option<TiledDocumentCache> = None;
let mut prev_hashes: Option<Vec<TilePairHash>> = None;

'outer: loop {
    let meta = renderer.wait_for_meta()?;

    let mut cache = match (prev_cache.take(), prev_hashes.take()) {
        (Some(mut old_cache), Some(old_hashes)) => {
            let c = merge_tile_cache(&meta.tile_hashes, &old_hashes, &mut old_cache);
            info!("merge: recovered {}/{} tiles", c.len(), meta.tile_count);
            c
        }
        _ => TiledDocumentCache::new(),
    };

    // inner loop uses `cache` ...

    match exit {
        Reload | Resize | ConfigReload => {
            prev_cache = Some(cache);
            prev_hashes = Some(meta.tile_hashes.clone());
        }
        Navigate | GoBack => { prev_cache = None; prev_hashes = None; }
        _ => break,
    }
}
```

## 変更ファイル

| ファイル | 変更内容 |
|---------|---------|
| `src/tile.rs` | `TileHash`, `TilePairHash`, `compute_tile_hash()`, `merge_tile_cache()` 追加。`DocumentMeta` に `tile_hashes` 追加。`TiledDocument::compute_tile_hashes()` 追加 |
| `src/viewer/mod.rs` | `prev_cache` + `prev_hashes` 導入、standalone `merge_tile_cache` で合流 |
| `src/viewer/state.rs` | `redraw`, `send_prefetch`, `ensure_loaded` が `TiledDocumentCache` を参照 |

## テスト

- **Unit**: `compute_tile_hash` — 同一 Frame → 同一ハッシュ、異なる Frame → 異なるハッシュ
- **Unit**: `merge_tile_cache` — 全一致 / 部分一致 / ゼロ一致 / evicted (cache に PNG なし)
- **Integration**: 同一入力の2回ビルドでハッシュ一致、末尾追記で部分回収、同一入力で全回収
- **手動**: `cargo run -- --log /tmp/mlux.log --watch doc.md` → "merge: recovered X/Y tiles"
