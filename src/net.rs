//! Сетевая часть MindaRust: подключение к ванильному серверу Mindustry 160.8.
//! Работает в отдельном потоке и отдаёт события окну через mpsc.

use std::io::{self, Write};
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::wire::*;
use crate::world::{parse_world, World};

const FRAMEWORK_REGISTER_UDP: u8 = 3;
const FRAMEWORK_REGISTER_TCP: u8 = 4;
const FRAMEWORK_KEEP_ALIVE: u8 = 2;
const FRAMEWORK_PING: u8 = 0;

const ID_STREAM_BEGIN: u8 = 0;
const ID_STREAM_CHUNK: u8 = 1;
const ID_WORLD_STREAM: u8 = 2;
const ID_CONNECT: u8 = 3;
const ID_CLIENT_SNAPSHOT: u8 = 29;
const ID_CONNECT_CONFIRM: u8 = 34;
const ID_KICK: u8 = 65;
const ID_KICK_2: u8 = 66;
const ID_SEND_CHAT: u8 = 105; // SendChatMessageCallPacket
const ID_SEND_MESSAGE: u8 = 106; // SendMessageCallPacket
const ID_SEND_MESSAGE2: u8 = 107; // SendMessageCallPacket2 (message, unformatted, playerID)

const VERSION_BUILD: i32 = 160;
const VERSION_TYPE: &str = "official";
const LOCALE: &str = "en";

/// Состояние окна.
#[derive(Debug, Clone, PartialEq)]
pub enum NetState {
    Menu,
    Loading,
    Playing,
    Error(String),
}

#[derive(Debug)]
pub enum NetEvent {
    Status(String),
    Progress(f32),
    World(World),
    Chat(String),
    Kick(String),
    Error(String),
    Connected,
}

struct State {
    tcp_id: Option<i32>,
    udp_ok: bool,
    kick: Option<String>,
    packets: u64,
    world_received: bool,
    stream: Option<(i32, usize, Vec<u8>)>,
}

fn gen_bytes(mut seed: u64, n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        out.push(seed as u8);
    }
    out
}

/// Возвращает постоянные 8 байт UUID и USID (base64), чтобы сервер не банил
/// за частую смену ID. Файл лежит рядом с exe.
fn load_or_create_identity() -> (Vec<u8>, String) {
    let path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("identity.bin")));
    if let Some(ref p) = path {
        if let Ok(bytes) = std::fs::read(p) {
            if bytes.len() >= 16 {
                return (bytes[..8].to_vec(), b64_encode(&bytes[8..16]));
            }
        }
    }

    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
        ^ ((std::process::id() as u64) << 32);
    let uuid_bytes = gen_bytes(seed, 8);
    let usid_bytes = gen_bytes(seed ^ 0x9e37_79b9_7f4a_7c15, 8);
    if let Some(ref p) = path {
        let mut all = Vec::with_capacity(16);
        all.extend_from_slice(&uuid_bytes);
        all.extend_from_slice(&usid_bytes);
        let _ = std::fs::write(p, all);
    }
    (uuid_bytes, b64_encode(&usid_bytes))
}

fn build_connect(name: &str, usid: &str, uuid_bytes: &[u8]) -> Vec<u8> {
    let mut p = Vec::new();
    put_i32(&mut p, VERSION_BUILD);
    put_typeio_string(&mut p, VERSION_TYPE);
    put_typeio_string(&mut p, name);
    put_typeio_string(&mut p, LOCALE);
    put_typeio_string(&mut p, usid);
    p.extend_from_slice(uuid_bytes);
    put_i64(&mut p, crc32(uuid_bytes) as i64);
    put_bool(&mut p, false);
    put_i32(&mut p, -1);
    p.push(0);
    p
}

fn build_snapshot(snapshot_id: i32) -> Vec<u8> {
    let mut p = Vec::new();
    put_i32(&mut p, snapshot_id);
    put_i32(&mut p, -1);
    put_bool(&mut p, true);
    for _ in 0..8 {
        put_f32(&mut p, 0.0);
    }
    put_i32(&mut p, -1);
    for _ in 0..4 {
        put_bool(&mut p, false);
    }
    put_i16(&mut p, -1);
    put_i32(&mut p, 0);
    put_i32(&mut p, -1);
    for _ in 0..4 {
        put_f32(&mut p, 0.0);
    }
    p
}

fn emit(tx: &Sender<NetEvent>, event: NetEvent) {
    let _ = tx.send(event);
}

/// Разобрать собранный поток мира и сообщить окну.
fn finish_world(state: &Arc<Mutex<State>>, tx: &Sender<NetEvent>, compressed: &[u8]) {
    state.lock().unwrap().world_received = true;
    match parse_world(compressed) {
        Ok(world) => {
            emit(tx, NetEvent::Status(format!("Мир загружен: {}x{}", world.width, world.height)));
            emit(tx, NetEvent::World(world));
            emit(tx, NetEvent::Connected);
        }
        Err(e) => {
            let _ = std::fs::write("world-error.txt", format!("{}\n", e));
            emit(tx, NetEvent::Error(format!("World parse failed: {}", e)));
        }
    }
}

