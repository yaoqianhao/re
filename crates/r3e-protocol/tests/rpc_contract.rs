//! 从 `r3e-editor/src/lib.rs` 内嵌的 4 个 `#[cfg(test)] mod` 原样迁出的 RPC 契约测试。
//!
//! 迁移只动可见性来源（`use super::*` 现在指向本文件根），断言一行未改：
//! M0-S5 的「只搬不删」要求全仓测试数 1 457 保持不变。

use r3e_protocol::*;
use serde_json::{json, Value};

#[cfg(test)]
mod save_rpc_tests {
    use super::*;

    /// 只关心 `scene.save` 的桩：记录收到的路径，回报固定应答。
    #[derive(Default)]
    struct SaveStub {
        seen: Option<Option<String>>,
    }

    impl EngineApi for SaveStub {
        fn info(&mut self) -> EngineInfo {
            unimplemented!("本测试不查 info")
        }

        fn scene_save(&mut self, path: Option<&str>) -> Result<Value, RpcFailure> {
            self.seen = Some(path.map(str::to_string));
            Ok(json!({ "path": path.unwrap_or("<default>"), "objects": 0 }))
        }
    }

    impl NavApi for SaveStub {}
    impl PrefabApi for SaveStub {}

    /// 什么都没实现的宿主：验证 trait 缺省实现拒绝 `scene.save`。
    struct BareStub;

    impl EngineApi for BareStub {
        fn info(&mut self) -> EngineInfo {
            unimplemented!("本测试不查 info")
        }
    }

    impl NavApi for BareStub {}
    impl PrefabApi for BareStub {}

    fn request(method: &str, params: Value) -> Request {
        Request {
            jsonrpc: JSONRPC.to_string(),
            id: Some(json!(1)),
            method: method.to_string(),
            params,
        }
    }

    #[test]
    fn scene_save_passes_path_through() {
        let mut api = SaveStub::default();
        let res = dispatch(
            &mut api,
            &request("scene.save", json!({ "path": "out/scene.json" })),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(
            api.seen,
            Some(Some("out/scene.json".to_string())),
            "路径应原样传给宿主"
        );
        assert_eq!(res.result.unwrap()["path"], json!("out/scene.json"));
    }

    #[test]
    fn scene_save_defaults_to_none_when_path_absent() {
        let mut api = SaveStub::default();
        // 方法名大小写不敏感：`Scene.Save` 与 `scene.save` 等价
        let res = dispatch(&mut api, &request("Scene.Save", json!({})));
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.seen, Some(None), "省略 path 时应交给宿主决定默认路径");
    }

    #[test]
    fn scene_save_rejects_blank_path() {
        let mut api = SaveStub::default();
        let res = dispatch(&mut api, &request("scene.save", json!({ "path": "   " })));
        let err = res.error.expect("空路径应报错");
        assert_eq!(err.code, INVALID_PARAMS);
        assert_eq!(api.seen, None, "参数非法时不应调用宿主");
    }

    #[test]
    fn scene_save_unsupported_by_default() {
        let mut api = BareStub;
        let res = dispatch(&mut api, &request("scene.save", json!({})));
        let err = res.error.expect("缺省实现应报未实现");
        assert_eq!(err.code, METHOD_NOT_FOUND);
        assert!(err.message.contains("scene.save"), "错误消息应点名方法");
    }
}

#[cfg(test)]
mod scene_stack_rpc_tests {
    use super::*;

    /// 记录收到的参数、回报固定应答的桩宿主。
    #[derive(Default)]
    struct StackStub {
        stack_limit: Option<Option<usize>>,
        switched: Option<String>,
        back_calls: usize,
        archived: Option<Option<String>>,
        loaded: Option<String>,
    }

    impl EngineApi for StackStub {
        fn info(&mut self) -> EngineInfo {
            unimplemented!("本测试不查 info")
        }

        fn scene_stack(&mut self, limit: Option<usize>) -> Result<Value, RpcFailure> {
            self.stack_limit = Some(limit);
            Ok(json!({
                "count": 1,
                "current": 0,
                "max": 16, // 与 r3e-core 的 MAX_SCENE_STACK 对齐；传输层不依赖 core，故写字面量
                "entries": [{ "index": 0, "name": "main", "path": "main.json", "objects": 2 }],
            }))
        }

        fn scene_switch(&mut self, path: &str) -> Result<Value, RpcFailure> {
            self.switched = Some(path.to_string());
            Ok(json!({ "path": path, "depth": 2 }))
        }

