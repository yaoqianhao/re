//! TCP + NDJSON 传输层。
//!
//! 只暴露「一条连接上读到的一条请求」这一抽象：连接线程负责读行、解析、鉴权、
//! 写回；真正的 [`EngineApi`](crate::EngineApi) 调用发生在**主线程**，
//! 由主线程每帧 `try_recv` 取出 [`Job`] 并 dispatch。
//!
//! 这样做的代价是每条请求要过一次 channel；收益是引擎状态全程单线程可见，
//! 不需要给 `Scene` / `Physics` / 脚本宿主加锁，也不会出现「RPC 线程改场景、
//! 渲染线程正在读」的撕裂。
//!
//! # 资源边界
//!
//! 桥是纯本地开发期组件，但线程与内存同样必须有上限，否则一个 Bug 客户端就能
//! 把引擎拖死。本模块给出四条硬边界，超界一律**明确回错误**而不是静默堆积：
//!
//! 1. 待处理队列：`mpsc::sync_channel` 定容 [`JOB_QUEUE_CAPACITY`]，满了回 `SERVER_BUSY`；
//! 2. 并发连接：最多 [`MAX_CONNECTIONS`] 条，超出的连接写一条错误后立即关闭；
//! 3. 回包等待：`recv_timeout` 上限 [`REPLY_TIMEOUT`]，主线程卡死时不再无限悬挂线程；
//! 4. 读行：`BufRead::read_until` 在缓冲上找 `\n`，且用 `take` 卡在 [`MAX_LINE_BYTES`]。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::Value;

use crate::{
    dispatch, EngineApi, Request, Response, RpcFailure, INVALID_REQUEST, PARSE_ERROR, RPC_TIMEOUT,
    SERVER_BUSY, UNAUTHORIZED,
};

/// 单行上限：防止对端灌一个几 GB 的「一行」把桥撑爆。
const MAX_LINE_BYTES: usize = 1 << 20;

/// 待处理请求队列容量。引擎每帧只消化 `--rpc-max-per-frame` 条，队列比它大得多
/// 才能吸收突发；再大就只是把延迟藏起来，不如老实回 `SERVER_BUSY`。
const JOB_QUEUE_CAPACITY: usize = 64;

/// 同时在线连接上限。编辑器通常只要 1～2 条，16 条足够容错，又能挡住连接洪泛。
const MAX_CONNECTIONS: usize = 16;

/// 连接线程等待主线程回包的上限。主线程每帧都会 `try_recv`，正常情况是毫秒级；
/// 超过这个值说明引擎卡住或主循环已退出，宁可回错误也不让线程永久悬挂。
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// 单次读的超时。空闲连接不该一直占着一个线程的阻塞点，超时后回到循环检查停止标志。
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// 主线程收到的一条待执行请求；`reply` 用来把响应送回连接线程。
pub struct Job {
    pub request: Request,
    /// `None` 表示通知（不回包），此时接收端已被丢弃。
    pub reply: SyncSender<Option<Response>>,
}

impl Job {
    /// 执行并回包。`api` 由宿主提供，只在主线程调用。
    pub fn run(self, api: &mut dyn EngineApi) {
        let response = dispatch(api, &self.request);
        // 通知没有接收端，send 失败属正常
        let _ = self.reply.send(Some(response));
    }
}

/// 监听中的桥。生命周期：`bind` -> 主线程循环 `try_recv` -> `stop`。
pub struct RpcServer {
    addr: SocketAddr,
    jobs: Receiver<Job>,
    stopping: Arc<AtomicBool>,
    /// 当前在线连接数（accept 时 +1，连接线程结束时 -1）。
    active: Arc<AtomicUsize>,
    accept_thread: Option<JoinHandle<()>>,
    token: Option<String>,
}

impl RpcServer {
    /// 绑定 `127.0.0.1:port`（`port = 0` 表示让系统挑一个，测试用）。
    pub fn bind(port: u16, token: Option<String>) -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        let addr = listener.local_addr()?;
        listener.set_nonblocking(true)?;

