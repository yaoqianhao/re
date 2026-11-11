//! 形变可视化的对照组：权重全 0（等值于静止姿态）。
//!
//! 与 `weight_one.rhai` 只差一个数字，两帧的像素差全部来自形变位移，
//! 这样「画面变了」就等于「形变确实喂到了顶点着色器」。
fn on_start() {
    log("morph count = " + morph_count("Face"));
    morph_set("Face", 0, 0.0);
    log("morph active = " + morph_active("Face") + " w = " + morph_weight("Face", 0));
}

fn on_update(dt) {}
