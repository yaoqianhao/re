//! 实例与覆盖：场景里的一次引用 + 相对模板的字段级差异。
//!
//! # 三态是怎么实现的（本卡最容易改坏的一处）
//!
//! 覆盖字段的 JSON 取值集合是 `值 | null | 键缺失`，三者语义各不相同：
//!
//! | JSON | 含义 | 结构体 |
//! |---|---|---|
//! | `"visible": false` | 覆盖成 `false` | `Some(FieldOverride::Set(false))` |
//! | `"visible": null` | **显式清空**模板里的 `visible` | `Some(FieldOverride::Clear)` |
//! | 键不存在 | 继承模板值 | `None` |
//!
//! 字段类型写成 `Option<FieldOverride<T>>` 而不是 `Option<T>`，就是为了让
//! 「清空」和「继承」能分开表达：`Clear` 是**动作**，会落盘成 `null`，
//! 否则「把模板里的材质删掉」这个操作在往返后就消失了。
//!
//! 关键分歧点在反序列化：serde 对 `Option<T>` 会把 `null` 和缺失都读成 `None`。
//! 这里用 [`deserialize_opt_field`] 接管——只要**键存在**就经
//! `Option::<T>::deserialize` 判断（`null` → `Clear`、有值 → `Set`），
//! 只有**键缺失**才由 `#[serde(default)]` 给 `None`。
//!
//! # 字段清单为什么由宏生成
//!
//! 可覆盖字段有 12 个（[`crate::ObjectDesc`] 的 13 个字段去掉 `name`——节点名由实例
//! 自己给，模板里的 `proto.name` 只是原型名，覆盖它没有意义）。手抄这 12 个名字会被
//! 用在五个地方（结构体字段、`FIELD_NAMES`、`count`、`field_state`、`apply_to`），
//! 任何一处漏改都会变成「静默不支持某字段」。宏 [`object_override!`] 只写一遍清单。
//!
//! # 为什么没有 `PartialEq`
//!
//! [`crate::PhysicsDesc`] / [`crate::SurfaceMaterial`] 没有实现 `PartialEq`；
//! 给别人的类型补实现既越权也不值当。判据一律「序列化成 JSON 再比」，
//! 这也更贴近本卡关心的东西：**外形**（哪些键在、哪些键是 `null`）。

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::prefab::validate_prefab_id;
use crate::{ObjectDesc, PhysicsDesc, SurfaceMaterial};

/// 单字段覆盖的三态之一（另两态是「键缺失」= `None`）。
#[derive(Debug, Clone)]
pub enum FieldOverride<T> {
    /// 覆盖为该值。
    Set(T),
    /// 显式清空：`apply` 后该字段为 `None`，不再继承模板值。落盘为 `null`。
    Clear,
}

impl<T> FieldOverride<T> {
    /// 覆盖成 `value`。
    pub fn set(value: T) -> Self {
        Self::Set(value)
    }

    /// 清空（落 `null`）。
    pub fn clear() -> Self {
        Self::Clear
    }

    /// 本字段的覆盖态。
    pub fn state(&self) -> OverrideState {
        match self {
            Self::Set(_) => OverrideState::Set,
            Self::Clear => OverrideState::Clear,
        }
    }

    /// 是否落了具体值（`Clear` 返回 `false`——它没有值，但有语义）。
    pub fn is_set(&self) -> bool {
        matches!(self, Self::Set(_))
    }

    /// 取具体值（`Clear` → `None`）。
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Set(value) => Some(value),
            Self::Clear => None,
        }
    }
}

/// 某个字段当前的覆盖态（工具层 / 命令层报状态用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideState {
    /// 未覆盖：继承模板值。
    Inherit,
    /// 覆盖为具体值。
    Set,
    /// 显式清空。
    Clear,
}

impl OverrideState {
    /// 是否为「有覆盖」（`Set` 与 `Clear` 都算：清空也是动作）。
    pub fn is_overridden(self) -> bool {
        matches!(self, Self::Set | Self::Clear)
    }

    /// 稳定的小写名字，供能力清单 / 命令回执使用。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Set => "set",
            Self::Clear => "clear",
        }
    }
}

impl<T: Serialize> Serialize for FieldOverride<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Set(value) => value.serialize(serializer),
            Self::Clear => serializer.serialize_none(),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for FieldOverride<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Self::Set(value),
            None => Self::Clear,
        })
    }
}

impl<T: schemars::JsonSchema> schemars::JsonSchema for FieldOverride<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!("FieldOverride_{}", T::schema_name()).into()
    }

    /// 三态在 JSON 上的取值集合就是 `T | null`；「键缺失」由外层 `Option` 表达。
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        generator.subschema_for::<Option<T>>()
    }
}

/// 读 `Option<FieldOverride<T>>`：进来即「键存在」，故返回 `Some(..)`。
///
/// `null` 由 `Option::<T>::deserialize` 判成 `None`，再映射为 [`FieldOverride::Clear`]；
/// 键缺失不会走到这里（`#[serde(default)]` 先兜住）。
fn deserialize_opt_field<'de, D, T>(deserializer: D) -> Result<Option<FieldOverride<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(FieldOverride::deserialize(deserializer)?))
}

/// 把一条覆盖叠到原型字段上：继承不动、`Set` 替换、`Clear` 置空。
fn apply_field<T: Clone>(slot: &mut Option<T>, overlay: Option<&FieldOverride<T>>) {
    match overlay {
        None => {}
        Some(FieldOverride::Set(value)) => *slot = Some(value.clone()),
        Some(FieldOverride::Clear) => *slot = None,
    }
}

