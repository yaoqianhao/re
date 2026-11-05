//! 3D 空间化音频的**纯数据与纯函数**层（路线表 §12 第十项）。
//!
//! 这一层刻意不碰设备、不碰场景：
//!
//! * 不依赖 `r3e-core`——空间化只需要三个 `f32` 的坐标，把 `core` 的 `Vec3` 拖进来会让
//!   音频 crate 反向依赖渲染数据模型；节点绑定用**节点名字符串**表达（由 `app` 层解析成
//!   世界坐标，见 [`crate::Audio::spatial_bindings`]）。
//! * 不碰 `AudioBackend`——这里只算「该给这条实例乘多少衰减」「该用多少倍速播放」，
//!   真正下发在后端（多普勒要变调，走 [`crate::AudioBackend::set_speed`]）。
//!
//! 三个旋钮的分工：
//!
//! | 旋钮 | 输入 | 作用 | 落点 |
//! |---|---|---|---|
//! | 距离衰减 | 听者位置 ↔ 声源位置 | 远了变小 | 乘到音量上 |
//! | 多普勒 | 声源径向速度 | 靠近变尖 / 远离变闷 | 播放倍速 |
//! | 听者位姿 | 活动相机的 position / forward | 上两者的参考点 | 只读快照 |
//!
//! **边界（本项不做）**：HRTF 双耳渲染、左右/环绕声像（需要多声道输出与卷积）、
//! 遮挡物衍射、房间混响。`forward` 保留在 [`ListenerPose`] 里只用于探针对照，
//! 不参与增益计算——所以「声音没有左右之分」是**设计取舍**，不是漏算。

/// 空间化声源注册表上限。与 UI 元素、局部光源一样给硬上限：
/// 脚本每帧都能调用注册函数，没有上限就是一个可以吃满内存的口子。
/// 超出后 `attach_*` 返回 `false` 并记一笔错误，已存在的实例不受影响。
pub const MAX_SPATIAL_SOURCES: usize = 32;

/// 距离衰减的下界默认值（米）。距离 ≤ 该值时增益恒为 1（近场不增益，避免爆音）。
pub const DEFAULT_MIN_DISTANCE: f32 = 1.0;

/// 距离衰减的上界默认值（米）。距离 ≥ 该值时增益恒为 0。
pub const DEFAULT_MAX_DISTANCE: f32 = 100.0;

/// 音速默认值（米/秒，20℃ 空气）。多普勒的分母就是它。
pub const DEFAULT_SPEED_OF_SOUND: f32 = 343.0;

/// 播放倍速的硬夹紧范围（`1 / 4 ..= 4`）。
///
/// 夹紧不是洁癖：倍速会同时改变音高**和**播放时长，超出这个范围的倍速在实际听感上
/// 已经是「变声」而不是「多普勒」了，而且会把 rodio 的采样步进推到失真区。
pub const MAX_DOPPLER_SHIFT: f32 = 4.0;

/// 距离衰减模型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DistanceModel {
    /// 逆距离：`min / d`，再乘一个在 `max` 处归零的平滑窗。默认值，最像点声源。
    #[default]
    Inverse,
    /// 线性：从 `min` 处的 1 直线降到 `max` 处的 0。可预期、便于做关卡数值。
    Linear,
    /// 逆平方：`(min / d)^2`，同样乘平滑窗。衰减比逆距离陡得多。
    Exponential,
}

impl DistanceModel {
    /// 稳定字符串（脚本 / 配置文件 / 探针都用它，不随 Debug 输出变化）。
    pub fn name(self) -> &'static str {
        match self {
            Self::Inverse => "inverse",
            Self::Linear => "linear",
            Self::Exponential => "exponential",
        }
    }

    /// 解析模型名；大小写不敏感，未知字符串返回 `None`（调用方决定回退策略）。
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "inverse" | "inv" | "1/d" => Some(Self::Inverse),
            "linear" | "lin" => Some(Self::Linear),
            "exponential" | "exp" | "1/d2" => Some(Self::Exponential),
            _ => None,
        }
    }

    /// 全部模型的稳定顺序（供脚本枚举 / 文档对齐）。
    pub fn all() -> [Self; 3] {
        [Self::Inverse, Self::Linear, Self::Exponential]
    }
}

