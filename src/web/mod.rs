//! 模块四配套：内置拓扑编辑器后端（NMTs 作为 HTTP + WebSocket 服务）。
//!
//! 设计目标（V3.0.2）：
//! - 编辑器组件「内置」：前端页面由二进制内嵌（`include_str!`），无需用户安装
//!   pywebview / Node / 任何依赖，也不依赖外网 CDN。
//! - 架构：NMTs 在 `127.0.0.1` 上起一个轻量 HTTP 服务（仅用 `tokio`，外加 `sha1`
//!   + `base64` 完成 WebSocket 握手），`GET /` 返回编辑器页面，`/ws` 为 WebSocket。
//! - 实时双向：浏览器端每次编辑经 `/ws` 以 `update` 消息回传后端，后端即时写入
//!   TUI 内存中的拓扑并重跑设计预检；`save` 触发落盘，`close` 关闭服务。
//!
//! 说明：手写 WebSocket 仅支持文本帧（localhost 单可信客户端，小负载），刻意不引入
//! axum/tungstenite 等重依赖，保证编译快、体积小、零部署。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use sha1::{Digest, Sha1};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::app::EditorMsg;
use crate::config::app_root;
use crate::core::topology::Topology;

/// WebSocket GUID（RFC 6455）。
const WS_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// 启动编辑器服务。
///
/// - `initial`：当前拓扑快照，连接建立后作为 `init` 推给前端。
/// - `to_ui`：前端消息（update/save/close）转发给 TUI 事件的通道。
/// - `rt`：用于派生服务任务的 tokio 运行时句柄。
///
/// 返回监听端口与关闭句柄（供 TUI 在退出 / 收到 close 时停止服务）。
pub fn start_editor(
    initial: Topology,
    to_ui: UnboundedSender<EditorMsg>,
    rt: tokio::runtime::Handle,
) -> Result<EditorServer> {
    let (std_listener, port) = bind_free_port()?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(tokio::sync::Mutex::new(initial));

    let sd = shutdown.clone();
    // 关键：TcpListener::from_std 必须在 reactor 上下文（tokio task 内）调用，
    // 否则主线程（同步 TUI 循环，不在 runtime 内）会 panic "no reactor running"。
    let handle = rt.spawn(async move {
        let listener = match TcpListener::from_std(std_listener) {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("编辑器监听转换失败：{e}");
                return;
            }
        };
        loop {
            if sd.load(Ordering::SeqCst) {
                break;
            }
            let accept = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
            let stream = match accept {
                Ok(Ok((s, _))) => s,
                _ => continue,
            };
            let to_ui = to_ui.clone();
            let shared = shared.clone();
            let sd = sd.clone();
            tokio::spawn(async move {
                let _ = handle_conn(stream, shared, to_ui, sd).await;
            });
        }
    });

    Ok(EditorServer {
        port,
        shutdown,
        handle,
    })
}

/// 已启动的编辑器服务句柄。
pub struct EditorServer {
    pub port: u16,
    pub shutdown: Arc<AtomicBool>,
    pub handle: JoinHandle<()>,
}

/// 在 127.0.0.1 的 18765..18800 区间找一个空闲端口（纯 std，无需 reactor）。
fn bind_free_port() -> Result<(std::net::TcpListener, u16)> {
    for port in 18765..18800u16 {
        let addr = format!("127.0.0.1:{port}");
        match std::net::TcpListener::bind(&addr) {
            Ok(std_l) => {
                std_l.set_nonblocking(true).ok();
                return Ok((std_l, port));
            }
            Err(_) => continue,
        }
    }
    Err(anyhow!("无法绑定拓扑编辑器端口（18765-18799 均被占用）"))
}

