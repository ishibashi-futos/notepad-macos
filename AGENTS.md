# AGENTS.md

## 概要
- macOS向けの軽量・高速なテキストエディタをRustで実装する
- CoreとUIを分離し、UIは描画/入力、Coreは編集ロジックを担当する

## 構成/依存
- src/main.rs: 起動/イベントループ
- src/app.rs: 依存の組み立て/ライフサイクル
- src/core/: ドメインロジックとデータ構造
- src/ui/: winit/wgpu/glyphonで描画/入力
- 依存方向はUI -> Coreのみ。CoreはUI/OSに依存しない

## Core責務
- ropeyによるバッファ管理
- 挿入/削除/選択/undo/redo
- ドキュメント状態(変更有無、カーソル)
- encoding_rsによるI/O変換

## UI責務
- winitのイベント処理とIME管理
- glyphonでの描画
- Coreの操作API呼び出しと状態表示

## エラーハンドリング
- 例外は`System`と`Domain`の2層のみ
- Coreは`Result<T, CoreError>`で返し、`context`で詳細を補足
- UIがメッセージ整形/通知、`retriable`で再試行導線を決定

```rust
#[derive(Debug)]
pub enum CoreError { System(SystemError), Domain(DomainError) }

#[derive(Debug)]
pub struct SystemError {
    pub kind: SystemErrorKind,
    pub context: String,
    pub retriable: bool,
}

#[derive(Debug)]
pub enum SystemErrorKind { Io, Permission, Encoding, Os, Unknown }

#[derive(Debug)]
pub struct DomainError {
    pub kind: DomainErrorKind,
    pub context: String,
}

#[derive(Debug)]
pub enum DomainErrorKind {
    InvalidOperation,
    InvalidState,
    OutOfRange,
    EmptySelection,
}
```

## 非同期
- UIスレッドは描画/入力専用
- I/O/検索など重い処理は非同期タスク化し、UIとはメッセージで連携
- タブ切替/ファイル切替でタスクをキャンセルできる設計
- 共有状態は最小化し、必要ならスナップショットを渡す