        fn scene_back(&mut self) -> Result<Value, RpcFailure> {
            self.back_calls += 1;
            Ok(json!({ "name": "main", "depth": 1 }))
        }

        fn scene_archive_save(&mut self, path: Option<&str>) -> Result<Value, RpcFailure> {
            self.archived = Some(path.map(str::to_string));
            Ok(json!({ "path": path.unwrap_or("<default>"), "objects": 0 }))
        }

        fn scene_archive_load(&mut self, path: &str) -> Result<Value, RpcFailure> {
            self.loaded = Some(path.to_string());
            Ok(json!({ "path": path, "objects": 0, "anims": 0 }))
        }
    }

    impl NavApi for StackStub {}
    impl PrefabApi for StackStub {}

    /// 什么都没实现的宿主：验证缺省实现一律拒绝。
    struct BareStub;

    impl EngineApi for BareStub {
        fn info(&mut self) -> EngineInfo {
            unimplemented!("本测试不查 info")
        }
    }

    impl NavApi for BareStub {}
    impl PrefabApi for BareStub {}

    fn request(method: &str, params: Value) -> Request {
        Request {
            jsonrpc: JSONRPC.to_string(),
            id: Some(json!(1)),
            method: method.to_string(),
            params,
        }
    }

    #[test]
    fn scene_stack_passes_limit_through() {
        let mut api = StackStub::default();
        let res = dispatch(&mut api, &request("scene.stack", json!({ "limit": 3 })));
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.stack_limit, Some(Some(3)), "limit 应原样传给宿主");
        assert_eq!(res.result.unwrap()["count"], json!(1));
    }

    #[test]
    fn scene_stack_defaults_to_no_limit() {
        let mut api = StackStub::default();
        // 归一化：`Scene.Stack` / `scene_stack` 都落到同一条分支
        let res = dispatch(&mut api, &request("Scene.Stack", json!({})));
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.stack_limit, Some(None));
    }

    #[test]
    fn scene_switch_requires_non_empty_path() {
        let mut api = StackStub::default();
        let res = dispatch(&mut api, &request("scene.switch", json!({ "path": "  " })));
        let err = res.error.expect("空路径应报错");
        assert_eq!(err.code, INVALID_PARAMS);
        assert_eq!(api.switched, None, "参数非法时不应调用宿主");

        let res = dispatch(&mut api, &request("scene.switch", json!({})));
        assert_eq!(res.error.expect("缺 path 应报错").code, INVALID_PARAMS);

        let res = dispatch(
            &mut api,
            &request("scene.switch", json!({ "path": "lv2.json" })),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.switched.as_deref(), Some("lv2.json"));
    }

    #[test]
    fn scene_back_needs_no_params() {
        let mut api = StackStub::default();
        let res = dispatch(&mut api, &request("scene.back", json!({})));
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.back_calls, 1);
    }

    #[test]
    fn scene_archive_save_path_semantics() {
        let mut api = StackStub::default();
        let res = dispatch(
            &mut api,
            &request("scene.archive.save", json!({ "path": "slots/a.json" })),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.archived, Some(Some("slots/a.json".to_string())));

        let res = dispatch(&mut api, &request("scene.archive.save", json!({})));
        assert!(
            res.error.is_none(),
            "省略 path 应交给宿主决定: {:?}",
            res.error
        );
        assert_eq!(api.archived, Some(None));

        let res = dispatch(
            &mut api,
            &request("scene.archive.save", json!({ "path": "   " })),
        );
        assert_eq!(res.error.expect("空白路径应报错").code, INVALID_PARAMS);
    }

    #[test]
    fn scene_archive_load_requires_path() {
        let mut api = StackStub::default();
        let res = dispatch(
            &mut api,
            &request("scene.archive.load", json!({ "path": "slots/a.json" })),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.loaded.as_deref(), Some("slots/a.json"));

        let res = dispatch(&mut api, &request("scene.archive.load", json!({})));
        assert_eq!(res.error.expect("缺 path 应报错").code, INVALID_PARAMS);
        assert_eq!(
            api.loaded.as_deref(),
            Some("slots/a.json"),
            "参数非法时不改动"
        );
    }

    #[test]
    fn scene_stack_methods_unsupported_by_default() {
        for method in [
            "scene.stack",
            "scene.switch",
            "scene.back",
            "scene.archive.save",
            "scene.archive.load",
        ] {
            let mut api = BareStub;
            let res = dispatch(&mut api, &request(method, json!({ "path": "x.json" })));
            let err = res.error.expect("缺省实现应报未实现");
            assert_eq!(err.code, METHOD_NOT_FOUND, "{method}");
        }
    }
}

