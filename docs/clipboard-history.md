# Ctrl+V: アプリ内クリップボードHistory（実装プラン/仕様）

## 目的
- システムクリップボードとは独立した「アプリ内履歴」を提供する。
- Cmd+C で履歴・システムクリップボードの両方に追加、Ctrl+V で履歴一覧を表示し、選択して貼り付ける。
- 通常のCmd+V は既存のシステムクリップボード連携のまま維持する。

## 仕様（確定）
### 対象データ
- テキストのみ（UTF-8）を履歴対象とする。
- 空文字列は履歴に追加しない。

### 履歴サイズ
- 最大 100 件。
- 追加時に上限を超えた場合は **最古を破棄**。
- 重複は **連続重複のみ抑制**（直前と同一なら追加しない）。

### 追加ルール
- Cmd+Fn+C 実行時、選択範囲がある場合のみ履歴へ追加。
- 追加後、選択インデックスは「最新」を指す。

### 表示/操作ルール（Ctrl+V）
- Ctrl+V を押すと、**下部ナビゲーションに履歴ウィンドウを表示**する。
- 表示は **3件ウィンドウでスクロール**する（上下キーで全履歴を移動）。
  - 選択がウィンドウ外に出た場合、ウィンドウは選択に追従してスクロールする。
- 表示中は **上キー/下キーで履歴の選択を変更**する。
- Enter or 1,2,3のいずれかの数字キーで確定し、**選択している内容をカーソル位置に貼り付け**る。
  - 1,2,3 は「表示中ウィンドウ内の位置」を指す。
- Escape でキャンセル（貼り付けなし）。
- 表示中は通常のテキスト入力は抑止する。

### 表示内容
- 下部ナビゲーションは、既存の検索ナビに **履歴表示モード**を追加する。
- 表示は「最新順（新しい→古い）」で 3 件ウィンドウ。
- ウィンドウは選択に追従して上下にスクロールする。
- 各行は先頭 40 文字程度で省略表示（改行は `\n` に置換）。
- 選択中の行は `>` で強調する。

例:
```
Clipboard:
> [1] hello world
  [2] こんにちは\n世界
  [3] foo bar baz
```

## データ構造（見直し）
### ClipboardHistory
- `items: Vec<String>`
- `max: usize` (固定 100)
- `selected_index: usize`（0 が最新）
- `window_start: usize`（表示ウィンドウの先頭）
- `visible: bool`

### 状態遷移
- `Ctrl+V` -> `visible = true`, `selected_index = 0`
- `Up` -> `selected_index = selected_index.saturating_sub(1)` + ウィンドウ追従
- `Down` -> `selected_index = min(selected_index + 1, items.len()-1)` + ウィンドウ追従
- `Enter` -> `visible = false` + 選択内容を貼り付け
- `Escape` -> `visible = false`
 - `1/2/3` -> `selected_index = window_start + (key-1)` + 貼り付け

## UI表示（見直し）
- `search_nav_buffer` を再利用して履歴表示を描画する。
- `search_nav_visible` を流用または新規フラグ `history_visible` を追加。
- 表示中は検索ナビより履歴ナビを優先表示する。
- 3件ウィンドウのスクロールを許容する高さを確保する。

## コア/API案
CoreはUI/OSに依存しない方針なので、履歴は **UI側（App/Ui層）** に配置。

- `ClipboardHistory::push(text: &str)`
  - 空文字は無視
  - 直前と同一なら無視
  - 上限超過なら先頭から削除
- `ClipboardHistory::visible_items() -> Vec<String>`
  - 選択に追従する3件ウィンドウを整形して返す
- `ClipboardHistory::selected_text() -> Option<&str>`
- `ClipboardHistory::show()` / `hide()`
- `ClipboardHistory::move_up()` / `move_down()`
- `ClipboardHistory::window_range() -> Range<usize>`
  - 現在の3件ウィンドウの範囲を返す

## イベント連携（UI/App）
- Cmd+Fn+C
  - `core.selected_text()` を取得
  - `ClipboardHistory::push()`
- Ctrl+V
  - `ClipboardHistory::show()`
  - `refresh_search_ui()` で履歴表示モードに切替
- Up/Down
  - 履歴表示中のみ `move_up()` / `move_down()`
  - `refresh_search_ui()` で更新
- 1/2/3
  - 履歴表示中のみ `window_start + (key-1)` を選択
  - `selected_text()` を `core.insert_str()` で貼り付け
  - `hide()`
- Enter
  - 履歴表示中のみ `selected_text()` を `core.insert_str()`
  - `hide()`
- Escape
  - 履歴表示中のみ `hide()`

## 例外/エラー
- 履歴が空の状態で Ctrl+V は何も表示しない。
- クリップボードの取得/設定は不要（アプリ内履歴のみ）。

## テスト方針
- 履歴最大数の境界テスト（101件目の追加）
- 連続重複抑制
- 表示3件の整形（改行/省略）
- 上下キーでの選択移動
- Enter 確定で貼り付けされること

## 実装順
1. `ClipboardHistory` 構造体を App 層に実装
2. 履歴表示テキスト生成と `search_nav_buffer` への統合
3. Ctrl+V/Up/Down/Enter/Escape のイベント分岐を追加
4. テスト追加
