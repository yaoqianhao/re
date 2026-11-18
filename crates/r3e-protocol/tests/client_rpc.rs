//! `r3e_protocol::client` 的线路级测试：本地起一个假引擎，只验「线上字节」与
//! 「错误形态」这两件在进程内测不出来的事。
//!
//! 覆盖六条（方案 S5-A-1 步骤 2）：正常 `result` / `error` 回包 / 令牌注入 /
//! 连接被拒 / 超长行拒绝 / 半关闭连接；外加一条方法名归一化——它跨进程才看得出
//! 「客户端发的名字」与「引擎 match 的名字」是否一致。

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use r3e_protocol::client::{ClientError, Endpoint, RpcClient, MAX_LINE_BYTES};
use serde_json::{json, Value};

/// 假引擎：接受一条连接，逐行读请求、交给 `reply` 产出响应行，并把收到的请求
/// 全量回传给测试（断言令牌、方法名用）。
///
/// 监听端口用 `0` 让系统分配，避免和真实引擎抢 8787。
fn fake_engine<F>(reply: F) -> (String, Receiver<Vec<Value>>)
where
    F: Fn(&Value) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("测试端口");
    let addr = listener.local_addr().expect("本地地址").to_string();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut writer = stream.try_clone().expect("复制句柄");
        let mut reader = BufReader::new(stream);
        let mut seen = Vec::new();
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let Ok(req) = serde_json::from_str::<Value>(line.trim_end()) else {
                break;
            };
            let body = reply(&req);
            seen.push(req);
            if writer.write_all(body.as_bytes()).is_err() || writer.write_all(b"\n").is_err() {
                break;
            }
            let _ = writer.flush();
        }
        let _ = tx.send(seen);
    });
    (addr, rx)
}

/// 标准的成功回包。
fn ok_response(req: &Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": req["id"], "result": result }).to_string()
}

#[test]
fn call_returns_result_verbatim() {
    let (addr, seen) = fake_engine(|req| ok_response(req, json!({ "txId": "tx-1", "staged": 3 })));
    let mut client = RpcClient::connect(&Endpoint::new(addr)).expect("连上假引擎");
    let result = client.call("tx.begin", json!({})).expect("tx.begin 应成功");
    assert_eq!(result["txId"], "tx-1");
    assert_eq!(result["staged"], 3);

    drop(client); // 关连接 -> 假引擎读到 EOF 才收尾，否则 recv 会一直等
    let requests = seen.recv().expect("服务端应收到请求");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["method"], "tx.begin");
    assert_eq!(requests[0]["jsonrpc"], "2.0");
    assert_eq!(requests[0]["id"], 1);
}

#[test]
fn remote_error_keeps_code_and_message() {
    let (addr, _seen) = fake_engine(|req| {
        json!({
            "jsonrpc": "2.0",
            "id": req["id"],
            "error": { "code": r3e_protocol::TX_CONFLICT, "message": "基哈希已漂移" }
        })
        .to_string()
    });
    let mut client = RpcClient::connect(&Endpoint::new(addr)).expect("连上假引擎");
    let err = client
        .call("tx.commit", json!({ "txId": "t" }))
        .expect_err("必须报错");
    assert_eq!(err.code(), Some(i64::from(r3e_protocol::TX_CONFLICT)));
    match &err {
        ClientError::Remote { message, .. } => assert!(message.contains("基哈希"), "{message}"),
        other => panic!("应为 Remote，实得 {other:?}"),
    }
    // 业务错误不是「环境没准备好」——CLI 靠这条把退出码 1 与 3 分开。
    assert!(!err.is_environment());
}

#[test]
fn token_rides_in_params_not_header() {
    let (addr, seen) = fake_engine(|req| ok_response(req, json!({ "ok": true })));
    let endpoint = Endpoint::new(addr).with_token("dev");
    let mut client = RpcClient::connect(&endpoint).expect("连上假引擎");
    client.call("engine.info", json!({})).expect("应成功");

    drop(client);
    let requests = seen.recv().expect("服务端应收到请求");
    assert_eq!(requests[0]["params"]["token"], "dev");
}

#[test]
fn connect_refused_is_transport_error() {
    // 拿一个端口再放掉，下一次连接必然被拒（比写死端口稳）。
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("占位端口");
        listener.local_addr().expect("地址").port()
    };
    let err = match RpcClient::connect(&Endpoint::new(format!("127.0.0.1:{port}"))) {
        Ok(_) => panic!("端口已释放，不该连上"),
        Err(e) => e,
    };
    assert!(matches!(err, ClientError::Transport(_)), "{err:?}");
    assert!(err.is_environment());
    assert!(err.advice().contains("--rpc-port"), "{}", err.advice());
}

#[test]
fn oversized_line_is_rejected() {
    let (addr, _seen) = fake_engine(|_req| {
        // 一行纯填充，超过 1 MiB 上限；不该被当成 JSON 解析，也不该把内存吃穿。
        let mut body = String::with_capacity(MAX_LINE_BYTES as usize + 16);
        body.push('"');
        body.push_str(&"a".repeat(MAX_LINE_BYTES as usize + 8));
        body.push('"');
        body
    });
    let mut client = RpcClient::connect(&Endpoint::new(addr)).expect("连上假引擎");
    let err = client
        .call("engine.info", json!({}))
        .expect_err("应拒绝超长行");
    match err {
        ClientError::Transport(detail) => {
            assert!(detail.contains("上限"), "{detail}");
        }
        other => panic!("应为 Transport，实得 {other:?}"),
    }
}

#[test]
fn half_closed_connection_is_transport_error() {
    // 服务端 accept 后立刻关闭，不给任何字节。
    let listener = TcpListener::bind("127.0.0.1:0").expect("测试端口");
    let addr = listener.local_addr().expect("地址").to_string();
    thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            drop(stream);
        }
    });
    let mut client = RpcClient::connect(&Endpoint::new(addr)).expect("连接本身能建");
    let err = client
        .call("engine.info", json!({}))
        .expect_err("对端已关闭");
    match err {
        ClientError::Transport(detail) => {
            assert!(detail.contains("关闭"), "{detail}");
        }
        other => panic!("应为 Transport，实得 {other:?}"),
    }
}

#[test]
fn method_name_is_canonicalised_before_sending() {
    let (addr, seen) = fake_engine(|req| ok_response(req, json!({ "ok": true })));
    let mut client = RpcClient::connect(&Endpoint::new(addr)).expect("连上假引擎");
    // 驼峰 + 下划线两种写法都必须落到引擎唯一认得的形态上。
    client.call("scene.setTransform", json!({})).expect("驼峰");
    client
        .call("scene.set_transform", json!({ "node": "hero" }))
        .expect("下划线");

    drop(client);
    let requests = seen.recv().expect("服务端应收到请求");
    assert_eq!(requests[0]["method"], "scene.settransform");
    assert_eq!(requests[1]["method"], "scene.settransform");
}

#[test]
fn sequential_calls_use_increasing_ids() {
    let (addr, seen) = fake_engine(|req| ok_response(req, json!({ "id": req["id"] })));
    let mut client = RpcClient::connect(&Endpoint::new(addr)).expect("连上假引擎");
    for _ in 0..3 {
        client.call("ping", json!({})).expect("ping");
    }
    drop(client);
    let requests = seen.recv().expect("服务端应收到请求");
    let ids: Vec<Value> = requests.iter().map(|r| r["id"].clone()).collect();
    assert_eq!(ids, vec![json!(1), json!(2), json!(3)]);
}
