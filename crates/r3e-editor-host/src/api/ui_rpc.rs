//! 屏幕 UI 列举与写入（编辑器 RPC 宿主层）。
//!
//! 由 M0-S2 从单体 `api.rs` 搬入：`self` → `api`，其余语句未改写。

use super::support::*;
use super::*;

pub(super) fn ui_list(
    api: &mut LocalEngineApi<'_>,
    limit: Option<usize>,
) -> Result<Value, RpcFailure> {
    let scene = api.scene.borrow();
    let items: Vec<Value> = scene
        .ui
        .iter()
        .take(limit.unwrap_or(usize::MAX))
        .map(ui_element_value)
        .collect();
    Ok(json!({
        "count": scene.ui.len(),
        "limit": ui::MAX_UI_ELEMENTS,
        "enabled": scene.ui_enabled,
        "elements": items,
    }))
}

pub(super) fn ui_set(api: &mut LocalEngineApi<'_>, patch: UiPatch) -> Result<Value, RpcFailure> {
    if patch.key.trim().is_empty() {
        return Err(RpcFailure::invalid_params("参数 `key` 不能为空"));
    }
    // 名字先全部解析完再落地：一个写错的锚点不该留下半条被改的元素。
    let kind = match patch.kind.as_deref() {
        Some(name) => Some(UiKind::parse(name).ok_or_else(|| {
            RpcFailure::invalid_params(format!(
                "未知元素类型 `{name}`（可用：rect / border / image）"
            ))
        })?),
        None => None,
    };
    let anchor = match patch.anchor.as_deref() {
        Some(name) => Some(Anchor::parse(name).ok_or_else(|| {
            RpcFailure::invalid_params(format!("未知锚点 `{name}`（可用：top_left / center ...）"))
        })?),
        None => None,
    };

    let mut scene = api.scene.borrow_mut();
    let existing = ui::find(&scene.ui, &patch.key).cloned();
    let inserted = existing.is_none();
    let mut element = existing.unwrap_or_else(|| UiElement {
        key: patch.key.clone(),
        ..UiElement::default()
    });
    apply_ui_patch(&mut element, &patch, kind, anchor);
    // 用 `upsert`（裸全量覆盖）而不是 `upsert_styled`：RPC 的语义是「给了才改」，
    // 改动已经在 clone 上做完了；`merge_same_key` 会强制沿用旧的
    // `layer` / `visible` / `uv`，那是脚本每帧重发位置时该有的行为，放在这里会把
    // 编辑器对层级 / 可见性的修改吞掉。
    let outcome = ui::upsert(&mut scene.ui, element.sanitized());
    if outcome == UiUpsert::Rejected {
        return Err(RpcFailure::invalid_params(format!(
            "UI 元素已达上限 {}，无法新增",
            ui::MAX_UI_ELEMENTS
        )));
    }
    let value = ui::find(&scene.ui, &patch.key)
        .map(ui_element_value)
        .unwrap_or(Value::Null);
    Ok(json!({
        "key": patch.key,
        "inserted": inserted,
        "replaced": outcome == UiUpsert::Replaced,
        "count": scene.ui.len(),
        "element": value,
    }))
}

pub(super) fn ui_remove(api: &mut LocalEngineApi<'_>, key: &str) -> Result<Value, RpcFailure> {
    let mut scene = api.scene.borrow_mut();
    let removed = ui::remove(&mut scene.ui, key);
    Ok(json!({ "key": key, "removed": removed, "count": scene.ui.len() }))
}

pub(super) fn ui_state(api: &mut LocalEngineApi<'_>) -> Result<Value, RpcFailure> {
    let interaction = api.scripts.ui_interaction();
    let state = interaction.borrow();
    let events: Vec<Value> = state
        .events()
        .iter()
        .map(|e| json!({ "kind": e.kind.name(), "key": e.key }))
        .collect();
    let (sw, sh) = state.screen();
    Ok(json!({
        "hover": state.hover(),
        "active": state.active(),
        "pointer_over_ui": state.pointer_over_ui(),
        "screen": [sw, sh],
        "events": events,
    }))
}
