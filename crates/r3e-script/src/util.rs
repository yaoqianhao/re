//! 脚本层的两个独立小工具：AST 函数探测与 rhai 数组转字符串。
//!
//! 单独成文件是为了让 `lib.rs` 不再继续变大（`cargo xtask lint-size` 的债务只许变小）。

use rhai::{Array, AST};

/// AST 中是否存在指定名字与参数个数的函数。
pub(crate) fn has_fn(ast: &AST, name: &str, arity: usize) -> bool {
    ast.iter_functions()
        .any(|f| f.name == name && f.params.len() == arity)
}

/// 便捷：把脚本里 `names()` 返回的数组转成字符串列表。
pub fn array_to_strings(arr: Array) -> Vec<String> {
    arr.into_iter()
        .filter_map(|d| d.into_string().ok())
        .collect()
}