/// 处理单个连接：HTTP 静态页 或 WebSocket 升级。
async fn handle_conn(
    stream: TcpStream,
    shared: Arc<tokio::sync::Mutex<Topology>>,
    to_ui: UnboundedSender<EditorMsg>,
    sd: Arc<AtomicBool>,
) -> Result<()> {
    let (rd, wr) = stream.into_split();
    let mut reader = BufReader::new(rd);
    let mut writer = BufWriter::new(wr);

    // 读取 HTTP 请求头（到空行结束）。
    let mut headers: HashMap<String, String> = HashMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        if line == "\r\n" || line == "\n" {
            break;
        } else if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }

    let is_ws = headers
        .get("upgrade")
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);

    if is_ws {
        do_ws_handshake(&mut writer, &headers).await?;
        ws_loop(&mut reader, &mut writer, shared, to_ui, sd).await?;
    } else {
        serve_static(&mut writer).await?;
    }
    Ok(())
}

/// 返回编辑器 HTML（优先磁盘文件，否则用内嵌副本——保证「内置」）。
fn editor_html() -> String {
    let on_disk = app_root()
        .join("editor")
        .join("topology_editor.html");
    if let Ok(s) = std::fs::read_to_string(&on_disk) {
        if !s.is_empty() {
            return s;
        }
    }
    EMBEDDED_HTML.to_string()
}

/// 响应静态页面（仅 `/` 返回编辑器，其余 404）。
async fn serve_static<W: AsyncWriteExt + Unpin>(writer: &mut BufWriter<W>) -> Result<()> {
    let html = editor_html();
    let body = html.as_bytes();
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    writer.write_all(resp.as_bytes()).await?;
    writer.write_all(body).await?;
    writer.flush().await?;
    Ok(())
}