/// 生成 [`ObjectOverride`]：字段清单只写这一遍（结构体 / 名字表 / 计数 / 状态查询 / 叠加）。
///
/// 清单与 [`ObjectDesc`] 的 JSON 键逐字对齐（`materialRef` 保持驼峰），
/// 因为覆盖的序列化结果会被 E02 直接写进场景文件、被 E03 用来算差异字节。
macro_rules! object_override {
    ($($label:literal => $field:ident : $ty:ty),* $(,)?) => {
        /// 实例相对模板的字段级差异（本卡只冻结外形，语义在 E03）。
        ///
        /// 只包含**可覆盖**的 12 个字段：[`ObjectDesc`] 的 13 个字段里去掉 `name`。
        /// 未知字段（含 `name`）一律反序列化失败——覆盖里写 `name` 是调用方的错，
        /// 静默忽略会让人以为改了实例名。
        #[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
        #[serde(default, deny_unknown_fields)]
        pub struct ObjectOverride {
            $(
                #[doc = concat!("模板 `", $label, "` 字段的覆盖态。")]
                #[serde(
                    rename = $label,
                    skip_serializing_if = "Option::is_none",
                    deserialize_with = "deserialize_opt_field"
                )]
                pub $field: Option<FieldOverride<$ty>>,
            )*
        }

        impl ObjectOverride {
            /// 覆盖字段的 JSON 名，顺序与结构体字段一致（判据⑭按它比对，无重复）。
            pub const FIELD_NAMES: &'static [&'static str] = &[$($label),*];

            /// 有覆盖的字段数：`Set` 与 `Clear` 都算（清空也是动作，否则清空会丢）。
            pub fn count(&self) -> usize {
                let mut count = 0;
                $( if self.$field.is_some() { count += 1; } )*
                count
            }

            /// 一条覆盖都没有（此时整段 `overrides` 不落盘）。
            pub fn is_empty(&self) -> bool {
                self.count() == 0
            }

            /// 字段名 → 覆盖态；名字不在表里返回 `None`（调用方据此报错，不静默）。
            pub fn field_state(&self, field: &str) -> Option<OverrideState> {
                match field {
                    $(
                        $label => Some(
                            self.$field
                                .as_ref()
                                .map_or(OverrideState::Inherit, |overlay| overlay.state()),
                        ),
                    )*
                    _ => None,
                }
            }

            /// 把覆盖叠到原型上：继承不动、`Set` 替换、`Clear` 置空。
            ///
            /// 归一化（标签去重、radius 收敛等）不在这一层做——`apply` 只负责
            /// 「算出目标形态」，写入路径的收敛留给真正落场景的那一步（E02/E04）。
            pub fn apply_to(&self, proto: &mut ObjectDesc) {
                $( apply_field(&mut proto.$field, self.$field.as_ref()); )*
            }
        }
    };
}

object_override! {
    "mesh" => mesh: String,
    "position" => position: [f32; 3],
    "rotation" => rotation: [f32; 3],
    "scale" => scale: [f32; 3],
    "color" => color: [f32; 4],
    "parent" => parent: String,
    "visible" => visible: bool,
    "physics" => physics: PhysicsDesc,
    "material" => material: SurfaceMaterial,
    "materialRef" => material_ref: String,
    "tags" => tags: Vec<String>,
    "interact_radius" => interact_radius: f32,
}

/// 场景里对模板的一次引用。
///
/// 只有「引用 + 差异」：模板本体不进场景文件（E02 判据⑫），实例靠 `template`
/// 这个名字回表取原型。`overrides` 为空时整段不落盘（判据③）。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrefabInstance {
    /// 实例（节点）名：进场景后就是节点名，重名规则由 `Scene::spawn` 定（E02）。
    pub name: String,
    /// 模板 ID（形状见 [`crate::prefab::validate_prefab_id`]）。
    pub template: String,
    /// 父节点名；省略表示挂场景根。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// 相对模板的差异；空则整键省略。
    #[serde(default, skip_serializing_if = "ObjectOverride::is_empty")]
    pub overrides: ObjectOverride,
}

impl PrefabInstance {
    /// 建一个无覆盖、挂根的实例。
    pub fn new(name: impl Into<String>, template: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            template: template.into(),
            parent: None,
            overrides: ObjectOverride::default(),
        }
    }

    /// 指定父节点名。
    pub fn with_parent(mut self, parent: impl Into<String>) -> Self {
        self.parent = Some(parent.into());
        self
    }

    /// 带上一条覆盖。
    pub fn with_override(mut self, overrides: ObjectOverride) -> Self {
        self.overrides = overrides;
        self
    }

    /// 收敛：名字与模板 ID 去首尾空白、父名去空白（空串即「挂根」，不留 `""`）。
    pub fn sanitized(mut self) -> Self {
        self.name = self.name.trim().to_string();
        self.template = self.template.trim().to_string();
        self.parent = self
            .parent
            .map(|parent| parent.trim().to_string())
            .filter(|parent| !parent.is_empty());
        self
    }

    /// 校验模板 ID 形状（实例重名 / 父级存在性属 `Scene::spawn` 的职责，在 E02）。
    pub fn validate(&self) -> Result<(), crate::prefab::PrefabIdError> {
        validate_prefab_id(&self.template)
    }

    /// 是否有覆盖（判据③的落盘开关）。
    pub fn has_overrides(&self) -> bool {
        !self.overrides.is_empty()
    }
}
