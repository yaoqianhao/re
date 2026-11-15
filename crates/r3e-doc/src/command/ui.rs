//! UI / 文本域。
//!
//! UI 复用 core 的 `ui::upsert` / `ui::remove`（含 `MAX_UI_ELEMENTS` 容量检查）；
//! 文本在 core 里没有 upsert 辅助（`texts` 就是裸 `Vec<ScreenText>`，脚本自己 push），
//! 这里按 key 做「有则替换、无则追加」，与 UI 的 key 语义保持一致。

use r3e_core::ui::{self, UiUpsert};
use r3e_core::{Scene, ScreenText, UiElement};

use crate::{command::CommandKind, DocError};

/// 按 key 写 UI 元素。
pub(super) fn write_ui(s: &mut Scene, el: &UiElement) -> Result<(), DocError> {
    match ui::upsert(&mut s.ui, el.clone()) {
        UiUpsert::Rejected => Err(DocError::Rejected(format!(
            "UI 元素表已满，`{}` 未写入",
            el.key
        ))),
        UiUpsert::Inserted | UiUpsert::Replaced => Ok(()),
    }
}

/// 按 key 删 UI 元素。
pub(super) fn remove_ui(s: &mut Scene, key: &str) -> Result<(), DocError> {
    if ui::find(&s.ui, key).is_none() {
        return Err(DocError::NotFound(format!("UI 元素 `{key}`")));
    }
    ui::remove(&mut s.ui, key);
    Ok(())
}

/// 按 key 写屏幕文本。
pub(super) fn write_text(s: &mut Scene, text: &ScreenText) -> Result<(), DocError> {
    match s.texts.iter_mut().find(|t| t.key == text.key) {
        Some(slot) => *slot = text.clone(),
        None => s.texts.push(text.clone()),
    }
    Ok(())
}

/// 按 key 删屏幕文本。
pub(super) fn remove_text(s: &mut Scene, key: &str) -> Result<(), DocError> {
    let before = s.texts.len();
    s.texts.retain(|t| t.key != key);
    if s.texts.len() == before {
        Err(DocError::NotFound(format!("屏幕文本 `{key}`")))
    } else {
        Ok(())
    }
}

impl CommandKind {
    /// 写 UI 命令（前像为同 key 的旧元素）。
    pub fn upsert_ui(s: &Scene, next: UiElement) -> Self {
        let old = ui::find(&s.ui, &next.key).cloned();
        Self::UpsertUi { next, old }
    }

    /// 删 UI 命令。
    pub fn remove_ui(s: &Scene, key: &str) -> Result<Self, DocError> {
        let old = ui::find(&s.ui, key)
            .cloned()
            .ok_or_else(|| DocError::NotFound(format!("UI 元素 `{key}`")))?;
        Ok(Self::RemoveUi { old })
    }

    /// 写文本命令（前像为同 key 的旧文本）。
    pub fn upsert_text(s: &Scene, next: ScreenText) -> Self {
        let old = s.texts.iter().find(|t| t.key == next.key).cloned();
        Self::UpsertText { next, old }
    }

    /// 删文本命令。
    pub fn remove_text(s: &Scene, key: &str) -> Result<Self, DocError> {
        let old = s
            .texts
            .iter()
            .find(|t| t.key == key)
            .cloned()
            .ok_or_else(|| DocError::NotFound(format!("屏幕文本 `{key}`")))?;
        Ok(Self::RemoveText { old })
    }
}
