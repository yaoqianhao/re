//! `scene.verify` 的协议层契约：只管参数形状，语义与裁决全在宿主。
//!
//! 从 `tests/rpc_contract.rs` 拆出（M1-S4-F）：那个文件已 763 行，再加这组用例会
//! 破 `lint-size` 的 800 行上限。断言一条未改，只换了可见性来源。

use r3e_protocol::*;
use serde_json::{json, Value};

mod scene_verify_rpc_tests {
    use super::*;

    /// 记录收到的查询、回报固定应答的桩宿主。
    #[derive(Default)]
    struct VerifyStub {
        seen: Option<VerifyQuery>,
    }

    impl EngineApi for VerifyStub {
        fn info(&mut self) -> EngineInfo {
            unimplemented!("本测试不查 info")
        }

        fn scene_verify(&mut self, query: VerifyQuery) -> Result<Value, RpcFailure> {
            let echo = query
                .targets
                .iter()
                .map(|t| json!({ "name": t.name, "at": t.at }))
                .collect::<Vec<_>>();
            self.seen = Some(query);
            Ok(json!({ "echo": echo }))
        }
    }

    impl NavApi for VerifyStub {}
    impl PrefabApi for VerifyStub {}

    fn request(method: &str, params: Value) -> Request {
        Request {
            jsonrpc: JSONRPC.to_string(),
            id: Some(json!(1)),
            method: method.to_string(),
            params,
        }
    }

    /// 目标点原样透传给宿主，缺省 `targets` / `limit` 都是「没有」而不是 0。
    #[test]
    fn scene_verify_passes_targets_through() {
        let mut api = VerifyStub::default();
        let res = dispatch(
            &mut api,
            &request(
                "scene.verify",
                json!({ "targets": [{ "name": "chest", "at": [1.0, 2.0, 3.0] }] }),
            ),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        let seen = api.seen.expect("宿主应收到查询");
        assert_eq!(seen.targets.len(), 1);
        assert_eq!(seen.targets[0].name, "chest");
        assert_eq!(seen.targets[0].at, [1.0, 2.0, 3.0]);
        assert_eq!(seen.limit, None, "缺省 limit 表示不截断");
        assert_eq!(res.result.unwrap()["echo"][0]["name"], json!("chest"));
    }

    /// 空 `targets` 合法：那是「只做场景自洽体检」。
    #[test]
    fn scene_verify_accepts_empty_targets() {
        let mut api = VerifyStub::default();
        let res = dispatch(&mut api, &request("scene.verify", json!({ "targets": [] })));
        assert!(
            res.error.is_none(),
            "空目标表不该是参数错误: {:?}",
            res.error
        );
        assert_eq!(api.seen.expect("宿主应收到查询").targets.len(), 0);
    }

    /// 形状错的三连：`targets` 不是数组 / 缺 `name` / `at` 不是三元数组。
    /// 三种都必须回参数错误，且**不打扰宿主**。
    #[test]
    fn scene_verify_rejects_malformed_targets() {
        let cases = [
            json!({ "targets": "chest" }),
            json!({ "targets": [{ "at": [0.0, 0.0, 0.0] }] }),
            json!({ "targets": [{ "name": "   ", "at": [0.0, 0.0, 0.0] }] }),
            json!({ "targets": [{ "name": "chest", "at": [0.0, 0.0] }] }),
            json!({ "targets": [{ "name": "chest", "at": [0.0, "x", 0.0] }] }),
        ];
        for params in cases {
            let mut api = VerifyStub::default();
            let res = dispatch(&mut api, &request("scene.verify", params.clone()));
            let err = res.error.expect("形状错应报参数错误");
            assert_eq!(err.code, INVALID_PARAMS, "参数 {params} 的错误码不对");
            assert!(api.seen.is_none(), "参数非法时不应调用宿主：{params}");
        }
    }

    /// 缺省实现：协议方法声明了但宿主没点亮时，必须报「未实现」并点名方法。
    #[test]
    fn scene_verify_unsupported_by_default() {
        struct BareStub;
        impl EngineApi for BareStub {
            fn info(&mut self) -> EngineInfo {
                unimplemented!("本测试不查 info")
            }
        }

        impl NavApi for BareStub {}
        impl PrefabApi for BareStub {}
        let mut api = BareStub;
        let res = dispatch(&mut api, &request("scene.verify", json!({})));
        let err = res.error.expect("缺省实现应报未实现");
        assert_eq!(err.code, METHOD_NOT_FOUND);
        assert!(err.message.contains("scene.verify"), "错误消息应点名方法");
    }
}