/// 单个空间化声源的调参集合。
///
/// 全部字段都是 `f32`，构造后应当经过 [`sanitize_settings`] 才参与计算：脚本传进来的
/// `NaN` / `inf` / `min >= max` 一律收敛成合法值，绝不 panic、也不产生 `NaN` 音量
/// （`NaN` 音量会一路传到 `Player::set_volume`，声音直接消失且没有任何报错）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialSettings {
    /// 距离 ≤ 该值时增益恒 1。
    pub min_distance: f32,
    /// 距离 ≥ 该值时增益恒 0。
    pub max_distance: f32,
    /// 衰减曲线。
    pub model: DistanceModel,
    /// 多普勒强度：`0` = 关（不受速度影响），`1` = 物理正确，`>1` = 夸张效果。
    pub doppler_factor: f32,
    /// 音速（米/秒）。
    pub speed_of_sound: f32,
}

impl Default for SpatialSettings {
    fn default() -> Self {
        Self {
            min_distance: DEFAULT_MIN_DISTANCE,
            max_distance: DEFAULT_MAX_DISTANCE,
            model: DistanceModel::default(),
            doppler_factor: 1.0,
            speed_of_sound: DEFAULT_SPEED_OF_SOUND,
        }
    }
}

impl SpatialSettings {
    /// 按远近两端构造（模型 / 多普勒 / 音速取默认值）。
    pub fn new(min_distance: f32, max_distance: f32) -> Self {
        Self {
            min_distance,
            max_distance,
            ..Self::default()
        }
    }

    /// 收敛成合法值后的副本。见 [`sanitize_settings`]。
    pub fn sanitized(self) -> Self {
        sanitize_settings(self)
    }

    /// 便于脚本 / 探针一行读完。
    pub fn summary(self) -> String {
        let s = self.sanitized();
        format!(
            "{} {:.2}..{:.2}m doppler×{:.2} c={:.1}m/s",
            s.model.name(),
            s.min_distance,
            s.max_distance,
            s.doppler_factor,
            s.speed_of_sound
        )
    }
}

/// 把任意（可能来自脚本的）参数收敛到可计算区间。
///
/// 规则：
/// * `min_distance`：非有限或 `<= 0` → [`DEFAULT_MIN_DISTANCE`]，最终夹到 `0.01 ..= 1000`；
/// * `max_distance`：非有限 → [`DEFAULT_MAX_DISTANCE`]；且必须 `> min`，
///   否则改成 `min + 1e-3`（只差一点点也比 0 除更安全）；
/// * `doppler_factor`：非有限 → `1.0`，夹到 `0.0 ..= 8.0`（`0` 合法 = 关）；
/// * `speed_of_sound`：非有限或 `<= 1` → 默认值，夹到 `1.0 ..= 10000.0`。
pub fn sanitize_settings(settings: SpatialSettings) -> SpatialSettings {
    let min = if settings.min_distance.is_finite() && settings.min_distance > 0.0 {
        settings.min_distance.clamp(0.01, 1000.0)
    } else {
        DEFAULT_MIN_DISTANCE
    };
    let mut max = if settings.max_distance.is_finite() && settings.max_distance > 0.0 {
        settings.max_distance.clamp(0.01, 100_000.0)
    } else {
        DEFAULT_MAX_DISTANCE
    };
    if max <= min {
        max = min + 1e-3;
    }
    let doppler_factor = if settings.doppler_factor.is_finite() {
        settings.doppler_factor.clamp(0.0, 8.0)
    } else {
        1.0
    };
    let speed_of_sound = if settings.speed_of_sound.is_finite() && settings.speed_of_sound > 1.0 {
        settings.speed_of_sound.clamp(1.0, 10_000.0)
    } else {
        DEFAULT_SPEED_OF_SOUND
    };
    SpatialSettings {
        min_distance: min,
        max_distance: max,
        model: settings.model,
        doppler_factor,
        speed_of_sound,
    }
}

