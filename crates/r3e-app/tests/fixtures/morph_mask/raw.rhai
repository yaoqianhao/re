//! 扩展四十探针：逐实例形状掩码。
//!
//! 同一个网格 `Octa` 挂 8 个形状，脚本用掩码决定「这个实例能看见哪几个」。
//! 掩码与「权重置 0」的区别在这里显形：被掩码挡住的形状**权重还在**，
//! 只是不参与形变。

fn on_start() {
    log("mask = " + morph_mask("Octa") + " / count = " + morph_count("Octa"));
}

fn on_update(dt) {}