#[cfg(test)]
mod audio_spatial_rpc_tests {
    use super::*;

    /// 只关心空间化音频的桩：记下收到的补丁，回报最小应答。
    #[derive(Default)]
    struct SpatialStub {
        set: Option<AudioSpatialPatch>,
        removed: Option<String>,
        cleared: u32,
        listener: Option<Option<ListenerPatch>>,
        enabled: Option<Option<bool>>,
        limit: Option<Option<usize>>,
    }

    impl EngineApi for SpatialStub {
        fn info(&mut self) -> EngineInfo {
            unimplemented!("本测试不查 info")
        }

        fn audio_spatial_list(&mut self, limit: Option<usize>) -> Result<Value, RpcFailure> {
            self.limit = Some(limit);
            Ok(json!({ "count": 1, "sources": [{ "key": "bell" }] }))
        }

        fn audio_spatial_set(&mut self, patch: AudioSpatialPatch) -> Result<Value, RpcFailure> {
            let key = patch.key.clone();
            self.set = Some(patch);
            Ok(json!({ "key": key }))
        }

        fn audio_spatial_remove(&mut self, key: &str) -> Result<Value, RpcFailure> {
            self.removed = Some(key.to_string());
            Ok(json!({ "removed": true }))
        }

        fn audio_spatial_clear(&mut self) -> Result<Value, RpcFailure> {
            self.cleared += 1;
            Ok(json!({ "cleared": 2 }))
        }

        fn audio_spatial_listener(
            &mut self,
            patch: Option<ListenerPatch>,
        ) -> Result<Value, RpcFailure> {
            self.listener = Some(patch);
            Ok(json!({ "position": [0.0, 0.0, 0.0] }))
        }

        fn audio_spatial_set_enabled(
            &mut self,
            enabled: Option<bool>,
        ) -> Result<Value, RpcFailure> {
            self.enabled = Some(enabled);
            Ok(json!({ "enabled": enabled.unwrap_or(true) }))
        }
    }

    impl NavApi for SpatialStub {}
    impl PrefabApi for SpatialStub {}

    struct BareStub;

    impl EngineApi for BareStub {
        fn info(&mut self) -> EngineInfo {
            unimplemented!("本测试不查 info")
        }
    }

    impl NavApi for BareStub {}
    impl PrefabApi for BareStub {}

    fn request(method: &str, params: Value) -> Request {
        Request {
            jsonrpc: JSONRPC.to_string(),
            id: Some(json!(1)),
            method: method.to_string(),
            params,
        }
    }