/// 坐标清洗：任一分量非有限（NaN / inf）→ 整点退回原点。
///
/// 脚本传入的坐标是"不可信输入"，`NaN` 一旦进了距离计算会污染增益，
/// 而增益为 `NaN` 会让后端把这条实例的音量永久设成 `NaN`（真的会静音）。
/// 统一在入口收一次，比在每个公式里判断便宜得多。
pub fn sanitize_position(position: [f32; 3]) -> [f32; 3] {
    if position.iter().all(|v| v.is_finite()) {
        position
    } else {
        [0.0; 3]
    }
}

/// 两点距离（非有限输入按 `0` 处理，交给调用方的边界判断）。
pub fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = sub(a, b);
    let len = dot(d, d).sqrt();
    if len.is_finite() {
        len
    } else {
        0.0
    }
}

/// 距离衰减增益，恒在 `0 ..= 1`。
///
/// 边界语义：
/// * `distance <= min_distance` → `1.0`（近场不放大）；
/// * `distance >= max_distance` → `0.0`（城外完全听不见，不是"很小声"）；
/// * 两者之间按模型衰减，`Inverse` / `Exponential` 额外乘一个
///   `1 - smoothstep(t)` 平滑窗：逆距离曲线在 `max` 处本来不为 0，
///   不乘窗就会在边界上"啪"地静音，玩家能听出这个断点。
/// * `distance` 为 `NaN` / `inf` → `0.0`（保守取静音：宁可听不见，也不要把 `NaN`
///   一路传进后端把这条实例的音量永久污染）。
pub fn attenuation(distance: f32, settings: SpatialSettings) -> f32 {
    let s = settings.sanitized();
    if !distance.is_finite() {
        return 0.0;
    }
    if distance <= s.min_distance {
        return 1.0;
    }
    if distance >= s.max_distance {
        return 0.0;
    }
    let span = s.max_distance - s.min_distance;
    let t = ((distance - s.min_distance) / span).clamp(0.0, 1.0);
    let window = 1.0 - smoothstep(t);
    let gain = match s.model {
        DistanceModel::Inverse => (s.min_distance / distance).min(1.0) * window,
        DistanceModel::Linear => 1.0 - t,
        DistanceModel::Exponential => (s.min_distance / distance).powi(2).min(1.0) * window,
    };
    if gain.is_finite() {
        gain.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// 声源速度在「声源 → 听者」方向上的投影，正 = **正在靠近**听者，负 = 远离。
///
/// 声源与听者重合时（距离 < 1e-4）没有可定义的方向，返回 `0`，
/// 多普勒据此不动作——比随便挑一个方向抖一下更合理。
pub fn radial_velocity(
    source_velocity: [f32; 3],
    source_position: [f32; 3],
    listener_position: [f32; 3],
) -> f32 {
    let dir = sub(listener_position, source_position);
    let len = dot(dir, dir).sqrt();
    if !len.is_finite() || len < 1e-4 {
        return 0.0;
    }
    let unit = mul(dir, 1.0 / len);
    let v = dot(source_velocity, unit);
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// 多普勒播放倍速比，恒在 `1 / MAX_DOPPLER_SHIFT ..= MAX_DOPPLER_SHIFT`。
///
/// 公式取经典近似 `ratio = c / (c - v_toward)`：
/// * 静止 → `1.0`；
/// * 朝听者来（`v_toward > 0`）→ 分母变小 → `ratio > 1`（音高升高）；
/// * 远离（`v_toward < 0`）→ `ratio < 1`（音高降低）；
/// * `doppler_factor == 0` → 恒 `1.0`（关掉多普勒）；
/// * `v_toward >= c`（超过音速）时公式发散，用 [`MAX_DOPPLER_SHIFT`] 夹住。
pub fn doppler_ratio(radial_velocity: f32, settings: SpatialSettings) -> f32 {
    let s = settings.sanitized();
    if s.doppler_factor <= 0.0 || !radial_velocity.is_finite() {
        return 1.0;
    }
    let c = s.speed_of_sound;
    let scaled = radial_velocity * s.doppler_factor;
    let denom = c - scaled;
    if denom <= 0.0 {
        // 分母被推过零：要么是"超音速靠近"（物理上该是无穷高的音高，夹到上限），
        // 要么是参数被填成了极端值。两种情况都只允许给极端倍速，绝不允许返回负数。
        return if scaled > 0.0 {
            MAX_DOPPLER_SHIFT
        } else {
            1.0 / MAX_DOPPLER_SHIFT
        };
    }
    let raw = c / denom;
    if !raw.is_finite() {
        return 1.0;
    }
    raw.clamp(1.0 / MAX_DOPPLER_SHIFT, MAX_DOPPLER_SHIFT)
}

/// 听者位姿快照（由 `app` 每帧从**活动相机**推导后推进来）。
///
/// `forward` 是指向"视线方向"的单位向量，默认 `-Z`。它不参与增益计算，
/// 只随快照记录（探针里能看到相机转向，将来做声像时也用得上）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListenerPose {
    /// 听者（= 相机）世界位置。
    pub position: [f32; 3],
    /// 听者朝向（单位向量，非有限时退化为 `-Z`）。
    pub forward: [f32; 3],
}

impl Default for ListenerPose {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            forward: [0.0, 0.0, -1.0],
        }
    }
}

