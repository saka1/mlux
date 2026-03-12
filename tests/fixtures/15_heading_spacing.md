# 見出しレベル1：文書のタイトル

この段落はh1の直後に配置されている。見出しの下余白（below）がどの程度あるかを確認するための本文である。段落が十分な長さを持つことで、余白の視覚的な印象が正確に伝わる。Rustの型システムは安全性と表現力を両立させるために設計されており、所有権・借用・ライフタイムの三つの概念が中核をなしている。

この段落はh2の直前に配置されている。見出しの上余白（above）がどの程度あるかを確認するための本文である。前のセクションとの視覚的な分離が適切かどうかを見極めるには、ある程度の長さの段落が必要になる。

## 見出しレベル2：セクションの導入

この段落はh2の直後に配置されている。レベル2の見出しは文書構造の主要な区切りとして機能する。フォントサイズの違いによって、上下の余白が視覚的にどう変化するかを観察できる。コンパイラが型の整合性を静的に検証することで、実行時エラーの多くを未然に防いでいる。

この段落はh3の直前に配置されている。レベル2からレベル3への遷移は、文書の階層構造において最も頻繁に現れるパターンである。余白の差が階層の違いを適切に表現しているかを確認する。

### 見出しレベル3：サブセクションの開始

この段落はh3の直後に配置されている。レベル3は具体的なトピックの説明に使われることが多い。所有権システムにおいて、各値は唯一の所有者を持ち、所有者がスコープを離れると値は自動的にドロップされる。この仕組みによりガベージコレクタなしでメモリ安全性を実現している。

この段落はh4の直前に配置されている。レベル3からレベル4への遷移は、詳細な説明が続く場面で使用される。見出しの階層が深くなるにつれて、余白がどのように変化するかを確認する。

#### 見出しレベル4：詳細項目

この段落はh4の直後に配置されている。レベル4の見出しは比較的小さなフォントサイズになる。借用規則では、不変参照は複数同時に存在できるが、可変参照は一つだけしか存在できないという制約が課される。

この段落はh5の直前に配置されている。レベル4からレベル5へ移行する際に、上余白の減少が適切なグラデーションになっているかを確認する。

##### 見出しレベル5：補足的な項目

この段落はh5の直後に配置されている。レベル5の見出しは本文に近いサイズとなる。ライフタイムパラメータは参照の有効期間をコンパイラに伝えるための注釈であり、実行時のオーバーヘッドは一切発生しない。

この段落はh6の直前に配置されている。最も深い見出しレベルへの遷移において、余白による階層の区別がまだ視認できるかを確認する。

###### 見出しレベル6：最も深い見出し

この段落はh6の直後に配置されている。レベル6は本文と同じフォントサイズになるため、余白だけが見出しと本文の区別を補助する重要な要素となる。トレイト境界を組み合わせることで、ジェネリクスに対して必要な振る舞いをコンパイル時に保証できる。

---

この段落は水平線の後に配置されている。すべての見出しレベルを通過した後の本文として、文書全体の余白バランスを俯瞰的に確認するための参考になる。

## 連続する同レベルの見出し

この段落はh2の直後にある。次にもう一つh2が続く。同レベルの見出しが連続する場合に、above同士の間隔が適切かを確認する。

## 二つ目のレベル2見出し

この段落はh2の直後にある。同じレベルの見出しが続いた場合でも、セクション間の分離が十分に感じられるかを確認する。

### 連続する見出し（レベル3）

直下にもう一つのh3が続く。

### 二つ目のレベル3見出し

この段落はh3の直後にある。レベル3が連続する場面は技術文書で頻出するため、この間隔の確認は重要である。

---

# Heading Level 1: Document Title

This paragraph is placed immediately after h1. It serves to verify the amount of below-spacing beneath the heading. A paragraph of sufficient length is needed so that the visual impression of the margin is conveyed accurately. Rust's type system is designed to balance safety and expressiveness, with ownership, borrowing, and lifetimes forming its three core concepts.

This paragraph is placed immediately before h2. It serves to verify the amount of above-spacing before the heading. Adequate paragraph length is necessary to judge whether the visual separation from the previous section is appropriate.

## Heading Level 2: Section Introduction

This paragraph is placed immediately after h2. Level 2 headings function as the primary structural divisions of a document. By varying the font size, one can observe how the vertical spacing changes visually. The compiler statically verifies type consistency, preventing many runtime errors before they occur.

This paragraph is placed immediately before h3. The transition from level 2 to level 3 is the most frequently occurring pattern in document hierarchy. The goal is to confirm whether the margin difference adequately expresses the distinction between levels.

### Heading Level 3: Subsection Start

This paragraph is placed immediately after h3. Level 3 is often used for describing specific topics. In the ownership system, each value has exactly one owner, and when the owner goes out of scope the value is automatically dropped. This mechanism achieves memory safety without a garbage collector.

This paragraph is placed immediately before h4. The transition from level 3 to level 4 is used in scenarios where detailed explanations follow. As the heading hierarchy deepens, the objective is to verify how the margins change accordingly.

#### Heading Level 4: Detailed Item

This paragraph is placed immediately after h4. Level 4 headings use a relatively small font size. The borrowing rules impose the constraint that multiple immutable references can exist simultaneously, but only one mutable reference is allowed at a time.

This paragraph is placed immediately before h5. When transitioning from level 4 to level 5, the goal is to verify that the decrease in above-spacing forms an appropriate gradient.

##### Heading Level 5: Supplementary Item

This paragraph is placed immediately after h5. Level 5 headings are close in size to the body text. Lifetime parameters are annotations that communicate the validity period of references to the compiler, incurring no runtime overhead whatsoever.

This paragraph is placed immediately before h6. At the transition to the deepest heading level, the goal is to confirm whether the hierarchy is still visually distinguishable through spacing alone.

###### Heading Level 6: Deepest Heading

This paragraph is placed immediately after h6. At level 6 the font size matches the body text, making spacing the sole element that helps distinguish headings from regular paragraphs. By combining trait bounds, one can guarantee at compile time that generics exhibit the required behavior.

---

This paragraph is placed after the horizontal rule. Having passed through all heading levels, it serves as a reference for evaluating the overall margin balance of the document.

## Consecutive Same-Level Headings

This paragraph is placed after h2. Another h2 follows immediately. The goal is to verify whether the interval between consecutive same-level above-spacings is appropriate.

## Second Level 2 Heading

This paragraph is placed after h2. Even when headings of the same level appear consecutively, the separation between sections should feel sufficient.

### Consecutive Headings (Level 3)

Another h3 follows immediately below.

### Second Level 3 Heading

This paragraph is placed after h3. Consecutive level 3 headings are common in technical documents, making this spacing check particularly important.