    #[test]
    fn spatial_set_passes_fields_through() {
        let mut api = SpatialStub::default();
        let res = dispatch(
            &mut api,
            &request(
                "audio.spatial.set",
                json!({
                    "key": "bell",
                    "position": [1.0, 2.0, 3.0],
                    "node": "bell_mesh",
                    "minDistance": 2.0,
                    "maxDistance": 40.0,
                    "model": "INVERSE",
                    "doppler": 0.0,
                }),
            ),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        let patch = api.set.expect("宿主应收到补丁");
        assert_eq!(patch.key, "bell");
        assert_eq!(patch.position, Some([1.0, 2.0, 3.0]));
        assert_eq!(patch.node.as_deref(), Some("bell_mesh"));
        assert_eq!(patch.min_distance, Some(2.0));
        assert_eq!(patch.max_distance, Some(40.0));
        // 大小写归一化交给宿主，传输层原样转交
        assert_eq!(patch.model.as_deref(), Some("INVERSE"));
        assert_eq!(patch.doppler, Some(0.0));
    }

    #[test]
    fn spatial_set_requires_more_than_key() {
        let mut api = SpatialStub::default();
        let res = dispatch(
            &mut api,
            &request("audio.spatial.set", json!({ "key": "bell" })),
        );
        assert_eq!(
            res.error.expect("只给 key 应报错").code,
            INVALID_PARAMS,
            "无字段调用没有意义"
        );
        assert!(api.set.is_none(), "参数非法时不应调用宿主");
    }

    #[test]
    fn spatial_set_rejects_bad_types() {
        let mut api = SpatialStub::default();
        for params in [
            json!({ "key": "  ", "position": [0.0, 0.0, 0.0] }),
            json!({ "key": "bell", "minDistance": "近" }),
            json!({ "key": "bell", "position": [1.0, 2.0] }),
            json!({ "key": "bell", "node": 7 }),
        ] {
            let res = dispatch(&mut api, &request("audio.spatial.set", params.clone()));
            assert_eq!(
                res.error.unwrap_or_else(|| panic!("{params} 应报错")).code,
                INVALID_PARAMS
            );
        }
        assert!(api.set.is_none(), "全部非法，宿主不该被调用");
    }

    #[test]
    fn spatial_methods_are_name_normalized() {
        let mut api = SpatialStub::default();
        // 大小写不敏感（`canonical` 归一化）；点分是方法名的一部分，不能省
        for method in [
            "Audio.Spatial.List",
            "AUDIO.SPATIAL.LIST",
            "audio.spatial.list",
        ] {
            let res = dispatch(&mut api, &request(method, json!({ "limit": 5 })));
            assert!(res.error.is_none(), "{method} 不应报错: {:?}", res.error);
            assert_eq!(api.limit, Some(Some(5)), "{method}");
        }
    }

    #[test]
    fn spatial_remove_requires_non_empty_key() {
        let mut api = SpatialStub::default();
        let res = dispatch(
            &mut api,
            &request("audio.spatial.remove", json!({ "key": "   " })),
        );
        assert_eq!(res.error.expect("空白 key 应报错").code, INVALID_PARAMS);
        assert!(api.removed.is_none());

        let res = dispatch(
            &mut api,
            &request("audio.spatial.remove", json!({ "key": "bell" })),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.removed.as_deref(), Some("bell"));
    }

    #[test]
    fn spatial_clear_needs_no_params() {
        let mut api = SpatialStub::default();
        let res = dispatch(&mut api, &request("audio.spatial.clear", json!({})));
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.cleared, 1);
        assert_eq!(res.result.unwrap()["cleared"], json!(2));
    }

    #[test]
    fn listener_patch_read_only_vs_write() {
        let mut api = SpatialStub::default();
        // 空参数 = 只读：宿主收到 None
        let res = dispatch(&mut api, &request("audio.spatial.listener", json!({})));
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.listener, Some(None), "空参数应走只读");

        // 只给 position
        let res = dispatch(
            &mut api,
            &request(
                "audio.spatial.listener",
                json!({ "position": [4.0, 1.0, 0.0] }),
            ),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        let patch = api.listener.flatten().expect("应转交补丁");
        assert_eq!(patch.position, Some([4.0, 1.0, 0.0]));
        assert_eq!(patch.forward, None, "没给的字段不填默认值");

        // 只给 forward
        let mut api = SpatialStub::default();
        let res = dispatch(
            &mut api,
            &request(
                "audio.spatial.listener",
                json!({ "forward": [0.0, 0.0, -1.0] }),
            ),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        let patch = api.listener.flatten().expect("应转交补丁");
        assert_eq!(patch.forward, Some([0.0, 0.0, -1.0]));
        assert_eq!(patch.position, None);
    }

    #[test]
    fn enabled_is_readable_and_writable() {
        let mut api = SpatialStub::default();
        let res = dispatch(&mut api, &request("audio.spatial.setenabled", json!({})));
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.enabled, Some(None), "省略 enabled 应为只读");

        let res = dispatch(
            &mut api,
            &request("audio.spatial.setenabled", json!({ "enabled": false })),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.enabled, Some(Some(false)));

        let res = dispatch(
            &mut api,
            &request("audio.spatial.setenabled", json!({ "enabled": "yes" })),
        );
        assert_eq!(res.error.expect("非布尔应报错").code, INVALID_PARAMS);
    }

    #[test]
    fn spatial_methods_unsupported_by_default() {
        for method in [
            "audio.spatial.list",
            "audio.spatial.remove",
            "audio.spatial.clear",
            "audio.spatial.listener",
            "audio.spatial.setenabled",
        ] {
            let mut api = BareStub;
            let res = dispatch(&mut api, &request(method, json!({ "key": "bell" })));
            assert_eq!(
                res.error
                    .unwrap_or_else(|| panic!("{method} 应报未实现"))
                    .code,
                METHOD_NOT_FOUND,
                "{method}"
            );
        }
        let mut api = BareStub;
        let res = dispatch(
            &mut api,
            &request(
                "audio.spatial.set",
                json!({ "key": "bell", "doppler": 1.0 }),
            ),
        );
        assert_eq!(res.error.expect("set 也应报未实现").code, METHOD_NOT_FOUND);
    }
}

