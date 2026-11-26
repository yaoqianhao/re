//! 覆盖 diff 三算子：`diff`（原型 → 目标算差异）、`apply`（原型 + 覆盖 → 实体）、
//! `revert`（覆盖倒回模板态）。
//!
//! # 本卡的范围
//!
//! M2-E01 冻结了签名与语义约定，M2-E03 把三个算子全部落地：
//!
//! * [`diff`]：只记录**与原型不同**的字段（判据⑨），同值字段收敛为「未覆盖」——
//!   否则 `overrides` 会随实例化膨胀成整份原型，判据⑩ 的体积比立刻失守；
//! * [`apply`]：`None` 继承、`Set` 替换、`Clear` 清空，`name` 由实例给定
//!   （模板的 `proto.name` 只是原型名）；E02 的判据⑬（「实例化结果 ==
//!   `apply(proto, overrides)`」）就压在这条语义上；
//! * [`revert`]：把覆盖倒回模板态，供「撤销一次覆盖」使用。
//!
//! # 为什么比较口径是「序列化取值」而不是 `PartialEq`
//!
//! [`ObjectDesc`] 的两个复合字段（`physics` / `material`）没有实现 `PartialEq`，
//! 给别人的类型补实现既越权也不值当；而本层真正关心的东西本来就是**落盘外形**
//! （哪些键在、哪些键是 `null`、值是多少）。两段 JSON 值相等 ⇔ 写进场景文件后
//! 逐字节相同，这正好是判据⑥⑫ 要比的东西。两条推论写在这里免得踩：
//!
//! * `-0.0` 与 `0.0` 判**不同**（JSON 字面量不同），这是有意的：它们落盘后确实不一样；
//! * `NaN` / `±inf` 在 JSON 里是 `null`，外形与 `Clear` 重合。非有限浮点该在
//!   [`crate::prefab`] 的写入路径就被 `sanitize` 拦住，不该活到 diff 这一层。

use serde::Serialize;
use serde_json::{Map, Value};

use crate::prefab::{ObjectOverride, OverrideState};
use crate::ObjectDesc;

/// 算两条原型之间的字段级差异。
///
/// 只记录**与原型不同**的字段（判据⑨）；同值字段一律落到「键缺失」= 继承模板值。
pub fn diff(proto: &ObjectDesc, target: &ObjectDesc) -> ObjectOverride {
    let proto_fields = json_fields(proto);
    let target_fields = json_fields(target);
    let mut fields = Map::new();
    for name in ObjectOverride::FIELD_NAMES {
        let before = proto_fields.get(*name);
        let after = target_fields.get(*name);
        if before == after {
            continue;
        }
        // 目标侧键缺失 = 该字段被**清掉** → 落 `null`（反序列化成 `Clear`）。
        // 不能写成「不记这个键」：不记就成了「继承」，清空动作会静默丢失。
        fields.insert((*name).to_string(), after.cloned().unwrap_or(Value::Null));
    }
    as_override(fields)
}

/// 把覆盖叠到原型上，并把名字换成实例名。
///
/// 这是「实例化」的最后一步：E02 先回表拿到 [`crate::prefab::PrefabTemplate`] 的
/// `proto`，再用本函数算出要写进场景的 [`ObjectDesc`]。
pub fn apply(proto: &ObjectDesc, overrides: &ObjectOverride, name: &str) -> ObjectDesc {
    let mut out = proto.clone();
    out.name = name.to_string();
    overrides.apply_to(&mut out);
    out
}

/// 把覆盖倒回模板态（覆盖字段逐项归还给原型）。
///
/// 归还 = 每个字段都不再带动作，只剩「继承」；而三态里**只有键缺失表示继承**
/// （`Set` 是替换、`Clear` 是显式清空成 `None`，两者都会盖住模板值），
/// 所以逐字段归还的结果必然是一份**空覆盖**：落盘时整段 `overrides` 省略，
/// `apply` 取回模板原值。这正是判据⑥「`set` → `revert` 后序列化字节 `A == B`」
/// 能成立的原因——不是「revert 恰好没做事」，而是三态语义只允许这一个答案。
///
/// 入参保留（E01 已冻结签名），并且按字段逐个摘而不是直接返回 `default()`：
/// 将来若要支持「按字段掩码部分归还」，这里 `remove` 的粒度就是现成的接线点，
/// 调用方语义不用改。
pub fn revert(overrides: &ObjectOverride) -> ObjectOverride {
    let mut fields = json_fields(overrides);
    for name in ObjectOverride::FIELD_NAMES {
        match overrides.field_state(name) {
            Some(OverrideState::Set) | Some(OverrideState::Clear) => {
                fields.remove(*name);
            }
            _ => {}
        }
    }
    as_override(fields)
}

/// 序列化成 JSON 字段表（键 = 落盘键，缺省字段本就不出现）。
fn json_fields<T: Serialize>(value: &T) -> Map<String, Value> {
    match serde_json::to_value(value) {
        Ok(Value::Object(fields)) => fields,
        Ok(_) => Map::new(),
        // 序列化失败只可能是自定义 `Serialize` 抛错。那时「算不出差异」是明确的事实，
        // 不能假装「没有差异」——空覆盖的语义是「与原型完全一样」，最坏会把真实改动吞掉。
        Err(err) => panic!("原型 / 覆盖必须能序列化成 JSON 对象：{err}"),
    }
}

/// JSON 字段表 → [`ObjectOverride`]。
fn as_override(fields: Map<String, Value>) -> ObjectOverride {
    match serde_json::from_value(Value::Object(fields)) {
        Ok(overrides) => overrides,
        // 走到这里说明 `ObjectOverride::FIELD_NAMES` 与 `ObjectDesc` 的 JSON 键漂移了
        // （改字段时忘了同步 `instance.rs` 的宏清单）。宁可当场炸，也不返回空覆盖：
        // 空覆盖会让「只落差异」退化成「什么都不记」，判据⑨ 表面通过、实际失效。
        Err(err) => panic!("覆盖字段清单与 ObjectDesc 失配（请同步 instance.rs 的宏清单）：{err}"),
    }
}
