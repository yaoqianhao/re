//! 扩展四十探针：逐实例形状掩码。
//!
//! 同一个网格 `Octa` 挂 8 个形状，脚本用掩码决定「这个实例能看见哪几个」。
//! 掩码与「权重置 0」的区别在这里显形：被掩码挡住的形状**权重还在**，
//! 只是不参与形变。

//! 重量与 `shape0` 完全一样，只多一行 `morph_show(0, false)`。
//!
//! 期望：画面回到基线。「权重置 0 假装」的实现做不到这一点上的区别 ——
//! 这里权重**确实**是 1.0（日志会打印），只是被掩码挡在形变之外。
fn on_start() {
    morph_set("Octa", 0, 1.0);
    let ok = morph_show("Octa", 0, false);
    log("mask = " + morph_mask("Octa") + " w0 = " + morph_weight("Octa", 0)
        + " show=" + ok + " visible0 = " + morph_visible("Octa", 0));
}

fn on_update(dt) {}