#[cfg(test)]
mod stats_capture_rpc_tests {
    use super::*;

    /// 只关心 P0-4 两个方法的桩：统计回固定快照，截图记录收到的路径。
    #[derive(Default)]
    struct FeedbackStub {
        stats_calls: usize,
        seen_path: Option<Option<String>>,
    }

    impl EngineApi for FeedbackStub {
        fn info(&mut self) -> EngineInfo {
            unimplemented!("本测试不查 info")
        }

        fn engine_stats(&mut self) -> Result<Value, RpcFailure> {
            self.stats_calls += 1;
            Ok(json!({
                "frame": { "index": 42, "fps": 60.0 },
                "render": { "draw_calls": 7 },
            }))
        }

        fn capture_frame(&mut self, path: Option<&str>) -> Result<Value, RpcFailure> {
            self.seen_path = Some(path.map(str::to_string));
            Ok(json!({
                "path": path.unwrap_or("<default>"),
                "width": 64,
                "height": 32,
                "bytes": 8192,
                "format": "rgba8unormsrgb",
            }))
        }
    }

    impl NavApi for FeedbackStub {}
    impl PrefabApi for FeedbackStub {}

    /// 什么都没实现的宿主：验证 trait 缺省实现拒绝这两个新方法。
    struct BareStub;

    impl EngineApi for BareStub {
        fn info(&mut self) -> EngineInfo {
            unimplemented!("本测试不查 info")
        }
    }

    impl NavApi for BareStub {}
    impl PrefabApi for BareStub {}

    fn request(method: &str, params: Value) -> Request {
        Request {
            jsonrpc: JSONRPC.to_string(),
            id: Some(json!(1)),
            method: method.to_string(),
            params,
        }
    }

    #[test]
    fn engine_stats_returns_host_snapshot() {
        let mut api = FeedbackStub::default();
        let res = dispatch(&mut api, &request("engine.stats", json!({})));
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        let value = res.result.expect("应有结果");
        assert_eq!(value["frame"]["index"], json!(42));
        assert_eq!(value["render"]["draw_calls"], json!(7));
        assert_eq!(api.stats_calls, 1, "应恰好调用宿主一次");
    }

    #[test]
    fn engine_stats_accepts_underscore_alias() {
        // 工具名 engine_stats 经 canonical 后是 enginestats，必须同样可达
        let mut api = FeedbackStub::default();
        let res = dispatch(&mut api, &request("engine_stats", json!({})));
        assert!(res.error.is_none(), "别名应可用: {:?}", res.error);
        assert_eq!(api.stats_calls, 1);
    }

    #[test]
    fn engine_stats_ignores_unknown_params() {
        // 上游可能顺手把 endpop2 等参数一起塞进来；统计是采样，多余参数不该拒
        let mut api = FeedbackStub::default();
        let res = dispatch(
            &mut api,
            &request("engine.stats", json!({ "detail": true })),
        );
        assert!(res.error.is_none(), "多余参数不应报错: {:?}", res.error);
    }

    #[test]
    fn capture_frame_passes_path_through() {
        let mut api = FeedbackStub::default();
        let res = dispatch(
            &mut api,
            &request("capture.frame", json!({ "path": "shots/a.png" })),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(
            api.seen_path,
            Some(Some("shots/a.png".to_string())),
            "路径应原样传给宿主"
        );
        assert_eq!(res.result.unwrap()["bytes"], json!(8192));
    }

    #[test]
    fn capture_frame_defaults_to_none_when_path_absent() {
        let mut api = FeedbackStub::default();
        // 方法名大小写不敏感
        let res = dispatch(&mut api, &request("Capture.Frame", json!({})));
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.seen_path, Some(None), "省略 path 时应交给宿主决定落点");
    }

    #[test]
    fn capture_frame_accepts_underscore_alias() {
        let mut api = FeedbackStub::default();
        let res = dispatch(&mut api, &request("capture_frame", json!({})));
        assert!(res.error.is_none(), "别名应可用: {:?}", res.error);
        assert_eq!(api.seen_path, Some(None));
    }

    #[test]
    fn capture_frame_rejects_blank_path() {
        let mut api = FeedbackStub::default();
        let res = dispatch(&mut api, &request("capture.frame", json!({ "path": "  " })));
        let err = res.error.expect("空路径应报错");
        assert_eq!(err.code, INVALID_PARAMS);
        assert_eq!(api.seen_path, None, "参数非法时不应调用宿主");
    }

