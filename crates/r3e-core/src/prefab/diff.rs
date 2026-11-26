//! 覆盖 diff 三算子：`diff`（原型 → 目标算差异）、`apply`（原型 + 覆盖 → 实体）、
//! `revert`（覆盖倒回模板态）。
//!
//! # 本卡的范围
//!
//! M2-E01 只冻结**签名与语义约定**：`diff` / `revert` 是空壳（`unimplemented!`），
//! 实体在 E03（判据⑨⑩⑪：只落差异、可逆）。`apply` 在本卡给出实现——
//! E02 的判据⑬（「实例化结果 == `apply(proto, overrides)`」）需要它已经可用，
//! 而它本身只是三态的机械展开，不含策略。
//!
//! # 语义约定（E03 按此实现，不得改口径）
//!
//! * `diff(proto, target)`：只记录**与原型不同**的字段；同值字段收敛为「未覆盖」，
//!   否则 `overrides` 会随实例化膨胀成整份原型，E03 判据⑨（同值字段数 == 0）即不再成立；
//! * `apply(proto, overrides, name)`：`None` 继承、`Set` 替换、`Clear` 清空，
//!   `name` 由实例给定（模板的 `proto.name` 只是原型名）；
//! * `revert(overrides)`：把覆盖倒回模板态，供「撤销一次覆盖」使用。

use crate::prefab::ObjectOverride;
use crate::ObjectDesc;

/// 算两条原型之间的字段级差异。
///
/// **E03 落地**：本卡只给签名，调用即 panic——与其返回一个"看起来能用但语义没定"
/// 的空覆盖，不如让误用立刻暴露。
pub fn diff(_proto: &ObjectDesc, _target: &ObjectDesc) -> ObjectOverride {
    unimplemented!("M2-E03 落地：diff 只记录与原型不同的字段")
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
/// **E03 落地**：与 [`diff`] 同理，本卡只给签名。
pub fn revert(_overrides: &ObjectOverride) -> ObjectOverride {
    unimplemented!("M2-E03 落地：覆盖倒回模板态")
}