        let (tx, jobs) = mpsc::sync_channel::<Job>(JOB_QUEUE_CAPACITY);
        let stopping = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicUsize::new(0));
        let flag = stopping.clone();
        let active_clone = active.clone();
        let token_clone = token.clone();
        let accept_thread = thread::Builder::new()
            .name("r3e-rpc-accept".into())
            .spawn(move || accept_loop(listener, tx, flag, active_clone, token_clone))?;

        Ok(Self {
            addr,
            jobs,
            stopping,
            active,
            accept_thread: Some(accept_thread),
            token,
        })
    }

    /// 实际监听地址（`--rpc-port 0` 时才知道真实端口）。
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// 非阻塞取一条：主线程每帧调用，配合 `--rpc-max-per-frame` 限流。
    pub fn try_recv(&self) -> Option<Job> {
        // `Empty` 与 `Disconnected` 对调用方同义：这一刻没有可执行的请求。
        self.jobs.try_recv().ok()
    }

    /// 阻塞取一条：给独立的「引擎线程」用（测试 / 无窗口宿主）。
    pub fn recv(&self) -> Option<Job> {
        self.jobs.recv().ok()
    }

    /// 当前在线连接数（用于观测与测试）。
    pub fn connections(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    /// 请求停止监听（连接线程会随后自然退出）。
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
    }

    /// 已配置的令牌（`None` 表示不鉴权）。
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }
}

impl Drop for RpcServer {
    fn drop(&mut self) {
        self.stop();
        if let Some(handle) = self.accept_thread.take() {
            let _ = handle.join();
        }
    }
}

/// 连接配额：拿到就代表占了一个并发位，`Drop` 时归还。
///
/// 用 RAII 而不是手工 `fetch_sub`，是为了让「线程创建失败」「读出错」「正常断开」
/// 等所有退出路径都自动归还，不会把计数器永久漏高。
struct ConnSlot {
    active: Arc<AtomicUsize>,
}

impl ConnSlot {
    fn acquire(active: &Arc<AtomicUsize>, max: usize) -> Option<Self> {
        let mut cur = active.load(Ordering::SeqCst);
        loop {
            if cur >= max {
                return None;
            }
            match active.compare_exchange(cur, cur + 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => {
                    return Some(Self {
                        active: active.clone(),
                    })
                }
                Err(actual) => cur = actual,
            }
        }
    }
}

