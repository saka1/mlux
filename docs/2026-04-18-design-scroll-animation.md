i# mlux 滑らかスクロール設計ドキュメント

TUI Markdown ビューア mlux における、スクロール目標地点到達までのアニメーション設計。Claude Code との実装議論のための事実調査と設計方針のまとめ。

## 0. 問題設定とアーキテクチャ上の切り分け

スクロール体験は以下の 2 層に分離できる。本ドキュメントは **下流層** のみを扱う。上流層は既に別途議論済みで、以下を前提とする。

**上流層(本ドキュメントの範囲外)**
- キー入力から意図を推論する層。EWMA ベースのキーストローク速度推定、velocity-dependent gain など。
- アウトプット: 更新された `target: f64`(スクロール目標地点、ピクセル単位)

**下流層(本ドキュメントの範囲)**
- 現在位置 `current` を `target` に向けて、知覚的に自然な動きで追従させる層。
- 入力: 時々刻々変化する `target`、出力: 毎フレームの `current`。

この分離は設計上もテスト上も筋が良い。上流は離散イベント駆動(入力列 → target 列)、下流は連続時間ステートマシン(target 列 → フレーム列)としてテストできる。両者の接点は `target` の 1 値のみ。

---

## 1. TUI 実装の事実調査: Kitty Graphics Protocol

### サブセル精度のピクセル配置は仕様範囲内で可能

mlux のタイル画像は通常セル境界に配置されるが、**セル内ピクセルオフセット** を指定する仕様が存在する。

- 配置コマンド (`a=p`) で `X`, `Y` キーによりセル内ピクセルオフセットを指定可能
- 制約: `X < cell_width`, `Y < cell_height`(セルサイズ未満)
- 公式仕様: https://sw.kovidgoyal.net/kitty/graphics-protocol/ (Cursor movement policy セクション周辺)
- GitHub 上のソース: https://github.com/kovidgoyal/kitty/blob/master/docs/graphics-protocol.rst

### スクロール実装への適用

縦スクロール量 `scroll_px` を持たせ、各タイル配置時に以下の分解を行う:

```
cell_row  = (tile_y - scroll_px) / cell_height   // 整数除算
Y_offset  = (tile_y - scroll_px) % cell_height   // 余り → Y キー
```

累積スクロールが `cell_height` を跨ぐたびに `cell_row` が 1 ずれ、`Y_offset` が 0 にラップする。視覚的には連続的なピクセル移動。

### 実装上の留意点

