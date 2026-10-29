# deltaflens

## 概要

JPEG XL が主役の、探す・並べる・見るが 1 つで完結する画像ビューワー。Rust + gpui で作る。Linux を最優先にする。

## 全体の構造

ウィンドウ → レイアウト（分割の木）→ 枠（Pane）→ タブ → 中身（エクスプローラーかビューワー）。

Zed の `Pane` / `PaneGroup` と同じ考え方。

## 設計の原則

- UI に依存しない部分（デコード、フォルダの走査、並べ替え、レイアウトの木など）は gpui に依存させず、UI と分ける。
- この部分にはユニットテストを書く。
- UI に依存しない部分は `deltaflens-core` に置く。gpui への依存はクレートを分けることでコンパイラーに守らせる。

## gpui について

- gpui は 1.0 前で API がよく変わり、AI の知識は古いことが多い。
- 推測で書かない。実装の前に `~/.cargo/git/checkouts/` にある gpui のソースや、Zed 本体、gpui のサンプル（`crates/gpui/examples/`）を読んで、実在する API を確かめる。

## 作業を終える前に

次をすべて通す。

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## AVIF

AVIF は Cargo の feature `avif` を有効にしたときだけ開ける。デコードには dav1d を使うので、ビルドする環境に dav1d の開発用のパッケージを入れる（pkg-config で見つかる必要がある）。

- Debian / Ubuntu: `libdav1d-dev`
- Arch: `dav1d`

```sh
cargo run --features avif
cargo test --workspace --features avif
```

既定の feature では、システムのライブラリなしでビルドでき、AVIF は対応形式に入らない（エクスプローラーにも ←→ の一覧にも出ない）。

## コードの約束

- パスは `String` ではなく `PathBuf` / `&Path` で持つ。
- 拡張子の判定は大文字と小文字を区別しない。
- テスト以外では `unwrap` / `expect` を使わず、エラーは `eyre::Result` で返す。
- 自前で書く前に、標準ライブラリや既存の依存に同じ機能がないか確かめる。
- データの持ち方は、使い方（どう探すか、どう更新するか）に合わせる。
- YAGNI。今の Issue に要らない抽象化や拡張はしない。

## 依存の追加

Issue に書かれていない依存を追加するときは、PR の説明に理由を書く。

## 言語

コードのコメントとコミットメッセージは英語で書く。
