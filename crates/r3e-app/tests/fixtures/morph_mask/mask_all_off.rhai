//! 扩展四十探针：逐实例形状掩码。
//!
//! 同一个网格 `Octa` 挂 8 个形状，脚本用掩码决定「这个实例能看见哪几个」。
//! 掩码与「权重置 0」的区别在这里显形：被掩码挡住的形状**权重还在**，
//! 只是不参与形变。

//! 掩码整块写 0：整个实例摘掉形变（`morph_count` 写 0，着色器提前返回）。
//!
//! 权重仍是 1.0，画面应回到基线，且统计里 `morph=` 实例数掉到 0。
fn on_start() {
    morph_set("Octa", 4, 1.0);
    let effective = morph_mask_set("Octa", 0);
    log("mask = " + morph_mask("Octa") + " effective = " + effective
        + " w4 = " + morph_weight("Octa", 4));
}

fn on_update(dt) {}