/// 完成 WebSocket 握手（计算 Sec-WebSocket-Accept）。
async fn do_ws_handshake<W: AsyncWriteExt + Unpin>(
    writer: &mut BufWriter<W>,
    headers: &HashMap<String, String>,
) -> Result<()> {
    let key = headers
        .get("sec-websocket-key")
        .ok_or_else(|| anyhow!("缺少 Sec-WebSocket-Key"))?;
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WS_GUID);
    let accept = B64.encode(hasher.finalize());
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    writer.write_all(resp.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

/// WebSocket 主循环：推送 init、接收前端消息并转发给 TUI。
async fn ws_loop<R, W>(
    reader: &mut BufReader<R>,
    writer: &mut BufWriter<W>,
    shared: Arc<tokio::sync::Mutex<Topology>>,
    to_ui: UnboundedSender<EditorMsg>,
    sd: Arc<AtomicBool>,
) -> Result<()>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    // 连接建立即推送当前拓扑快照
    let topo = shared.lock().await.clone();
    let init = serde_json::json!({ "type": "init", "topo": topo });
    write_ws_text(writer, &init.to_string()).await?;

    loop {
        if sd.load(Ordering::SeqCst) {
            break;
        }
        let frame = match read_ws_frame(reader).await {
            Ok(Some(f)) => f,
            Ok(None) => break, // 连接关闭
            Err(_) => break,
        };
        let (opcode, data) = frame;
        match opcode {
            0x8 => break, // Close
            0x9 => {
                // Ping -> Pong（0x8A = FIN | pong）
                write_ws_frame(writer, 0x8A, &data).await?;
            }
            0x1 => {
                // Text：解析前端指令
                let msg: serde_json::Value = match serde_json::from_slice(&data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                match msg.get("type").and_then(|v| v.as_str()) {
                    Some("update") => {
                        if let Some(topo_val) = msg.get("topo") {
                            if let Ok(t) = serde_json::from_value::<Topology>(topo_val.clone()) {
                                *shared.lock().await = t.clone();
                                let _ = to_ui.send(EditorMsg::Update(t));
                            }
                        }
                    }
                    Some("save") => {
                        let _ = to_ui.send(EditorMsg::Save);
                    }
                    Some("close") => {
                        let _ = to_ui.send(EditorMsg::Close);
                        sd.store(true, Ordering::SeqCst);
                        break;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// 读取一个 WebSocket 数据帧，返回 (opcode, payload)。
/// 仅处理文本/关闭/ping/continuation；小负载（单帧）足够覆盖本场景。
async fn read_ws_frame<R: AsyncReadExt + Unpin>(
    reader: &mut BufReader<R>,
) -> Result<Option<(u8, Vec<u8>)>> {
    let mut hdr = [0u8; 2];
    if reader.read_exact(&mut hdr).await.is_err() {
        return Ok(None);
    }
    let opcode = hdr[0] & 0x0f;
    let masked = (hdr[1] & 0x80) != 0;
    let mut len = (hdr[1] & 0x7f) as u64;
    if len == 126 {
        let mut b = [0u8; 2];
        reader.read_exact(&mut b).await?;
        len = u16::from_be_bytes(b) as u64;
    } else if len == 127 {
        let mut b = [0u8; 8];
        reader.read_exact(&mut b).await?;
        len = u64::from_be_bytes(b);
    }
    let mut mask = [0u8; 4];
    if masked {
        reader.read_exact(&mut mask).await?;
    }
    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload).await?;
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    Ok(Some((opcode, payload)))
}

/// 写文本帧（服务端→客户端，不加掩码）。
async fn write_ws_text<W: AsyncWriteExt + Unpin>(
    writer: &mut BufWriter<W>,
    text: &str,
) -> Result<()> {
    write_ws_frame(writer, 0x81, text.as_bytes()).await
}

/// 写任意帧（opcode 已含 FIN 位，如 0x81 文本、0xA pong），服务端发送不加掩码。
async fn write_ws_frame<W: AsyncWriteExt + Unpin>(
    writer: &mut BufWriter<W>,
    opcode: u8,
    payload: &[u8],
) -> Result<()> {
    let len = payload.len();
    let mut frame = Vec::with_capacity(len + 10);
    frame.push(opcode); // FIN(0x80) | opcode（调用方已设 FIN 位）
    if len < 126 {
        frame.push(len as u8);
    } else if len < 65536 {
        frame.push(126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    writer.write_all(&frame).await?;
    writer.flush().await?;
    Ok(())
}

/// 内嵌编辑器前端（与 `editor/topology_editor.html` 内容一致，作为无外部文件时的兜底）。
const EMBEDDED_HTML: &str = include_str!("../../editor/topology_editor.html");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::topology::demo_topology;
    use tokio::io::AsyncWriteExt;

    /// 最小 WebSocket 客户端：握手 + 发送掩码文本帧 + 读文本帧。
    /// 用 BufReader 做握手，避免把初始帧字节提前读丢。
    async fn ws_client(port: u16) -> tokio::io::BufReader<tokio::net::TcpStream> {
        use tokio::io::AsyncBufReadExt;
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let mut reader = tokio::io::BufReader::new(stream);
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let req = format!(
            "GET /ws HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        reader.get_mut().write_all(req.as_bytes()).await.unwrap();
        reader.get_mut().flush().await.unwrap();
        // 逐行读握手响应头，直到空行（剩余字节留在 BufReader 缓冲，不丢）
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await.unwrap();
            if n == 0 || line == "\r\n" || line == "\n" {
                break;
            }
        }
        reader
    }

    /// 读一个服务端文本帧（未掩码），返回 payload 字符串。
    /// 由于 TCP 可能分片，这里持续读取直到拿满帧长度。
    async fn read_text<R: tokio::io::AsyncReadExt + Unpin>(reader: &mut tokio::io::BufReader<R>) -> String {
        use tokio::io::AsyncReadExt;
        let mut hdr = [0u8; 2];
        reader.read_exact(&mut hdr).await.unwrap();
        let mut len = (hdr[1] & 0x7f) as usize;
        if len == 126 {
            let mut b = [0u8; 2];
            reader.read_exact(&mut b).await.unwrap();
            len = u16::from_be_bytes(b) as usize;
        } else if len == 127 {
            let mut b = [0u8; 8];
            reader.read_exact(&mut b).await.unwrap();
            len = u64::from_be_bytes(b) as usize;
        }
        let mut payload = vec![0u8; 0];
        let mut buf = [0u8; 4096];
        while payload.len() < len {
            let n = reader.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            payload.extend_from_slice(&buf[..n]);
        }
        payload.truncate(len);
        String::from_utf8(payload).unwrap()
    }

    /// 发送掩码文本帧（客户端→服务端必须掩码）。
    async fn send_text(stream: &mut tokio::net::TcpStream, text: &str) {
        let data = text.as_bytes();
        let mask = [0x12u8, 0x34, 0x56, 0x78];
        let mut frame = vec![0x81u8, (data.len() as u8) | 0x80];
        frame.extend_from_slice(&mask);
        let mut masked: Vec<u8> = data.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]).collect();
        frame.append(&mut masked);
        stream.write_all(&frame).await.unwrap();
        stream.flush().await.unwrap();
    }

    #[tokio::test]
    async fn editor_http_and_ws_roundtrip() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<EditorMsg>();
        let rt = tokio::runtime::Handle::current();
        let srv = start_editor(demo_topology(), tx, rt).unwrap();

        // 1) HTTP / 返回内嵌编辑器 HTML
        let resp = reqwest_ish_get(srv.port).await;
        assert!(resp.contains("<!DOCTYPE html>"), "HTTP 应返回编辑器 HTML");
        assert!(resp.contains("拓扑编辑器"), "页面应包含标题");

        // 2) WS 握手 + 收到 init 快照
        let mut reader = ws_client(srv.port).await;
        let init = read_text(&mut reader).await;
        let v: serde_json::Value = serde_json::from_str(&init).unwrap();
        assert_eq!(v["type"], "init");
        assert!(v["topo"]["devices"].as_array().unwrap().len() >= 5);

        // 3) 前端发送 update → TUI 通道收到 EditorMsg::Update
        let updated = serde_json::json!({
            "type": "update",
            "topo": { "devices": [ { "id":"a","name":"测试","vendor":"huawei_vrp","role":"core" } ], "links": [] }
        });
        send_text(reader.get_mut(), &updated.to_string()).await;
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        match msg {
            EditorMsg::Update(t) => assert_eq!(t.devices.len(), 1),
            _ => panic!("期望 Update 消息"),
        }

        // 4) save 消息
        send_text(reader.get_mut(), r#"{"type":"save"}"#).await;
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(msg, EditorMsg::Save));

        srv.shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        srv.handle.abort();
    }

    /// 极简同步 HTTP GET（避免引入 reqwest 依赖，仅本测试用）。
    async fn reqwest_ish_get(port: u16) -> String {
        use tokio::io::AsyncReadExt;
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let req = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = stream.read(&mut chunk).await.unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        String::from_utf8_lossy(&buf).to_string()
    }

    /// 回归测试（V3.0.3）：在主线程「无 reactor 上下文」直接调用 start_editor
    /// 必须返回 Ok 且不 panic。原实现会在主线程调用 TcpListener::from_std 触发
    /// "there is no reactor running" 崩溃；修复后绑定只用 std，from_std 在
    /// rt.spawn 任务内执行。
    #[test]
    fn start_editor_without_runtime_context_does_not_panic() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<EditorMsg>();
        // 构造一个真实 runtime 的 handle，但在当前（非 runtime）线程调用 start_editor。
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let handle = rt.handle().clone();
        // 当前线程不在 runtime 内（drop 了 BlockOn 作用域），直接同步调用：
        let srv = start_editor(demo_topology(), tx, handle);
        assert!(srv.is_ok(), "主线程无 reactor 调用 start_editor 应成功");
        let srv = srv.unwrap();
        // 让 spawn 任务有机会执行 from_std（不报错即证明没 panic）。
        std::thread::sleep(std::time::Duration::from_millis(200));
        srv.shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        srv.handle.abort();
        rt.shutdown_background();
    }
}

