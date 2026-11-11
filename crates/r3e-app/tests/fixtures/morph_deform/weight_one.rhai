//! 形变可视化的试验组：把唯一形状的权重推到 1.0。
//!
//! `morph_face.glb` 里左侧两个顶点带 (-2, 1.8, 0) 的增量，权重 1 时四边形
//! 会向左上拉伸 —— 各向同性的「整体缩放」做不到这个形状，因此截图差异只能
//! 由形变解释。
fn on_start() {
    log("morph count = " + morph_count("Face"));
    morph_set("Face", 0, 1.0);
    log("morph active = " + morph_active("Face") + " w = " + morph_weight("Face", 0));
}

fn on_update(dt) {}