impl Drop for ConnSlot {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

fn accept_loop(
    listener: TcpListener,
    tx: SyncSender<Job>,
    stopping: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    token: Option<String>,
) {
    log::info!(
        "编辑器桥监听 {}",
        listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "?".into())
    );
    while !stopping.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut stream, peer)) => {
                let Some(slot) = ConnSlot::acquire(&active, MAX_CONNECTIONS) else {
                    log::warn!("编辑器桥：连接数已达上限 {MAX_CONNECTIONS}，拒绝 {peer}");
                    let resp = Response::fail(
                        None,
                        RpcFailure::new(
                            SERVER_BUSY,
                            format!("连接数已达上限 {MAX_CONNECTIONS}，请稍后重试"),
                        ),
                    );
                    let _ = stream.set_write_timeout(Some(REPLY_TIMEOUT));
                    let _ = writeln_with_flush(&mut stream, &resp.to_line());
                    continue;
                };
                log::info!("编辑器桥：{peer} 已连接");
                let tx = tx.clone();
                let flag = stopping.clone();
                let token = token.clone();
                let spawned = thread::Builder::new()
                    .name(format!("r3e-rpc-{peer}"))
                    .spawn(move || {
                        // slot 移进线程，线程结束时自动归还连接位
                        let _slot = slot;
                        serve_connection(stream, tx, flag, token);
                    });
                if let Err(e) = spawned {
                    // slot 已随闭包丢弃，计数自动回退
                    log::warn!("编辑器桥：连接线程创建失败（{e}），断开 {peer}");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                log::warn!("编辑器桥 accept 失败: {e}");
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
    log::info!("编辑器桥监听已停止");
}

fn serve_connection(
    stream: TcpStream,
    tx: SyncSender<Job>,
    stopping: Arc<AtomicBool>,
    token: Option<String>,
) {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "?".into());
    // 读超时让空闲连接定期回到循环检查停止标志；写超时让半死连接不至于卡住线程。
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(REPLY_TIMEOUT));
    let mut reader = match stream.try_clone() {
        Ok(s) => BufReader::new(s),
        Err(e) => {
            log::warn!("编辑器桥：复制 socket 失败（{e}）");
            return;
        }
    };
    let mut writer = stream;
    let mut buf = Vec::new();

    loop {
        if stopping.load(Ordering::SeqCst) {
            break;
        }
        buf.clear();
        // `read_until` 在 BufReader 的缓冲上找 `\n`（一次系统调用覆盖多行），
        // 外层 `take` 把单行长度硬卡在上限，避免超长行把内存吃穿。
        let read = match read_line_limited(&mut reader, &mut buf) {
            Ok(0) => break, // EOF
            Ok(n) => n,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue; // 空闲连接：回去看停止标志
            }
            Err(e) => {
                log::debug!("编辑器桥：读 {peer} 失败（{e}）");
                break;
            }
        };
        if read > MAX_LINE_BYTES {
            let resp = Response::fail(
                None,
                RpcFailure::new(
                    INVALID_REQUEST,
                    format!("单行超过 {MAX_LINE_BYTES} 字节上限"),
                ),
            );
            let _ = writeln_with_flush(&mut writer, &resp.to_line());
            break;
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(response) = handle_line(line, &tx, token.as_deref()) else {
            continue; // 通知：无回包
        };
        if writeln_with_flush(&mut writer, &response.to_line()).is_err() {
            break;
        }
    }
    log::info!("编辑器桥：{peer} 已断开");
}