/// Запустить подключение в фоновом потоке.
pub fn start(host: String, port: u16, name: String, tx: Sender<NetEvent>, rx: Receiver<String>) {
    thread::spawn(move || {
        let addr = match format!("{}:{}", host, port).to_socket_addrs() {
            Ok(mut it) => match it.next() {
                Some(a) => a,
                None => {
                    emit(&tx, NetEvent::Error("адрес не найден".into()));
                    return;
                }
            },
            Err(e) => {
                emit(&tx, NetEvent::Error(format!("не удалось разрешить адрес: {}", e)));
                return;
            }
        };

        emit(&tx, NetEvent::Status(format!("Подключаюсь к {}:{} ...", host, port)));
        let stream = match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
            Ok(s) => s,
            Err(e) => {
                emit(&tx, NetEvent::Error(format!("TCP-подключение не удалось: {}", e)));
                return;
            }
        };
        stream.set_read_timeout(Some(Duration::from_millis(100))).ok();

        let (uuid_bytes, usid) = load_or_create_identity();

        let state = Arc::new(Mutex::new(State {
            tcp_id: None,
            udp_ok: false,
            kick: None,
            packets: 0,
            world_received: false,
            stream: None,
        }));
        let writer = Arc::new(Mutex::new(stream.try_clone().expect("клон TCP")));

        // --- Поток чтения TCP ---
        {
            let state = Arc::clone(&state);
            let writer = Arc::clone(&writer);
            let tx = tx.clone();
            thread::spawn(move || {
                let mut reader = stream;
                loop {
                    match read_tcp_frame(&mut reader) {
                        Ok(body) => {
                            let inb = match parse_body(&body) {
                                Some(v) => v,
                                None => continue,
                            };
                            match inb.framework {
                                Some(FRAMEWORK_REGISTER_TCP) => {
                                    let mut pos = 0;
                                    if let Some(id) = get_i32(&inb.extra, &mut pos) {
                                        state.lock().unwrap().tcp_id = Some(id);
                                    }
                                }
                                Some(FRAMEWORK_REGISTER_UDP) => {
                                    state.lock().unwrap().udp_ok = true;
                                }
                                Some(FRAMEWORK_PING) => {
                                    let mut extra = inb.extra.clone();
                                    if extra.len() >= 5 && extra[4] == 0 {
                                        extra[4] = 1;
                                        let frame = tcp_framework_frame(FRAMEWORK_PING, &extra);
                                        let mut w = writer.lock().unwrap();
                                        let _ = w.write_all(&frame);
                                    }
                                }
                                Some(_) => {}
                                None => match inb.id {
                                    ID_KICK | ID_KICK_2 => {
                                        let mut pos = 0;
                                        let reason = get_typeio_string(&inb.data, &mut pos)
                                            .unwrap_or_else(|| "без причины".into());
                                        state.lock().unwrap().kick = Some(reason.clone());
                                        emit(&tx, NetEvent::Kick(reason));
                                    }
                                    ID_STREAM_BEGIN => {
                                        let mut pos = 0;
                                        let id = get_i32(&inb.data, &mut pos);
                                        let total = get_i32(&inb.data, &mut pos);
                                        let kind = get_u8(&inb.data, &mut pos);
                                        if let (Some(id), Some(total), Some(2)) = (id, total, kind) {
                                            let mut st = state.lock().unwrap();
                                            st.stream = Some((id, total as usize, Vec::new()));
                                            emit(&tx, NetEvent::Status(format!(
                                                "Получаю мир: {} байт", total
                                            )));
                                        }
                                    }
                                    ID_STREAM_CHUNK => {
                                        let mut pos = 0;
                                        let id = get_i32(&inb.data, &mut pos);
                                        let len = get_i16(&inb.data, &mut pos);
                                        if let (Some(id), Some(len)) = (id, len) {
                                            if len >= 0 && pos + len as usize <= inb.data.len() {
                                                let bytes = &inb.data[pos..pos + len as usize];
                                                let mut complete = None;
                                                {
                                                    let mut st = state.lock().unwrap();
                                                    if let Some((sid, total, buf)) = &mut st.stream {
                                                        if *sid == id {
                                                            buf.extend_from_slice(bytes);
                                                            let progress = if *total == 0 {
                                                                1.0
                                                            } else {
                                                                buf.len() as f32 / *total as f32
                                                            };
                                                            emit(&tx, NetEvent::Progress(progress));
                                                            if buf.len() >= *total {
                                                                complete = Some(buf.clone());
                                                                st.stream = None;
                                                            }
                                                        }
                                                    }
                                                }
                                                if let Some(compressed) = complete {
                                                    finish_world(&state, &tx, &compressed);
                                                }
                                            }
                                        }
                                    }
                                    ID_WORLD_STREAM => {
                                        let bytes = {
                                            let mut st = state.lock().unwrap();
                                            st.stream.take().map(|(_, _, b)| b)
                                        };
                                        if let Some(compressed) = bytes {
                                            finish_world(&state, &tx, &compressed);
                                        }
                                    }
                                    ID_SEND_MESSAGE => {
                                        let mut pos = 0;
                                        let text = get_typeio_string(&inb.data, &mut pos);
                                        if let Some(text) = text {
                                            emit(&tx, NetEvent::Chat(text));
                                        }
                                    }
                                    ID_SEND_MESSAGE2 => {
                                        // Современные серверы шлют чат через SendMessageCallPacket2:
                                        // message, unformatted, playerID (i32).
                                        let mut pos = 0;
                                        if let Some(message) = get_typeio_string(&inb.data, &mut pos) {
                                            emit(&tx, NetEvent::Chat(message));
                                        }
                                    }
                                    _ => {
                                        state.lock().unwrap().packets += 1;
                                    }
                                },
                            }
                        }
                        Err(e)
                            if e.kind() == io::ErrorKind::WouldBlock
                                || e.kind() == io::ErrorKind::TimedOut => {}
                        Err(e) => {
                            emit(&tx, NetEvent::Error(format!("соединение закрыто: {}", e)));
                            break;
                        }
                    }
                }
            });
        }

        // --- Ждём RegisterTCP ---
        let conn_id = loop {
            {
                let st = state.lock().unwrap();
                if let Some(id) = st.tcp_id {
                    break id;
                }
                if let Some(reason) = &st.kick {
                    emit(&tx, NetEvent::Kick(reason.clone()));
                    return;
                }
            }
            thread::sleep(Duration::from_millis(20));
        };

        // --- UDP ---
        let udp = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(e) => {
                emit(&tx, NetEvent::Error(format!("не удалось создать UDP: {}", e)));
                return;
            }
        };
        udp.connect(addr).ok();
        let mut reg_extra = Vec::new();
        put_i32(&mut reg_extra, conn_id);
        let reg_udp = framework_body(FRAMEWORK_REGISTER_UDP, &reg_extra);
        let mut last_keepalive = Instant::now() - Duration::from_secs(10);

        loop {
            {
                let st = state.lock().unwrap();
                if st.udp_ok {
                    break;
                }
                if let Some(reason) = &st.kick {
                    emit(&tx, NetEvent::Kick(reason.clone()));
                    return;
                }
            }
            let _ = udp.send(&reg_udp);
            if last_keepalive.elapsed() >= Duration::from_secs(5) {
                let frame = tcp_framework_frame(FRAMEWORK_KEEP_ALIVE, &[]);
                let mut w = writer.lock().unwrap();
                let _ = w.write_all(&frame);
                last_keepalive = Instant::now();
            }
            thread::sleep(Duration::from_millis(100));
        }

        // --- ConnectPacket ---
        let connect = build_connect(&name, &usid, &uuid_bytes);
        {
            let mut w = writer.lock().unwrap();
            if w.write_all(&tcp_frame(ID_CONNECT, &connect)).is_err() {
                emit(&tx, NetEvent::Error("не удалось отправить ConnectPacket".into()));
                return;
            }
            w.flush().ok();
        }

        // --- Основной цикл ---
        let mut last_keepalive = Instant::now();
        let mut last_snapshot = Instant::now();
        let mut snapshot_id: i32 = 0;
        let mut confirm_sent = false;

        loop {
            {
                let st = state.lock().unwrap();
                if let Some(reason) = &st.kick {
                    emit(&tx, NetEvent::Kick(reason.clone()));
                    return;
                }
            }

            // Исходящий чат из окна.
            match rx.try_recv() {
                Ok(text) => {
                    if !text.is_empty() {
                        let mut payload = Vec::new();
                        put_typeio_string(&mut payload, &text);
                        let mut w = writer.lock().unwrap();
                        let _ = w.write_all(&tcp_frame(ID_SEND_CHAT, &payload));
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }

            if !confirm_sent && state.lock().unwrap().world_received {
                let mut w = writer.lock().unwrap();
                if w.write_all(&tcp_frame(ID_CONNECT_CONFIRM, &[])).is_ok() {
                    confirm_sent = true;
                    emit(&tx, NetEvent::Status("ConnectConfirm отправлен".into()));
                }
            }

            if last_keepalive.elapsed() >= Duration::from_secs(5) {
                let frame = tcp_framework_frame(FRAMEWORK_KEEP_ALIVE, &[]);
                let mut w = writer.lock().unwrap();
                if w.write_all(&frame).is_err() {
                    emit(&tx, NetEvent::Error("keepalive не отправлен".into()));
                    return;
                }
                last_keepalive = Instant::now();
            }

            if last_snapshot.elapsed() >= Duration::from_millis(66) {
                let payload = build_snapshot(snapshot_id);
                let _ = udp.send(&packet_body(ID_CLIENT_SNAPSHOT, &payload));
                snapshot_id = snapshot_id.wrapping_add(1);
                last_snapshot = Instant::now();
            }

            thread::sleep(Duration::from_millis(8));
        }
    });
}
