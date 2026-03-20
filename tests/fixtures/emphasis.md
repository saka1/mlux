# Emphasis Fixture

## Basic Emphasis

This is *italic text* in a sentence.

This is **bold text** in a sentence.

This is ***bold italic text*** in a sentence.

## Japanese Only

*日本語の斜体テスト*

**日本語の太字テスト**

***日本語の太字斜体テスト***

ここで*強調*を使う。文中の*一部分だけ*を強調する例。

## Latin Only

*This is emphasized text* in English.

**This is strong text** in English.

***This is strong + emphasized*** in English.

## Mixed CJK / Latin

*日本語とEnglishの混在emphasis*

**太字の中にLatinが混ざる**

***太字斜体Bold Italicの混在***

ここでいう*state*とは、アプリケーションの状態を指す。

*Rustのライフタイム*は重要な概念です。

## Emphasis Inside Headings

### *Italic* in h3

### **Bold** in h3

### ***Bold Italic*** in h3

### *見出しの中の斜体*

## Emphasis in Block Quotes

> *引用の中の斜体テスト*
>
> **引用の中の太字テスト**
>
> This is *emphasis* inside a quote.

## Emphasis in Lists

- *リスト項目の斜体*
- **リスト項目の太字**
- ***リスト項目の太字斜体***
- Normal text with *some emphasis* inside
- 通常テキストの中に*強調部分*がある

1. *番号付きリストの斜体*
2. **番号付きリストの太字**
3. 通常テキストに*部分的な強調*を含む

## Emphasis with Inline Code and Links

*emphasis with `inline code` inside*

*emphasis with [a link](https://example.com) inside*

**bold with `inline code` inside**

## Long Emphasis (Line Wrapping)

*これは非常に長い強調テキストです。行の折り返しが発生するかどうかを確認するために、十分な長さのテキストを用意しています。圏点が複数行にまたがる場合の挙動を観察するためのテストケースです。*

*This is a very long emphasized text that should wrap across multiple lines to test how emphasis markers behave when they span across line boundaries in the rendered output.*

## Nested Structures

> - *引用の中のリストの中の斜体*
> - **引用の中のリストの中の太字**

- > *リストの中の引用の中の斜体*

## Adjacent Emphasis

*italic* and **bold** and ***bold italic*** on the same line.

*斜体*と**太字**と***太字斜体***が同じ行にある。

## Edge Cases

*a* single character emphasis

*あ* single CJK character emphasis

Word*内部*emphasis (no spaces)

*句読点を含む強調。*

*Emphasis ending with punctuation.*

~~*strikethrough and italic*~~

**Bold with *nested italic* inside**

*Italic with **nested bold** inside*
