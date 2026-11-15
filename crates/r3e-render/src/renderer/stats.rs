use crate::envprobe::EnvProbeStats;
use crate::occlusion::OcclusionStats;
use crate::postfx::PostFxStats;
use crate::ssao::SsaoStats;

/// 一帧的离屏读回结果（P0-4 截图）。
///
/// 只给「已转成 RGBA8、行填充已去掉」的紧密像素，编码成什么图片格式由上层决定
/// （编码要引 `image` 之类的依赖，不该压进渲染层）。
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    /// 紧密排列的 RGBA8，长度恒为 `width * height * 4`；alpha 恒为 255。
    pub rgba8: Vec<u8>,
    /// 读回时用的目标纹理格式（排查偏色用）。
    pub format: wgpu::TextureFormat,
}

impl CapturedFrame {
    /// 像素总字节数，交给调用方做落盘前自检。
    pub fn byte_len(&self) -> usize {
        self.rgba8.len()
    }
}

/// 主通道视锥剔除的层级化读数（扩展二十九）。
///
/// 语义与阴影通道那套（`shadow_pruned` / `shadow_box_tests`）同构，只是下标从
/// 「(级, 遮挡体)」换成「实例」：`instance_tests + pruned` 恒等于「过层级筛的实例数」。
#[derive(Debug, Clone, Copy, Default)]
pub struct CullStats {
    /// 累计重建主通道视锥树的次数。稳态下应保持不变。
    pub tree_builds: u64,
    /// 本帧做的层级盒判定数（被下探的树节点 + 叶子判定）。
    pub box_tests: u32,
    /// 本帧真正落到单个实例上的判定数。
    pub instance_tests: u32,
    /// 没做实例判定就定成不可见的实例数（整枝跳过 + 叶子直接被裁）。
    pub pruned: u32,
    /// `pruned` 里来自多实例子树的部分 —— 层级剔除的净收益。
    pub branch_pruned: u32,
}

/// 主通道实例缓冲的脏段上传读数（扩展三十一）。
///
/// 口径：`recorded + skipped` 恒等于本帧实例记录总数，`bytes` 是这一帧真正
/// `write_buffer` 出去的字节。静态场景里 `skipped` 应接近总数、`bytes` 趋近 0；
/// `full = true` 的那一帧是换代 / 实例数变化 / 脏段碎到超限的退化路径。
#[derive(Debug, Clone, Copy, Default)]
pub struct InstanceUploadStats {
    /// 本帧脏段数量（`full` 时恒为 1）。
    pub ranges: u32,
    /// 本帧真正写入的实例记录数。
    pub recorded: u32,
    /// 与上一帧逐字节相同、被跳过的实例记录数。
    pub skipped: u32,
    /// 本帧实际写出的字节数。
    pub bytes: u64,
    /// 全量上传本该写出的字节数 —— 与 `bytes` 相除就是本帧省下的比例。
    pub full_bytes: u64,
    /// 本帧是否退化成全量。稳态下应恒为 false。
    pub full: bool,
    /// 累计换代次数（缓冲重建 → 影子清零）。逐帧增长说明容量估算失效。
    pub resets: u64,
}

/// 扩展三十五：暂存环的读数。
///
/// 口径：`frames` 是走过暂存路径的帧数，`writes` 是这些帧里真正打到
/// `Queue::write_buffer` 的次数（理想情况 = `frames`，即一帧一次），`copies`
/// 是命令编码器里录制的 `copy_buffer_to_buffer` 条数（≈ 脏段总数），`fallbacks`
/// 是装不下 / 超上限而退化回逐段直传的次数。`saves` = 逐段直传本该有的调用次数
/// 减去 `writes`，就是这一层省掉的队列往返。
#[derive(Debug, Clone, Copy, Default)]
pub struct StagingStats {
    /// 暂存路径是否启用（`R3E_UPLOAD_STAGING=0` 可关掉，回到逐段 `write_buffer`）。
    pub enabled: bool,
    /// 槽数（多帧飞行深度）。
    pub slots: u32,
    /// 单槽字节容量。
    pub capacity: u64,
    /// 累计走暂存路径的帧数。
    pub frames: u64,
    /// 累计 `Queue::write_buffer` 调用次数（暂存路径下每帧至多一次）。
    pub writes: u64,
    /// 累计录制的块拷贝条数。
    pub copies: u64,
    /// 累计暂存字节数。
    pub bytes: u64,
    /// 累计退化次数（装不下 / 超上限 / 关闭）。
    pub fallbacks: u64,
    /// 累计因扩容重建暂存环的次数。逐帧增长说明容量估算失效。
    pub rebuilds: u64,
    /// 本帧省掉的 `write_buffer` 调用次数（`ranges - 1` 的累计和，下限 0）。
    pub saves: u64,
}