impl ListenerPose {
    /// 用位置 + 朝向构造；任一分量非有限则回退默认值，朝向零向量退化为 `-Z`。
    pub fn new(position: [f32; 3], forward: [f32; 3]) -> Self {
        let position = if position.iter().all(|v| v.is_finite()) {
            position
        } else {
            [0.0, 0.0, 0.0]
        };
        let len = dot(forward, forward).sqrt();
        let forward = if len.is_finite() && len > 1e-6 {
            mul(forward, 1.0 / len)
        } else {
            [0.0, 0.0, -1.0]
        };
        Self { position, forward }
    }

    /// 只有位置（朝向取 `-Z`）。
    pub fn from_position(position: [f32; 3]) -> Self {
        Self::new(position, [0.0, 0.0, -1.0])
    }
}

/// 一条空间化声源的运行时状态。
///
/// 位置既可以由脚本直接给（[`SpatialSource::position`] 被 `Audio::set_source_position` 改），
/// 也可以由 `app` 每帧从场景节点解析后喂进来；`node` 只记录"绑在谁身上"，
/// 解析成坐标是 `app` 的事（本 crate 不依赖 `r3e-core`）。
#[derive(Debug, Clone)]
pub struct SpatialSource {
    /// 当前世界位置。
    pub position: [f32; 3],
    /// 上一帧位置（算速度用）。
    pub previous_position: [f32; 3],
    /// 平滑后的速度（米/秒）。
    pub velocity: [f32; 3],
    /// 绑定的场景节点名；`None` = 位置完全由脚本给。
    pub node: Option<String>,
    /// 调参。
    pub settings: SpatialSettings,
    /// 最近一次算出的距离增益（`0 ..= 1`）。
    pub gain: f32,
    /// 最近一次算出的播放倍速（`0.25 ..= 4`）。
    pub rate: f32,
    /// 最近一次算出的到听者距离（米），探针用。
    pub distance: f32,
}

impl SpatialSource {
    /// 用位置 + 调参构造；初始增益按"还没算过"取 `1.0`。
    pub fn new(position: [f32; 3], settings: SpatialSettings) -> Self {
        let position = if position.iter().all(|v| v.is_finite()) {
            position
        } else {
            [0.0, 0.0, 0.0]
        };
        Self {
            position,
            previous_position: position,
            velocity: [0.0; 3],
            node: None,
            settings: settings.sanitized(),
            gain: 1.0,
            rate: 1.0,
            distance: 0.0,
        }
    }