    #[test]
    fn capture_frame_rejects_non_string_path() {
        let mut api = FeedbackStub::default();
        let res = dispatch(&mut api, &request("capture.frame", json!({ "path": 7 })));
        assert_eq!(
            res.error.expect("数字路径应报错").code,
            INVALID_PARAMS,
            "类型错应被参数层拦下"
        );
        assert_eq!(api.seen_path, None);
    }

    #[test]
    fn feedback_methods_unsupported_by_default() {
        let mut api = BareStub;
        for method in ["engine.stats", "capture.frame"] {
            let res = dispatch(&mut api, &request(method, json!({})));
            let err = res.error.expect("缺省实现应报未实现");
            assert_eq!(err.code, METHOD_NOT_FOUND, "{method}");
            assert!(
                err.message.contains(method),
                "错误消息应点名方法: {}",
                err.message
            );
        }
    }
}

// ---------------------------------------------------------------- 门禁：实现与声明对齐

/// 从 dispatch 源码里抠出所有 `match` 臂的方法名字面量。
///
/// 只认「行首是双引号 + 本行含 `=>`」的行，取 `=>` 之前的部分再把双引号里的内容全取出——
/// 这样 `"a" | "b" =>`、`"a" if cond =>`、`"a" => match ...` 三种写法都能正确解析，
/// 而臂体里出现的字符串（`"ok"` / `"error"` 之类）不会被误当成方法名。
fn collect_dispatch_arms() -> Vec<String> {
    const SOURCES: &[&str] = &[
        include_str!("../src/dispatch/mod.rs"),
        include_str!("../src/dispatch/query.rs"),
        include_str!("../src/dispatch/tx.rs"),
        include_str!("../src/dispatch/scene.rs"),
        include_str!("../src/dispatch/media.rs"),
        include_str!("../src/dispatch/physics.rs"),
        include_str!("../src/dispatch/nav.rs"),
        include_str!("../src/dispatch/character.rs"),
        include_str!("../src/dispatch/terrain.rs"),
        include_str!("../src/dispatch/prefab.rs"),
        include_str!("../src/dispatch/inspect.rs"),
    ];

    let mut names = Vec::new();
    for src in SOURCES {
        for line in src.lines() {
            let head = line.trim_start();
            if !head.starts_with('"') {
                continue;
            }
            let Some(pos) = head.find("=>") else { continue };
            let mut in_str = false;
            let mut buf = String::new();
            for ch in head[..pos].chars() {
                match ch {
                    '"' => {
                        if in_str {
                            names.push(std::mem::take(&mut buf));
                        }
                        in_str = !in_str;
                    }
                    c if in_str => buf.push(c),
                    _ => {}
                }
            }
        }
    }
    names.sort_unstable();
    names.dedup();
    names
}

/// `RPC_METHODS` 必须与 `dispatch` 的 `match` 臂双向对齐（S5-03 的「删实现要红」）。
///
/// 为什么扫源码而不是调 `dispatch`：删掉一个臂时代码**照样编译**（掉进 `_ =>` 未知方法分支），
/// 运行时才会 404。扫源码能把这种「声明还在、实现没了」的漂移在测试期就按住。
#[test]
fn rpc_methods_match_dispatch_arms() {
    /// 允许比 `RPC_METHODS` 多出来的臂：传输层心跳 + 两条无点历史别名（仍接受，不再声明）。
    const EXTRA_ARMS: &[&str] = &["ping", "enginestats", "captureframe"];

    let arms = collect_dispatch_arms();

    let mut expected: Vec<String> = RPC_METHODS.iter().map(|m| (*m).to_string()).collect();
    expected.extend(EXTRA_ARMS.iter().map(|m| (*m).to_string()));
    expected.sort_unstable();
    expected.dedup();

    let missing: Vec<&String> = expected.iter().filter(|m| !arms.contains(m)).collect();
    let extra: Vec<&String> = arms.iter().filter(|m| !expected.contains(m)).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "RPC_METHODS 与 dispatch 臂不一致：dispatch 缺 {missing:?}（声明了但没实现），多 {extra:?}（实现了但没声明）"
    );
    assert_eq!(
        arms.len(),
        RPC_METHODS.len() + EXTRA_ARMS.len(),
        "臂总数对不上"
    );
}