/// 一帧的绘制统计，便于日志 / HUD 观测。
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderStats {
    pub draw_calls: u32,
    pub instances: u32,
    /// 本帧被视锥剔除的实例数（阶段三 A）。
    pub culled: u32,
    /// 想剔除但拿不到包围盒、只能照画的实例数。持续非零说明资产层没喂 AABB。
    pub cull_unknown: u32,
    /// 本帧阴影通道提交的 draw call 数（阶段 D）。0 表示阴影关闭或贴图未就绪。
    /// 扩展十六起每级各跑一遍 pass，因此开启 N 级时约为实例批次数 × N。
    pub shadow_draws: u32,
    /// 本帧生效的阴影级联数（扩展十六）。1 = 单级（等价阶段 D），0 = 关闭。
    pub shadow_cascades: u32,
    /// 本帧阴影按级裁剪判过的 (级, 遮挡体) 对数（扩展二十四）。
    /// 0 = 阴影关闭、没有遮挡体，或 `caster_cull` 被关掉。
    pub shadow_casters: u32,
    /// 其中被裁掉的 (级, 遮挡体) 对数（扩展二十四）。越接近 `shadow_casters`，
    /// 说明远景级剔得越多；恒为 0 说明 `caster_extend` 把范围铺得过大或裁剪关闭。
    pub shadow_culled: u32,
    /// 扩展二十五：没做实例判定就定成不可见的实例数（整枝跳过 + 叶子直接被裁）。
    /// `shadow_casters + shadow_pruned` 恒等于「级数 × 遮挡体数」。
    pub shadow_pruned: u32,
    /// 扩展二十五：`shadow_pruned` 里来自多实例子树的部分 —— **层级剔除的净收益**。
    /// 叶子被裁在逐实例判定里也是一次 AABB，省不下来，所以只有这部分算赚。
    pub shadow_branch_pruned: u32,
    /// 扩展二十五：本帧真正做的层级盒判定数。约等于「被下探的树节点数 × 级数」，
    /// 远小于实例数说明树的层级在起作用。
    pub shadow_box_tests: u32,
    /// 扩展二十五：累计重建遮挡体层级树的次数。稳态下应保持不变 —— 逐帧增长
    /// 说明 `Scene::layout_version` 在每帧被推进（增删节点），缓存没生效。
    pub caster_tree_builds: u64,
    /// 扩展二十九：主通道视锥剔除的层级化读数。
    pub cull: CullStats,
    pub frame_index: u64,
    /// 本帧文本通道画出的字形四边形数（M1 文字渲染）。
    pub text_glyphs: u32,
    /// 本帧文本通道的 draw call 数。0 表示文本关闭 / 无字体 / 无文本。
    pub text_draws: u32,
    /// 本帧缺字数（该字符所有已加载字体面都没有），这些字符只占位不绘制。
    pub text_missing: u32,
    /// 本帧 UI 通道画出的四边形数（M1 最小 UI）。
    pub ui_quads: u32,
    /// 本帧 UI 通道的 draw call 数。0 表示 UI 关闭 / 无元素。
    pub ui_draws: u32,
    /// 本帧 HDR 后处理链的 pass 统计（扩展十三）。
    pub postfx: PostFxStats,
    /// 本帧屏幕空间遮蔽的统计（扩展十七）。`passes = 0` 表示整条跳过。
    pub ssao: SsaoStats,
    /// 本帧真正上传并参与计算的局部光源数（扩展十五）。0 = 场景没有灯 / 总开关关闭。
    pub local_lights: u32,
    /// 本帧投影的局部光源数（扩展十八 C，最多 `MAX_LOCAL_SHADOW_CASTERS`）。
    pub local_shadow_casters: u32,
    /// 本帧局部光源阴影提交的深度 pass 层数（点光 6 / 聚光 1）。
    /// 0 = 没有灯开 `cast_shadow`，此时渲染开销与扩展十七完全一致。
    pub local_shadow_layers: u32,
    /// 本帧走半透明通道的 draw call 数（扩展十九）。0 = 场景没有半透明物件。
    pub transparent_draws: u32,
    /// 本帧走半透明通道的实例数。与 `transparent_draws` 一起区分
    /// 「没有半透明物件」与「有但被剔除」。
    pub transparent_instances: u32,
    /// 本帧深度预通道的 draw call 数（扩展十九）。MSAA 之后主通道的深度是
    /// 多重采样的、不能给遮蔽通道用，因此多跑一条单采样深度 pass。
    /// 恒等于不透明批次的 draw call 数；0 表示本帧没有不透明物件。
    pub depth_prepass_draws: u32,
    /// 本帧遮挡剔除的统计（扩展二十二）。`enabled = false` 表示开关关闭或
    /// 深度金字塔还没就绪，此时 `occluded` 恒为 0、画面与关闭前完全一致。
    pub occlusion: OcclusionStats,
    /// 本帧 HDR 反射探针的状态（扩展二十三）。`active = false` 表示没配探针，
    /// 镜面 IBL 走解析天空、画面与扩展十二逐像素一致。
    pub env_probe: EnvProbeStats,
    /// 本帧主通道实例缓冲的上传情况（扩展三十一）。全量路径下
    /// `bytes == full_bytes`，脏段路径下 `skipped` 就是省掉的记录数。
    pub instance_upload: InstanceUploadStats,
    /// 本帧关节矩阵缓冲的上传情况（扩展三十二）。记录是 mat4（64 B），
    /// `skipped` 是与上一帧逐字节相同、没传的矩阵数；`resets` 恒为 0。
    pub joint_upload: InstanceUploadStats,
    /// 扩展三十五：暂存环（持久暂存 + 多帧飞行）的读数。脏段数据先打包进
    /// 一块 CPU 暂存区、一帧只调一次 `write_buffer`，再由编码器按段块拷贝。
    pub upload_staging: StagingStats,
    /// 扩展三十六：本帧形变表规模（浮点数）。0 = 场景里没有带形状的网格。
    pub morph_floats: u64,
    /// 扩展三十六：累计整表重传次数。稳态下应停止增长 —— 逐帧增长说明
    /// `MeshCache` 在每帧重传网格（版本号被无意义地推进）。
    pub morph_uploads: u64,
    /// 本帧带形变且权重非零的实例数（着色器 `morph_offset` 真正被执行的实例）。
    /// 0 = 所有实例都没设权重 / 网格没有形状，顶点路径与扩展三十五逐位相同。
    pub morph_instances: u32,
    /// 扩展三十八：其中**互斥组超出预算、权重被按比例归一化**的实例数。
    /// 0 = 场景没配互斥组（或配了但组内和 ≤ 1），进缓冲的权重与场景原值逐位相同。
    /// 逐帧非零说明有表情在「抢预算」—— 排查「表情叠在一起脸翻面」看这一个数。
    pub morph_normalized: u32,
    /// 扩展三十八：本帧逐形状剔除余量的合计（世界单位）。
    /// 与扩展三十六的「峰值权重 × 最长位移」口径相比只会更小，
    /// 差额即「逐形状余量」省下来的包围盒膨胀量。
    pub morph_cull_inflate: f32,
    /// 扩展四十：本帧被**逐实例形状掩码**整段摘掉形变的实例数。
    /// 0 = 没有实例把形状集关空；非零时这些实例的顶点路径与无形变网格逐位相同
    /// （不是「权重刚好为 0」，而是连 `morph_offset` 都不进）。
    pub morph_masked: u32,
    /// 扩展四十：被掩码挡在门外的非零权重槽合计 —— 本帧省下的乘加组数。
    ///
    /// 与 `morph_instances` 一起看：实例数不变而这里持续非零，说明掩码在
    /// 「同一份网格的实例之间」真的分化了形状集（而不是全场景一刀切）。
    pub morph_masked_shapes: u32,
}