    /// 绑定节点名（链式，便于构造）。
    pub fn with_node(mut self, node: impl Into<String>) -> Self {
        self.node = Some(node.into());
        self
    }

    /// 推进一帧，返回 `(gain, rate)`。
    ///
    /// `dt` 是帧时长（秒）：
    /// * `dt <= 0`（首帧 / 暂停）→ 不更新速度，只按当前位置重算增益（暂停时相机还在动，
    ///   距离衰减必须跟着变，否则暂停再继续会听到一声爆音）；
    /// * 速度用**位置差分**得到，再做一阶平滑（系数 0.4）：脚本瞬移节点时
    ///   不平滑就会有一个 300m/s 的单帧尖峰，音高"咔"一下。
    pub fn advance(&mut self, listener: ListenerPose, dt: f32) -> (f32, f32) {
        if dt.is_finite() && dt > 0.0 {
            let raw = mul(sub(self.position, self.previous_position), 1.0 / dt);
            if raw.iter().all(|v| v.is_finite()) {
                const SMOOTH: f32 = 0.4;
                self.velocity = add(mul(self.velocity, 1.0 - SMOOTH), mul(raw, SMOOTH));
            }
        }
        self.previous_position = self.position;

        self.distance = distance(self.position, listener.position);
        self.gain = attenuation(self.distance, self.settings);
        let v = radial_velocity(self.velocity, self.position, listener.position);
        self.rate = doppler_ratio(v, self.settings);
        (self.gain, self.rate)
    }
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn mul(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn model_names_round_trip() {
        for m in DistanceModel::all() {
            assert_eq!(DistanceModel::parse(m.name()), Some(m));
        }
        assert_eq!(DistanceModel::parse("LINEAR"), Some(DistanceModel::Linear));
        assert_eq!(
            DistanceModel::parse(" 1/d2 "),
            Some(DistanceModel::Exponential)
        );
        assert_eq!(DistanceModel::parse("bogus"), None);
        assert_eq!(DistanceModel::default(), DistanceModel::Inverse);
    }

    #[test]
    fn attenuation_边界与单调() {
        let s = SpatialSettings::new(2.0, 32.0);
        assert!(close(attenuation(0.0, s), 1.0), "重合必须是满增益");
        assert!(close(attenuation(2.0, s), 1.0), "min 处必须是满增益");
        assert!(close(attenuation(32.0, s), 0.0), "max 处必须静音");
        assert!(close(attenuation(999.0, s), 0.0), "max 外必须静音");

        // 三种模型都必须单调不增，且严格掉下来（不是常数）。
        for m in DistanceModel::all() {
            let s = SpatialSettings {
                model: m,
                ..SpatialSettings::new(2.0, 32.0)
            };
            let mut prev = 1.0;
            for i in 0..=30 {
                let d = 2.0 + i as f32;
                let g = attenuation(d, s);
                assert!(g <= prev + 1e-6, "{m:?} 在 d={d} 处反弹了：{prev} → {g}");
                assert!((0.0..=1.0).contains(&g), "{m:?} 增益越界：{g}");
                prev = g;
            }
            assert!(prev < 0.05, "{m:?} 在 32m 处没有降下来：{prev}");
            // 逆距离在中点的值应当明显大于线性（同距离时逆距离衰减更慢）
            if m == DistanceModel::Inverse {
                assert!(attenuation(6.0, s) > 0.1);
            }
        }
    }

    #[test]
    fn attenuation_线性模型是平滑斜坡() {
        let s = SpatialSettings {
            model: DistanceModel::Linear,
            ..SpatialSettings::new(1.0, 11.0)
        };
        // 中点 6m → 恰好 0.5
        assert!(close(attenuation(6.0, s), 0.5));
        assert!(close(attenuation(3.5, s), 0.75));
    }

    #[test]
    fn attenuation_非有限距离取静音() {
        let s = SpatialSettings::default();
        assert_eq!(attenuation(f32::NAN, s), 0.0);
        assert_eq!(attenuation(f32::INFINITY, s), 0.0);
        assert_eq!(attenuation(f32::NEG_INFINITY, s), 0.0);
    }

    #[test]
    fn sanitize_收敛非法参数() {
        let s = sanitize_settings(SpatialSettings {
            min_distance: -5.0,
            max_distance: 0.0,
            model: DistanceModel::Linear,
            doppler_factor: f32::NAN,
            speed_of_sound: 0.0,
        });
        assert!(close(s.min_distance, DEFAULT_MIN_DISTANCE));
        assert!(s.max_distance > s.min_distance, "max 必须大于 min");
        assert!(close(s.doppler_factor, 1.0));
        assert!(close(s.speed_of_sound, DEFAULT_SPEED_OF_SOUND));

        // min >= max 时把 max 顶出去一点，绝不出现 0 除
        let s = sanitize_settings(SpatialSettings::new(10.0, 10.0));
        assert!(s.max_distance > s.min_distance);
        let g = attenuation(10.0, s);
        assert!(g.is_finite() && (0.0..=1.0).contains(&g));

        // 0 倍多普勒是合法的「关闭」，不能被当成非法值纠正成 1
        let s = sanitize_settings(SpatialSettings {
            doppler_factor: 0.0,
            ..SpatialSettings::default()
        });
        assert!(close(s.doppler_factor, 0.0));
    }

    #[test]
    fn radial_velocity_方向正负() {
        // 听者在原点，声源在 +X 10m 处
        let src = [10.0, 0.0, 0.0];
        let lis = [0.0, 0.0, 0.0];
        // 朝听者飞（-X 方向 10m/s）→ 正
        let v = radial_velocity([-10.0, 0.0, 0.0], src, lis);
        assert!(close(v, 10.0), "靠近必须是正：{v}");
        // 远离（+X）→ 负
        let v = radial_velocity([10.0, 0.0, 0.0], src, lis);
        assert!(close(v, -10.0), "远离必须是负：{v}");
        // 纯切向（+Y）→ 0
        let v = radial_velocity([0.0, 10.0, 0.0], src, lis);
        assert!(close(v, 0.0), "切向不该有多普勒：{v}");
        // 重合 → 0（无定义时保守取 0）
        let v = radial_velocity([10.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]);
        assert!(close(v, 0.0));
    }

    #[test]
    fn doppler_比例与夹紧() {
        let s = SpatialSettings::default();
        assert!(close(doppler_ratio(0.0, s), 1.0), "静止必须 1.0");
        let fast = doppler_ratio(30.0, s);
        assert!(fast > 1.0 && fast < 1.2, "靠近 30m/s 应略升：{fast}");
        let slow = doppler_ratio(-60.0, s);
        assert!(slow < 1.0 && slow > 0.75, "远离 60m/s 应略降：{slow}");

        // 超音速靠近 + factor 拉满 → 夹在上限，且不出 NaN / 负数
        let s = SpatialSettings {
            doppler_factor: 8.0,
            ..SpatialSettings::default()
        };
        let hi = doppler_ratio(343.0, s);
        assert!(close(hi, MAX_DOPPLER_SHIFT), "超音速靠近应夹到上限：{hi}");
        let lo = doppler_ratio(-343.0, s);
        assert!(
            (1.0 / MAX_DOPPLER_SHIFT..=1.0).contains(&lo),
            "极端远离也不该越界：{lo}"
        );

        // 刚好等于音速（分母归零）不能变成 NaN / inf
        let ratio = doppler_ratio(DEFAULT_SPEED_OF_SOUND, SpatialSettings::default());
        assert!(ratio.is_finite(), "{ratio}");

        // factor = 0 → 关掉
        let s = SpatialSettings {
            doppler_factor: 0.0,
            ..SpatialSettings::default()
        };
        assert!(close(doppler_ratio(500.0, s), 1.0));

        // 非有限速度不产生 NaN 倍速
        assert!(close(
            doppler_ratio(f32::NAN, SpatialSettings::default()),
            1.0
        ));
        assert!(doppler_ratio(f32::INFINITY, SpatialSettings::default()).is_finite());
    }

    #[test]
    fn listener_pose_退化处理() {
        let l = ListenerPose::new([1.0, 2.0, 3.0], [0.0, 0.0, -4.0]);
        assert!(close(l.forward[2], -1.0), "朝向必须归一化");
        assert_eq!(l.position, [1.0, 2.0, 3.0]);

        let l = ListenerPose::new([f32::NAN, 0.0, 0.0], [0.0, 0.0, 0.0]);
        assert_eq!(l.position, [0.0, 0.0, 0.0]);
        assert_eq!(l.forward, [0.0, 0.0, -1.0]);

        assert_eq!(ListenerPose::default().position, [0.0; 3]);
        assert_eq!(
            ListenerPose::from_position([5.0, 0.0, 0.0]).position,
            [5.0, 0.0, 0.0]
        );
    }

    #[test]
    fn source_advance_静止与运动() {
        let mut src = SpatialSource::new([12.0, 0.0, 0.0], SpatialSettings::new(1.0, 100.0));
        let lis = ListenerPose::from_position([0.0, 0.0, 0.0]);

        // 首帧（dt=0）：算出距离衰减，速度保持 0 → 倍速 1
        let (gain, rate) = src.advance(lis, 0.0);
        assert!(gain < 1.0, "12m 处必须有衰减：{gain}");
        assert!(close(src.distance, 12.0));
        assert!(close(rate, 1.0));

        // 逐帧靠近：每帧 1m（dt=0.1 → 10m/s），10 帧后停在 x=2（不越过听者）
        for _ in 0..10 {
            src.position[0] -= 1.0;
            src.advance(lis, 0.1);
        }
        let approaching = src.advance(lis, 0.1).1;
        assert!(close(src.distance, 2.0));
        assert!(approaching > 1.0, "持续靠近后音高该升：{approaching}");

        // 原路退回到 x=12（远离 10 帧）→ 倍速落到 1 以下
        for _ in 0..10 {
            src.position[0] += 1.0;
            src.advance(lis, 0.1);
        }
        let receding = src.advance(lis, 0.1).1;
        assert!(close(src.distance, 12.0));
        assert!(receding < 1.0, "远离后音高该降：{receding}");
    }

    #[test]
    fn source_advance_瞬移不产生速度尖峰() {
        let mut src = SpatialSource::new([0.0, 0.0, 0.0], SpatialSettings::default());
        let lis = ListenerPose::from_position([0.0, 0.0, 50.0]);
        src.advance(lis, 0.016);
        // 一帧瞬移 100m（dt=1/60）→ 真实速度 6000m/s；平滑后不该等于这个尖峰
        src.position[0] = 100.0;
        let (_, rate) = src.advance(lis, 1.0 / 60.0);
        assert!(
            rate.is_finite() && (0.25..=MAX_DOPPLER_SHIFT).contains(&rate),
            "{rate}"
        );
        let radial = radial_velocity(src.velocity, src.position, lis.position);
        assert!(
            radial.abs() < 6000.0 * 0.6,
            "单帧瞬移必须被平滑掉：{radial}"
        );
        // 第二帧起速度稳定（位置不再变）→ 倍速回落到 1 附近
        let (_, rate) = src.advance(lis, 1.0 / 60.0);
        assert!(rate.is_finite());
    }

    #[test]
    fn summary_稳定格式() {
        let s = SpatialSettings::new(1.5, 20.0);
        let text = s.summary();
        assert!(text.starts_with("inverse 1.50..20.00m"), "{text}");
        assert!(text.contains("doppler×1.00"));
    }
}