- **画像データ再送不要**: `i=<id>` で保持された画像に対し配置コマンド (`a=p`) のみ再発行。これが 60fps 可能性の前提。
- **旧配置の削除**: placement id で管理し、`a=d,d=i,i=<id>,p=<pid>` で古い配置を削除しないと残像。
- **ちらつき**: kitty 本家は frame coalescing されるが、WezTerm/Ghostty 等は実装差がある。`z` 負値レイヤーなどで対策可能。
- **互換性**: `X`/`Y` オフセットは主要実装(kitty, WezTerm, Ghostty)では対応。xterm.js も最近実装(https://github.com/xtermjs/xterm.js/discussions/5683 参照)。
- **入力ドレインとの相性**: tick ベース設計と相性が良い。1 tick で `scroll_px` を少量加算 → 再配置。

### 採用されなかった代替案

- **Unicode placeholder 経由**: テキストスクロールに追従するがセル単位粒度のみ。サブセル不可。
- **相対配置 (`P`, `Q`, `H`, `V`)**: 親配置からのオフセットだがセル単位。
- **アニメーションフレーム (`a=a`)**: 単一画像の差し替え用。タイル構成の全体スクロールには不適。

---

## 2. GUI 実装の事実調査

ブラウザのスクロール実装は「指数減衰」一択ではない。大きく 2 系統ある。

### 2.1 Chrome デフォルト: 固定時間 cubic-bezier

- 入力 → 一定量の `target` を加算(マウスホイール 1 tick = 100px、キーボード矢印 = 40px など固定 delta)
- `target` まで事前に決まった duration で cubic-bezier カーブに従って到達
- CSS `scroll-behavior: smooth` も同様
- 参考: https://blogs.windows.com/msedgedev/2020/04/02/scrolling-personality-improvements/

**各入力ごとに独立したパラメトリックアニメーション**を走らせる。途中で新入力が来ればキャンセル/再スタート(ScrollAnimator の重ね掛けロジック)。

### 2.2 Edge impulse モード: 物理ベース

- EdgeHTML 由来。ホイール 1 tick ごとに「速い初速 (impulse) → 摩擦で減速」
- Chromium にポートされ、`chrome://flags/#impulse-scroll-animations` で有効化可能(過去)
- 現在は Edge でマウスホイール・キーボード・スクロールバー・タッチフリング全てに適用
- 摩擦項のある物理モデル(指数減衰に近い)
- ソース実装例: https://chromium.googlesource.com/chromium/src/+/refs/heads/main/ui/events/gestures/physics_based_fling_curve.h

### 2.3 なぜ Chrome は Bezier + 固定時間を選んだか

- **終了時刻が予測可能**(指数減衰は漸近的なのでスナップ判定が要る)
- **CSS タイミング関数と整合**(transition と同じ数学モデル)
- **離散入力に向く**(1 クリック = 1 アニメの単発モデル)
- **テスタビリティ**(入力 → 出力が決定論的)

### 2.4 mlux への含意

mlux の上流層は「`target` を連続更新する」モデル(j 連打や velocity-gain で target が育つ)。この場合:

- **Bezier + 固定時間**: 入力ごとに「古いアニメをキャンセルして新しいアニメを開始」となり、連打時に duration リセットによる速度不連続が起きやすい。
- **指数減衰(またはクリティカルダンプのばね)**: `target` だけ更新し `current`/`velocity` は持ち越されるので、連続入力との相性が構造的に良い。

**結論**: mlux の連続 target 更新モデルには指数減衰が適合的。Edge の impulse モードの方向性に近い。

---

## 3. 補間モデルの詳細(採用候補: 指数減衰)

### 3.1 数学形式

```
current = target + (current - target) * exp(-λ * dt)
```

half-life(残距離が半分になる時間)ベースで書くと直感的:

```
α = 1 - 0.5^(dt / half_life)
current += (target - current) * α
```

推奨値: `half_life = 80〜150ms`。オーバーシュート無しで読み物のスクロールに適する。

### 3.2 フレームレート独立は必須

NG 例(60fps 固定前提):
```
current += (target - current) * 0.1
```

`exp(-λ*dt)` 形式は連続時間の解で任意 dt で正しい。120Hz/30fps/可変 fps いずれでも破綻しない。

### 3.3 入力の連続性保持

j 連打や PgDn → j のように進行中に新入力が来るケース:
- `target` のみ更新し、`current` と `velocity` は維持
- 指数減衰は状態が `current` のみなので自然に連続
- スプリングダンパで作る場合は `velocity` を明示的に持ち越す

### 3.4 距離適応

PgDn と `gg`(先頭へ)を同じカーブで動かすと遠距離が終わらないか目が追えない。

- 固定時間: 近距離滑らか、遠距離速すぎ
- 線形 `T ∝ d`: 近距離が遅い
- **サブ線形 `T ∝ √d` or `log(d)`**: 実用バランスが良い(Stevens の冪則に整合)

指数減衰なら距離に応じて `half_life` を動的に少し伸ばす。

### 3.5 停止条件

指数減衰は漸近的に永遠に追いかけるので、実装では閾値で打ち切る:

- `|target - current| < 0.5px` でスナップし `velocity = 0`
- さらに速度ベースで `|Δcurrent/Δt| < 0.1°/s 相当` で停止

mlux は描画コストが重いので、アイドル時の微小再描画回避は CPU/バッテリー上重要。

### 3.6 スプリングダンパは?

より豊かな挙動が作れるがオーバーシュートしうる。読書ビューアではオーバーシュートは邪魔。クリティカルダンプか指数減衰で十分。

---

## 4. 知覚研究のエビデンス

mlux は「読む」ツールなので、眼球運動・読字研究が直接適用できる。以下、一次文献ベース。

### 4.1 視覚の物理限界

- **Smooth pursuit(滑動性追従眼球運動)の追跡可能速度**: 約 30–40°/s まで
  - Grokipedia レビュー: https://grokipedia.com/page/Smooth_pursuit
- **視力劣化の閾値**: 網膜速度 **2.5°/s** を超えると空間視力が急激に低下
  - Westheimer & McKee (1975)、Valsecchi et al. (2013) で引用: https://jov.arvojournals.org/article.aspx?articleid=2121460

**mlux への適用**: 視距離 50cm、行高 20px ≒ 0.4° として、連続読み時のスクロール速度上限は **約 6 行/秒**(視野角 2.5°/s 相当)。これを超えるなら読ませるのを諦める設計にする。

### 4.2 スクロール読字の眼球運動モデル

- **ドリフトテキスト読字では fixation が smooth pursuit に置き換わる**
  - Valsecchi, Gegenfurtner, & Schütz (2013) "Saccadic and smooth-pursuit eye movements during reading of drifting texts", *Journal of Vision* 13(10):8. DOI: 10.1167/13.10.8
  - https://jov.arvojournals.org/article.aspx?articleid=2121460
  - PubMed: https://pubmed.ncbi.nlm.nih.gov/23956456/
- **サッカード後の pursuit gain 増加**(8.5%)、水平ドリフト時のサッカード ピーク速度低下

**含意**: スクロール中は読みにくい。→ 減速時の最後の一押しを長めに取り、停止状態を確保する。`half_life` を距離に応じて変える方針の裏付け。

### 4.3 Smooth pursuit の立ち上がり遅延

- 開始潜時: **100–200ms**
- 維持中の神経処理遅延: **67ms**
- Schütz, Braun, & Gegenfurtner (2011) "Eye movements and perception: a selective review", *Journal of Vision* 11. https://pubmed.ncbi.nlm.nih.gov/21917784/
- Gegenfurtner (2016) "The Interaction Between Vision and Eye Movements", *Perception* 45. https://journals.sagepub.com/doi/pdf/10.1177/0301006616657097

**含意**: 急激な動き出しは不自然。**100ms 程度の ramp-up (ease-in)** を入れると追従性が上がる。純粋指数減衰は ease-in を持たないので、初速をゼロから立ち上げる補正が有効。

### 4.4 読書アンカー保存(Gaze-enhanced scrolling)

- Stanford HCI: ページスクロール直前に視線位置にマーカーを描画し、マーカーと共にスクロール → 視線が自然に追従し読み位置再取得コストを下げる
- https://hci.stanford.edu/cstr/reports/2007-11.pdf

**mlux への含意**: 大ジャンプ(PgDn, Ctrl-D)でも瞬間移動せず、1 フレームでもアニメを挟むと視線追従できる。

### 4.5 空間記憶・読解への影響

- スクロール形式は inference 系質問の回答を困難にする(空間マップ喪失)
- Kerzel & Ziegler (2005) "Visual Short-Term Memory During Smooth Pursuit Eye Movements"

**含意**: 位置インジケータ/minimap は知覚的自然さとは別軸で読解を支える。

### 4.6 SDAZ(Speed-Dependent Automatic Zooming)

- Igarashi & Hinckley (2000) "Speed-dependent automatic zooming for browsing large documents", UIST 2000, pp. 139-148. DOI: 10.1145/354401.354435
- https://kenhinckley.wordpress.com/2000/11/05/paper-speed-dependent-automatic-zooming-for-browsing-large-document/
- Cockburn & Savage (2003) での追試: https://link.springer.com/chapter/10.1007/978-1-4471-3754-2_6

速度が上がるほど自動ズームアウトして網膜速度を一定に保つ。TUI で厳密な zoom は困難だが、**高速スクロール中に本文をフェードさせて見出しだけ残す** 等の情報密度動的変更で近似可能。

### 4.7 Stevens の冪則(心理物理学)

- 知覚速度は物理速度と線形でなく冪則(指数 0.6–0.8)
- 「倍速く感じる」には物理的に 2.3 倍必要
- 遠距離ジャンプの時間設計 `T ∝ √d` の心理物理学的根拠

### 4.8 動きの自然さの一般原則

- **ジャーク(加速度変化)の連続性**: 人間は速度より加速度変化に敏感。C∞ カーブが自然、区分線形は不自然。
- **最小知覚速度**: 約 0.1–0.2°/s。これ以下は「動いているが気づかない」。スナップ閾値に利用可。
- **motion induced blindness / change blindness**: 動作中の細部は見えない前提で、高速時は描画簡略化可(画像解像度を下げるなど)。

---

## 5. mlux への推奨設計

上記知見を踏まえた、下流補間層の推奨仕様。

### 5.1 コア補間: 指数減衰 + 初期 ramp-up

```
current = target + (current - target) * exp(-λ(d, t) * dt)
```

ただし:
- 新規入力到着から 100ms は λ を小さくして ease-in 特性を作る(pursuit 立ち上がり潜時に整合)
- それ以降は通常の減衰

### 5.2 パラメータ推奨初期値

| パラメータ | 推奨値 | 根拠 |
|---|---|---|
| 基本 half_life | 120ms | UI 心地よさの経験値 |
| ramp-up 時間 | 100ms | pursuit 潜時 |
| 距離スケーリング | `half_life × (1 + log(1 + d/viewport))` | サブ線形、Stevens 則 |
| 連続読み上限速度 | 視野角 2.5°/s | Westheimer & McKee (1975) |
| スナップ位置閾値 | 0.5px | ピクセル量子化 |
| スナップ速度閾値 | 0.1°/s 相当 | 最小知覚速度 |

### 5.3 運用レイヤ

- **位置インジケータ**: 右端にスクロールバー相当の視覚要素(空間記憶補助)
- **大ジャンプ**: 瞬間移動せず、最短でも 1 フレーム分のアニメを挟む(視線追従)
- **減速末尾の延長**: 停止付近で half_life を短くし、ピタッと止まる感触(ただし反応性は保つ)
- **高速時の描画簡略化**: 画像解像度を落とす、本文をフェードして見出しを残す(SDAZ 近似)

### 5.4 tick ベース入力ドレインとの統合

1 tick の処理:
1. 入力キューをドレインし `target` を更新
2. 指数減衰で `current` を `target` へ寄せる
3. `current` が変化していれば再描画、十分近ければスナップして停止

入力が無い tick でも(2)が走ることで「キーを離した後に少し続いて止まる」慣性感。
速度 `|Δcurrent/Δt|` はそのまま prefetch 強度や W-TinyLFU の重みに流用可能。

---

## 6. 参考文献まとめ

### 技術仕様

- Kitty Graphics Protocol 公式仕様: https://sw.kovidgoyal.net/kitty/graphics-protocol/
- Kitty Graphics Protocol GitHub: https://github.com/kovidgoyal/kitty/blob/master/docs/graphics-protocol.rst
- xterm.js Kitty Graphics 議論: https://github.com/xtermjs/xterm.js/discussions/5683
- Microsoft Edge Scrolling Improvements: https://blogs.windows.com/msedgedev/2020/04/02/scrolling-personality-improvements/
- Chromium physics_based_fling_curve: https://chromium.googlesource.com/chromium/src/+/refs/heads/main/ui/events/gestures/physics_based_fling_curve.h
- CSS cubic-bezier: https://css-tricks.com/almanac/functions/c/cubic-bezier/

### 学術論文(眼球運動・読字)

- Valsecchi, M., Gegenfurtner, K. R., & Schütz, A. C. (2013). Saccadic and smooth-pursuit eye movements during reading of drifting texts. *Journal of Vision*, 13(10):8. DOI: 10.1167/13.10.8
  - https://jov.arvojournals.org/article.aspx?articleid=2121460
  - PubMed: https://pubmed.ncbi.nlm.nih.gov/23956456/
- Westheimer, G., & McKee, S. P. (1975). Visual acuity in the presence of retinal-image motion. (2.5°/s 視力劣化閾値の原典)
- Schütz, A. C., Braun, D. I., & Gegenfurtner, K. R. (2011). Eye movements and perception: a selective review. *Journal of Vision*, 11.
  - https://pubmed.ncbi.nlm.nih.gov/21917784/
- Gegenfurtner, K. R. (2016). The Interaction Between Vision and Eye Movements. *Perception*, 45(12).
  - https://journals.sagepub.com/doi/pdf/10.1177/0301006616657097
- Kerzel, D., & Ziegler, N. E. (2005). Visual Short-Term Memory During Smooth Pursuit Eye Movements.
- Rayner, K. (1998). Eye movements in reading and information processing: 20 years of research.

### 学術論文(HCI / スクロール)

- Igarashi, T., & Hinckley, K. (2000). Speed-dependent automatic zooming for browsing large documents. UIST 2000, pp. 139-148. DOI: 10.1145/354401.354435
  - https://kenhinckley.wordpress.com/2000/11/05/paper-speed-dependent-automatic-zooming-for-browsing-large-document/
- Cockburn, A., Savage, J., & Wallace, A. (2005). Tuning and Testing Scrolling Interfaces that Automatically Zoom.
  - https://link.springer.com/chapter/10.1007/978-1-4471-3754-2_6
- Stanford HCI: Gaze-enhanced Scrolling Techniques.
  - https://hci.stanford.edu/cstr/reports/2007-11.pdf
- Harvey, H., & Walker, R. (2014). Reading with peripheral vision: (central vision loss and scrolling text).

### 心理物理学

- Stevens, S. S. (1957). On the psychophysical law. (冪則原典)

