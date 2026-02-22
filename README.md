# photo-tagger

工事写真を Gemini AI で機械種別・測点ごとにグループ分けする Rust CLI ツール/ライブラリ。
結果は `photo-groups.json` に出力される。

## 前提条件

- **Gemini CLI** がインストール・認証済みであること

```bash
npm install -g @google/gemini-cli
gemini auth
```

## CLI 使い方

### 写真のグループ分け

```bash
photo-tagger <フォルダ>
```

フォルダ内の写真を AI で分類し、`photo-groups.json` に保存する。

```bash
photo-tagger <フォルダ> --dry-run    # 結果表示のみ（ファイル保存なし）
photo-tagger <フォルダ> --profile    # 処理時間計測を表示
```

### 伝票モード

PDF や画像から伝票データを抽出し、Excel に出力する。

```bash
photo-tagger <PDF> --voucher-type asgara --output out.xlsx
```

ページ範囲の指定:

```bash
photo-tagger <PDF> --voucher-type asgara --page-from 10 --page-to 20 --output out.xlsx
```

JSON から Excel への変換:

```bash
photo-tagger --convert input.json --output out.xlsx
```

## ライブラリ使い方

```rust
use photo_tagger::run_grouping;

let records = run_grouping(folder, 10, None)?;
```

## 出力形式

`photo-groups.json` はファイル名をキーとする JSON オブジェクト:

```json
{
  "20260211_143052.jpg": {
    "role": "機械全景",
    "machine_type": "タイヤローラー",
    "machine_id": "BW24R",
    "group": 1,
    "has_board": false,
    "detected_text": "",
    "description": "タイヤローラーの全景写真"
  }
}
```

| フィールド | 説明 |
|---|---|
| `role` | 写真の役割（機械全景、作業状況、出来形管理 など） |
| `machine_type` | 機械・対象の種類（タイヤローラー、マカダムローラー など） |
| `machine_id` | 型式番号や測点の識別情報 |
| `group` | 時系列でのグループ番号（同一機械・同一時間帯） |
| `has_board` | 黒板が写っているか |
| `detected_text` | 黒板・銘板・証票から読み取ったテキスト |
| `description` | 写真内容の1文要約 |

## インクリメンタル処理

既存の `photo-groups.json` を保持し、新規ファイルのみ解析する。
全ファイルを再分類したい場合は環境変数を設定する:

```bash
PHOTO_TAGGER_FORCE_RECLASSIFY=1 photo-tagger <フォルダ>
```
