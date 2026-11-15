//! 写入口：`version` / `dirty` / `writes` 的记账点。
//!
//! 全仓只有一个写函数 [`Document::write`]。这里刻意不做任何校验、不做逐帧扫描：
//! `FnOnce` 零分配，`version += 1` 一条 add，性能面只多一次自增（S3 风险表 R6）。

use r3e_core::Scene;

use crate::Document;

impl Document {
    /// 当前版本号。每次 [`Document::write`] / [`Document::replace_scene`] 自增。
    ///
    /// S4 的 `StateHash` 以它作节拍器：版本变了才需要重算哈希。
    pub fn version(&self) -> u64 {
        self.version
    }

    /// 是否有未保存改动。
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// 标记为已保存（存盘成功后由上层调用）。
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// 累计写次数。诊断与测试用，不进序列化、不影响渲染。
    pub fn write_count(&self) -> u64 {
        self.writes
    }

    /// **唯一可变通道**。
    ///
    /// 闭包拿到 `&mut Scene` 后可以自由调用 `Scene` 的任何方法或改公开字段；返回后
    /// 自动推进版本、置脏、累计写次数。
    ///
    /// 控制流注意（S3 风险表 R1）：闭包体里的 `return` 只退出闭包，不再退出外层函数；
    /// `?` 也只在闭包内生效。需要提前退出外层时，让闭包返回 `Result` / `Option`，
    /// 由调用方在外层 `?`。
    ///
    /// ```ignore
    /// // 单表达式
    /// doc.write(|s| s.upsert_local_light(light));
    /// // 多行块
    /// doc.write(|s| {
    ///     s.camera.position = pos;
    ///     s.camera.target = target;
    /// });
    /// // 需要短路
    /// doc.write(|s| s.spawn(desc))?;      // 闭包返回 Result，外层 ?
    /// ```
    pub fn write<T>(&mut self, f: impl FnOnce(&mut Scene) -> T) -> T {
        let out = f(&mut self.scene);
        self.touch();
        out
    }

    /// 整体换场景（打开新场景 / 场景栈回退），返回旧场景。
    ///
    /// 换场景也是一次「写」：版本推进、置脏，但**不**回绕版本号——回退旧场景后上层
    /// 拿到的是一个更新的版本，缓存不会误命中（S3 风险表 R3）。
    pub fn replace_scene(&mut self, next: Scene) -> Scene {
        let old = std::mem::replace(&mut self.scene, next);
        // 换场景后旧撤销栈的前像全部指向另一个场景，留着只会撤销出错。
        self.undo.clear();
        self.touch();
        old
    }

    /// 记账：版本 +1、置脏、写次数 +1。
    ///
    /// 命令层（[`crate::command`]）执行成功后会自己调一次，因此命令走的是「先改、
    /// 成功才记账」的顺序，失败不推进版本。
    pub(crate) fn touch(&mut self) {
        self.version += 1;
        self.dirty = true;
        self.writes += 1;
    }
}
