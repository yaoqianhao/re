//! 扩展三十九探针：夹具 `octa_face.glb` 挂 8 个形状，脚本只决定「动哪一个」。
//!
//! 形状 0 把左边两个顶点往左拉（第 1 组权重 `w0.x`），形状 4 把上边两个顶点
//! 往上顶（第 2 组权重 `w1.x`），形状 7 的位移全 0（第 2 组 `w1.w`，用来验
//! 「权重进了正确的槽位但位移为 0」—— 差 0 像素才算映射没错位）。

fn on_start() {
    log("morph count = " + morph_count("Octa"));
    morph_set("Octa", 7, 1.0);
    log("w7 = " + morph_weight("Octa", 7) + " active = " + morph_active("Octa"));
}

fn on_update(dt) {}
