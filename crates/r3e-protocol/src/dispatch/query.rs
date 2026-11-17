//! `query.*` 派发（M1-S2-B）：语义查询面三条方法。
//!
//! | 方法 | 语义 | 副作用 |
//! |---|---|---|
//! | `query.find` | 按类别 / 标签 / 区域 / 组件位过滤，返回五字段投影 | 无（只读投影） |
//! | `query.describe` | 单节点的投影 + 命中的组件位 + 类别 | 无 |
//! | `query.stats` | 全场景普查（总量 / 类别分布 / 标签计数） | 无 |
//!
//! 三条方法**都不建事务、都不推版本**：宿主实现里不许出现 `Document::write()`，
//! 「查一次场景哈希就变」在 M1 是硬红线——core 侧 `tests/world_query.rs` 与宿主侧
//! `api::tests::query` 各守一遍，验收口径就是查询前后 `StateHash` 逐位相等。
//!
//! 本层只做参数整形：类别名与组件位名一律**字符串原样下行**，认不认识由
//! `r3e-core` 的 `NodeKind::parse` / `Component::parse` 裁决——拼错的名字必须变成
//! 明确的参数错误，而不是一份看起来正常的空结果（AI 会据此断定「场景里没有」）。
//!
//! 命中不到本组时交给 [`super::tx::dispatch_tx`]。

use super::*;
use crate::query::QuerySpec;

pub(super) fn dispatch_query(
    api: &mut dyn EngineApi,
    id: Option<Value>,
    params: &Value,
    method: &str,
    raw: &str,
) -> Response {
    match method {
        "query.find" => {
            let spec = match query_spec_from(params) {
                Ok(spec) => spec,
                Err(f) => return Response::fail(id, f),
            };
            match api.query_find(spec) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "query.describe" => match with_node(params, |node| api.query_describe(node)) {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "query.stats" => match api.query_stats() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        _ => tx::dispatch_tx(api, id, params, method, raw),
    }
}

/// 解析 `query.find` 的过滤条件。
///
/// 三个列表字段缺省得到空 `Vec`：空 vec 在 core 侧的语义就是「这一维不过滤」，
/// 不必在协议层再套一层 `Option`，否则宿主还得把 `None` 与 `Some([])` 拆成两条路径。
fn query_spec_from(params: &Value) -> Result<QuerySpec, RpcFailure> {
    Ok(QuerySpec {
        // 空串 / 全空白按「没给」处理：AI 把「不限类别」写成 `""` 是常态，
        // 为这种输入报类型错误纯属刁难，而 core 的 `parse("")` 本来也归 `any`。
        kind: opt_str(params, "kind")?
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(str::to_string),
        all_tags: opt_str_list(params, "allTags")?.unwrap_or_default(),
        any_tags: opt_str_list(params, "anyTags")?.unwrap_or_default(),
        region: match param(params, "region") {
            None | Some(Value::Null) => None,
            Some(_) => opt_f32_array::<6>(params, "region")?,
        },
        require: opt_str_list(params, "require")?.unwrap_or_default(),
        exclude: opt_str_list(params, "exclude")?.unwrap_or_default(),
        limit: opt_limit(params)?,
    })
}