/// 解析一行并投递给主线程；返回 `None` 表示不回包。
fn handle_line(line: &str, tx: &SyncSender<Job>, token: Option<&str>) -> Option<Response> {
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(Response::fail(
                None,
                RpcFailure::new(PARSE_ERROR, format!("JSON 解析失败: {e}")),
            ))
        }
    };
    let request = match Request::from_value(value) {
        Ok(r) => r,
        Err(f) => return Some(Response::fail(None, f)),
    };

    let id = request.id.clone();
    if let Err(failure) = check_token(&request.params, token) {
        return Some(Response::fail(id, failure));
    }
    let (reply, waiter) = mpsc::sync_channel::<Option<Response>>(1);
    let is_notification = request.is_notification();
    // 用 `try_send` 而非阻塞 `send`：队列满时立刻回 `SERVER_BUSY`，让客户端自己退避，
    // 而不是把连接线程堵在这里、进而把连接数和内存一起拖涨。
    if let Err(e) = tx.try_send(Job { request, reply }) {
        let failure = match e {
            TrySendError::Full(_) => RpcFailure::new(
                SERVER_BUSY,
                "引擎请求队列已满（主线程消化不过来），请稍后重试",
            ),
            TrySendError::Disconnected(_) => {
                RpcFailure::new(INVALID_REQUEST, "引擎主循环已停止，桥不再受理请求")
            }
        };
        return Some(Response::fail(id, failure));
    }
    if is_notification {
        return None;
    }
    // 带超时等回包：主线程卡住 / 已退出时不会让连接线程永久悬挂。
    match waiter.recv_timeout(REPLY_TIMEOUT) {
        Ok(resp) => resp,
        Err(mpsc::RecvTimeoutError::Timeout) => Some(Response::fail(
            id,
            RpcFailure::new(
                RPC_TIMEOUT,
                format!(
                    "引擎 {} 毫秒内未回包（主线程卡顿或已退出）",
                    REPLY_TIMEOUT.as_millis()
                ),
            ),
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Some(Response::fail(
            id,
            RpcFailure::new(INVALID_REQUEST, "引擎未回包（主循环已退出）"),
        )),
    }
}

/// 鉴权放在主线程之外做，省得把错误令牌也排进队列。
pub fn check_token(params: &Value, token: Option<&str>) -> Result<(), RpcFailure> {
    let Some(expected) = token else {
        return Ok(());
    };
    match params.get("token").and_then(Value::as_str) {
        Some(got) if got == expected => Ok(()),
        Some(_) => Err(RpcFailure::new(UNAUTHORIZED, "令牌不正确")),
        None => Err(RpcFailure::new(
            UNAUTHORIZED,
            "缺少 `params.token`（启动时设置了 --rpc-token）",
        )),
    }
}

fn writeln_with_flush(writer: &mut TcpStream, line: &str) -> std::io::Result<()> {
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// 读到 `\n` 为止，返回读到的字节数（含换行）；超过上限时截停并返回该值。
///
/// 走 `BufRead::read_until`：数据由 `BufReader` 缓冲，一次 `read` 能覆盖多行，
/// 不再像逐字节版本那样每来一个字节就调一次 `read`。
fn read_line_limited(
    reader: &mut BufReader<TcpStream>,
    buf: &mut Vec<u8>,
) -> std::io::Result<usize> {
    let cap = (MAX_LINE_BYTES + 1) as u64;
    reader.by_ref().take(cap).read_until(b'\n', buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 连接配额：拿到 `max` 个之后必须拒绝，归还后又能拿到。
    #[test]
    fn conn_slot_caps_and_releases() {
        let active = Arc::new(AtomicUsize::new(0));
        let a = ConnSlot::acquire(&active, 2).expect("第 1 个应成功");
        let b = ConnSlot::acquire(&active, 2).expect("第 2 个应成功");
        assert!(ConnSlot::acquire(&active, 2).is_none(), "第 3 个应被拒");
        assert_eq!(active.load(Ordering::SeqCst), 2);
        drop(a);
        let c = ConnSlot::acquire(&active, 2).expect("归还后应能再拿");
        drop((b, c));
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }

    /// 队列满时 `try_send` 必须报 `Full`（对应 `SERVER_BUSY`），而不是阻塞。
    #[test]
    fn full_queue_is_reported_not_blocked() {
        let (tx, rx) = mpsc::sync_channel::<Job>(1);
        let make = |n: u64| {
            let req = Request {
                jsonrpc: crate::JSONRPC.to_string(),
                id: Some(serde_json::json!(n)),
                method: "engine.info".into(),
                params: Value::Null,
            };
            let (reply, _waiter) = mpsc::sync_channel::<Option<Response>>(1);
            Job {
                request: req,
                reply,
            }
        };
        tx.try_send(make(1)).expect("第一条应入队");
        match tx.try_send(make(2)) {
            Err(TrySendError::Full(_)) => {}
            Ok(_) => panic!("容量 1 的队列不该收下第二条"),
            Err(TrySendError::Disconnected(_)) => panic!("接收端还活着"),
        }
        // 腾出位置后应恢复
        let job = rx.try_recv().expect("应取到第一条");
        assert_eq!(job.request.id, Some(serde_json::json!(1)));
        tx.try_send(make(3)).expect("腾位后应能再入队");
    }

    /// `read_line_limited` 应一次读出完整一行，且不吞掉下一行的字节。
    #[test]
    fn read_line_limited_keeps_line_boundaries() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("绑定测试端口");
        let addr = listener.local_addr().expect("取端口");
        let writer = thread::spawn(move || {
            let mut s = TcpStream::connect(addr).expect("连接");
            s.write_all(b"{\"a\":1}\n{\"b\":2}\n").expect("写入两行");
            s.flush().expect("flush");
        });
        let (stream, _) = listener.accept().expect("接受连接");
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        let n1 = read_line_limited(&mut reader, &mut buf).expect("读第一行");
        assert_eq!(n1, 8);
        assert_eq!(String::from_utf8_lossy(&buf), "{\"a\":1}\n");
        buf.clear();
        let n2 = read_line_limited(&mut reader, &mut buf).expect("读第二行");
        assert_eq!(n2, 8);
        assert_eq!(String::from_utf8_lossy(&buf), "{\"b\":2}\n");
        writer.join().expect("写线程退出");
    }
}
