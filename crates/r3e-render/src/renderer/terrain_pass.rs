//! 地形通道的设备侧接线（M2-C03b）：持有 `TerrainPass`、按需上传、往主通道提交。
//!
//! 地形是**纯派生层**：几何由运行时按地形场 + LOD 现场生成，渲染层不认识高度场，
//! 只接受一份「已经能直接上传」的顶点/索引。这条边界让 `r3e-render` 不依赖
//! `r3e-terrain`，也让地形几何的判重（要不要重传）留在知道 LOD 何时变的那一侧。
//!
//! 为什么单独一个文件而不是塞进 `frame.rs`：`frame.rs` 已经背着主通道的整段编码逻辑，
//! 再往里放「建管线 / 传缓冲 / 写 uniform」会把「怎么画一帧」和「地形怎么上 GPU」
//! 搅在一起；这两个东西的改动理由完全不同。

use r3e_core::Scene;

use crate::postfx::HDR_FORMAT;
use crate::terrain::{TerrainDrawItem, TerrainLayersUniform, TerrainPass};

use super::{Renderer, DEPTH_FORMAT, MSAA_SAMPLE_COUNT};

impl Renderer {
    /// 递入本帧地形几何。`None` = 撤掉地形通道（释放管线与缓冲）。
    ///
    /// 传空几何（顶点或索引为空）**不建通道**：空地形与没有地形画出来一模一样，
    /// 却要白付一条管线 + 两块缓冲的常驻成本。
    ///
    /// 判重由调用方负责：它知道 LOD 何时切换、地块何时重新装配。这里只认
    /// 「递进来就是要重传」，逐帧比对上万顶点是纯浪费。
    pub fn set_terrain(&mut self, item: Option<TerrainDrawItem>) {
        match item {
            Some(item) if !item.is_empty() => self.terrain_dirty = Some(item),
            // 空几何 = 这一帧没有地形可画，把通道撤掉
            Some(_) | None => {
                self.terrain = None;
                self.terrain_dirty = None;
            }
        }
    }

    /// 设定 4 层材质参数（基色 + 光照塑形）。缺省值即草绿 / 岩灰 / 沙黄 / 雪白。
    pub fn set_terrain_layers(&mut self, layers: TerrainLayersUniform) {
        self.terrain_layers = layers;
    }

    /// 当前地形待绘制的索引数；0 表示这一帧没有地形。
    pub fn terrain_index_count(&self) -> u32 {
        self.terrain
            .as_ref()
            .map(TerrainPass::index_count)
            .unwrap_or(0)
    }

    /// 每帧准备：脏数据上传 + 写相机 / 材质 uniform。
    ///
    /// 没有地形（也没收到脏数据）时**一次动作都不做** —— 不用地形的场景，
    /// 这段调用连一次 `write_buffer` 都不会产生。
    pub(crate) fn prepare_terrain(&mut self, scene: &Scene) {
        let dirty = self.terrain_dirty.take();
        if self.terrain.is_none() {
            let Some(item) = dirty else { return };
            let pass = TerrainPass::new(&self.device, HDR_FORMAT, DEPTH_FORMAT, MSAA_SAMPLE_COUNT);
            let mut pass = pass;
            pass.upload(&self.device, &self.queue, &item);
            self.terrain = Some(pass);
        } else if let Some(item) = dirty {
            if let Some(pass) = self.terrain.as_mut() {
                pass.upload(&self.device, &self.queue, &item);
            }
        }

        // 相机每帧都在动，uniform 必须每帧重写（32 个 f32 + 96 字节材质表，代价可忽略）
        let mut camera = scene.camera;
        camera.aspect = self.aspect();
        let light = scene.light;
        // 环境项取 `light.ambient` 的最亮分量：着色器只认一个标量，
        // 取最亮而不是平均，避免偏色环境（例如落日橙）把地形压成灰
        let ambient = light.ambient.x.max(light.ambient.y).max(light.ambient.z);
        if let Some(pass) = self.terrain.as_ref() {
            pass.update_uniforms(
                &self.queue,
                &camera.view_projection().to_cols_array_2d(),
                [camera.position.x, camera.position.y, camera.position.z],
                [light.direction.x, light.direction.y, light.direction.z],
                [light.intensity, ambient],
                &self.terrain_layers,
            );
        }
    }

    /// 往主通道里画地形，返回本帧绘制的索引数（0 = 没画）。
    ///
    /// 必须在**不透明段之后、半透明段之前**调用：地形是不透明几何，混进半透明段
    /// 会吃到混合公式；而放在不透明段里又会和 PBR 批次抢管线设置。
    pub(crate) fn draw_terrain(&self, pass: &mut wgpu::RenderPass<'_>) -> u32 {
        let Some(terrain) = self.terrain.as_ref() else {
            return 0;
        };
        terrain.draw(pass);
        terrain.index_count()
    }
}
