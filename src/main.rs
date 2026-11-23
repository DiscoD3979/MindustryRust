//! MindaRust — клиент с окном для обычных серверов Mindustry 160.8.
//! Рисует мир настоящими спрайтами из ассетов игры, показывает чат и FPS.

mod atlas;
mod blocks;
mod entities;
mod net;
mod wire;
mod world;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use macroquad::prelude::*;
use macroquad::window::Conf;

use atlas::{Atlas, Region};
use entities::{PlayerState, UnitState};
use net::{ClientInput, NetEvent, NetState};
use world::World;

fn window_conf() -> Conf {
    Conf {
        window_title: "MindaRust".into(),
        window_width: 1280,
        window_height: 720,
        window_resizable: true,
        platform: macroquad::miniquad::conf::Platform {
            // Без ограничения герцовкой: FPS не привязан к монитору.
            swap_interval: Some(0),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Ищет файл либо рядом с exe, либо в корне проекта (target/release/../..).
fn asset_path(rel: &str) -> String {
    let mut candidates = vec![PathBuf::from(rel)];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join(rel));
            candidates.push(parent.join("../..").join(rel));
        }
    }
    for c in candidates {
        if c.exists() {
            return c.to_string_lossy().to_string();
        }
    }
    rel.to_string()
}

struct TextField {
    label: &'static str,
    text: String,
    rect: Rect,
}

impl TextField {
    fn new(label: &'static str, text: &str, x: f32, y: f32, w: f32) -> Self {
        Self {
            label,
            text: text.to_string(),
            rect: Rect::new(x, y, w, 38.0),
        }
    }

    fn draw(&self, font: Option<&Font>, focused: bool) {
        draw_text_ex(
            self.label,
            self.rect.x,
            self.rect.y - 6.0,
            TextParams {
                font,
                font_size: 16,
                color: Color::new(0.65, 0.72, 0.80, 1.0),
                ..Default::default()
            },
        );
        draw_rectangle(
            self.rect.x,
            self.rect.y,
            self.rect.w,
            self.rect.h,
            Color::new(0.10, 0.12, 0.16, 0.95),
        );
        draw_rectangle_lines(
            self.rect.x,
            self.rect.y,
            self.rect.w,
            self.rect.h,
            2.0,
            if focused {
                Color::new(0.30, 0.70, 1.00, 1.0)
            } else {
                Color::new(0.25, 0.30, 0.38, 1.0)
            },
        );
        let mut shown = self.text.clone();
        if focused && (get_time() * 2.0) as u64 % 2 == 0 {
            shown.push('|');
        }
        draw_text_ex(
            &shown,
            self.rect.x + 8.0,
            self.rect.y + 27.0,
            TextParams {
                font,
                font_size: 20,
                color: WHITE,
                ..Default::default()
            },
        );
    }

    fn clicked(&self) -> bool {
        let (mx, my) = mouse_position();
        is_mouse_button_pressed(MouseButton::Left) && self.rect.contains(vec2(mx, my))
    }

    fn input(&mut self, latch: &mut KeyLatch) {
        if let Some(ch) = get_char_pressed() {
            // На некоторых Windows-раскладках miniquad отдаёт управляющие
            // символы для Esc/Backspace — не добавляем их в текст.
            if !ch.is_control() && self.text.len() < 64 {
                self.text.push(ch);
            }
        }
        if backspace_pressed(latch) {
            self.text.pop();
        }
    }
}

/// Своя обработка Backspace: is_key_pressed в macroquad зависит от флага repeat,
/// поэтому надёжнее отслеживать удержание самим.
struct KeyLatch {
    down: bool,
    last: f64,
}

impl KeyLatch {
    fn new() -> Self {
        Self { down: false, last: 0.0 }
    }
}

fn backspace_pressed(latch: &mut KeyLatch) -> bool {
    let down = is_key_down(KeyCode::Backspace) || is_key_down(KeyCode::Delete);
    let now = get_time();
    if down {
        if !latch.down {
            latch.down = true;
            latch.last = now;
            return true;
        }
        if now - latch.last > 0.30 {
            latch.last = now;
            return true;
        }
    } else {
        latch.down = false;
    }
    false
}

fn find_region<'a>(atlas: &'a Atlas, name: &str) -> Option<&'a Region> {
    if let Some(r) = atlas.regions.get(name) {
        return Some(r);
    }
    for suffix in ["1", "0", "full", "ui"] {
        let key = format!("{}{}", name, suffix);
        if let Some(r) = atlas.regions.get(&key) {
            return Some(r);
        }
    }
    None
}

fn draw_sprite(
    atlas: &Atlas,
    pages: &[Texture2D],
    name: &str,
    sx: f32,
    sy: f32,
    size: f32,
    tint: Color,
) -> bool {
    let region = match find_region(atlas, name) {
        Some(r) => r,
        None => return false,
    };
    let texture = match pages.get(region.page) {
        Some(t) => t,
        None => return false,
    };
    // В атласе спрайты обрезаны по пустым краям: original_w/h — исходный
    // размер, offset_x/y — отступ обрезанной картинки внутри исходной.
    // Рисуем обрезанный регион с его настоящим размером и отступом,
    // иначе картинки растягиваются и съезжают с тайлов.
    let scale = size / 32.0;
    draw_texture_ex(
        texture,
        sx + region.offset_x * scale,
        sy + region.offset_y * scale,
        tint,
        DrawTextureParams {
            source: Some(Rect::new(
                region.x as f32,
                region.y as f32,
                region.w as f32,
                region.h as f32,
            )),
            dest_size: Some(vec2(region.w as f32 * scale, region.h as f32 * scale)),
            ..Default::default()
        },
    );
    true
}

/// Рисует блок только за его центром и с правильным размером (3x3 и т.д.).
fn draw_block_sprite(
    atlas: &Atlas,
    pages: &[Texture2D],
    name: &str,
    sx: f32,
    sy: f32,
    tile: f32,
) -> bool {
    let span = find_region(atlas, name)
        .map(|r| (r.original_w.max(1) / 32).max(1) as f32)
        .unwrap_or(1.0);
    let off = (span - 1.0) * tile / 2.0;
    draw_sprite(atlas, pages, name, sx - off, sy - off, tile, WHITE)
}

/// Игроки пока рисуются цветной точкой с именем; спрайт зависит от юнита,
/// поэтому юнит рисуется отдельной сущностью UnitState.
fn draw_player(p: &PlayerState, sx: f32, sy: f32, tile: f32, font: Option<&Font>) {
    let c = p.color as u32;
    let color = if c == 0 || c == 0xFFFFFFFF {
        Color::new(0.35, 0.85, 1.0, 1.0)
    } else {
        Color::new(
            ((c >> 16) & 0xFF) as f32 / 255.0,
            ((c >> 8) & 0xFF) as f32 / 255.0,
            (c & 0xFF) as f32 / 255.0,
            1.0,
        )
    };
    let r = (tile * 0.55).clamp(2.0, 14.0);
    draw_circle(sx, sy, r + 1.0, Color::new(0.0, 0.0, 0.0, 0.7));
    draw_circle(sx, sy, r, color);
    draw_text_ex(
        &p.name,
        sx + r + 4.0,
        sy - r,
        TextParams {
            font,
            font_size: 14,
            color: WHITE,
            ..Default::default()
        },
    );
}

/// Рисует юнит спрайтом из content header. Если спрайта нет — цветной точкой,
/// чтобы сущность не пропадала на модифицированном сервере.
fn draw_unit(
    u: &UnitState,
    name: Option<&str>,
    atlas: Option<&Atlas>,
    pages: &[Texture2D],
    pos: Vec2,
    tile: f32,
    font: Option<&Font>,
) {
    let size = tile * 1.25;
    let drawn = atlas
        .zip(name)
        .map(|(a, n)| {
            draw_sprite(
                a,
                pages,
                n,
                pos.x - size / 2.0,
                pos.y - size / 2.0,
                size,
                WHITE,
            )
        })
        .unwrap_or(false);
    if !drawn {
        let team_color = match u.team {
            0 => Color::new(0.35, 0.85, 1.0, 1.0),
            1 => Color::new(0.95, 0.35, 0.35, 1.0),
            _ => Color::new(0.85, 0.85, 0.45, 1.0),
        };
        let r = (tile * 0.45).clamp(2.0, 12.0);
        draw_circle(pos.x, pos.y, r, team_color);
    }
    draw_text_ex(
        format!("{}", u.unit_type),
        pos.x + size / 2.0 + 3.0,
        pos.y,
        TextParams {
            font,
            font_size: 12,
            color: Color::new(0.85, 0.90, 0.95, 0.8),
            ..Default::default()
        },
    );
}

// Вариант текстуры пола: точная реализация Arc Mathf.randomSeed + Rand.
struct ArcRand {
    seed0: u64,
    seed1: u64,
}

impl ArcRand {
    fn new(seed: i64) -> Self {
        let s = if seed == 0 { i64::MIN } else { seed };
        let seed0 = murmur3(s as u64);
        Self { seed0, seed1: murmur3(seed0) }
    }

    fn next_u64(&mut self) -> u64 {
        let s1 = self.seed0;
        let s0 = self.seed1;
        self.seed0 = s0;
        let s1 = s1 ^ (s1 << 23);
        let out = (s1 ^ s0 ^ (s1 >> 17) ^ (s0 >> 26)).wrapping_add(s0);
        self.seed1 = out;
        out
    }

    fn next_bounded(&mut self, n: u64) -> u64 {
        loop {
            let bits = self.next_u64() >> 1;
            let value = bits % n;
            let ok = (bits as i64)
                .wrapping_sub(value as i64)
                .wrapping_add((n - 1) as i64)
                >= 0;
            if ok { return value; }
        }
    }
}

fn murmur3(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^= x >> 33;
    x
}

fn floor_variant(x: i32, y: i32, variants: i32) -> i32 {
    if variants <= 1 { return 0; }
    let pack = (((x as i16) as i32) << 16) | (((y as i16) as u16) as i32);
    let max = variants - 1;
    let mut r = ArcRand::new(pack as i64);
    if max != 0 && (max & (max - 1)) == 0 {
        let _ = r.next_u64();
    }
    r.next_bounded(variants as u64) as i32
}

fn draw_world(
    world: &World,
    atlas: Option<&Atlas>,
    pages: &[Texture2D],
    palette: Option<&Image>,
    cam_x: f32,
    cam_y: f32,
    zoom: f32,
) {
    let tile = 16.0 * zoom;
    let w = screen_width();
    let h = screen_height();
    let min_x = (cam_x - w / (2.0 * tile)).floor() as i32;
    let max_x = (cam_x + w / (2.0 * tile)).ceil() as i32;
    let min_y = (cam_y - h / (2.0 * tile)).floor() as i32;
    let max_y = (cam_y + h / (2.0 * tile)).ceil() as i32;

    fn name_for(world: &World, id: i16) -> &str {
        if id >= 0 && (id as usize) < world.block_names.len() {
            &world.block_names[id as usize]
        } else {
            blocks::block_name(id)
        }
    }

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let (floor, ore, block) = match world.tile(x as usize, y as usize) {
                Some(v) => v,
                None => continue,
            };
            let sx = (x as f32 - cam_x) * tile + w / 2.0;
            // В Mindustry ось Y направлена вверх, а в окне — вниз.
            let sy = (cam_y - y as f32) * tile + h / 2.0;

            let floor_name = name_for(world, floor);
            let mut drawn = false;
            if let Some(a) = atlas {
                let variant = 1 + floor_variant(x, y, 6);
                let name = format!("{}{}", floor_name, variant);
                drawn = draw_sprite(a, pages, &name, sx, sy, tile, WHITE)
                    || draw_sprite(a, pages, floor_name, sx, sy, tile, WHITE);
            }
            if !drawn {
                let color = palette
                    .filter(|p| floor >= 0 && (floor as u32) < p.width as u32)
                    .map(|p| p.get_pixel(floor as u32, 0))
                    .unwrap_or(Color::new(0.18, 0.18, 0.20, 1.0));
                draw_rectangle(sx, sy, tile + 0.5, tile + 0.5, color);
            }

            if ore >= 0 {
                if let Some(a) = atlas {
                    draw_sprite(
                        a,
                        pages,
                        name_for(world, ore),
                        sx,
                        sy,
                        tile,
                        Color::new(1.0, 1.0, 1.0, 0.75),
                    );
                }
            }

            let idx = y as usize * world.width + x as usize;
            if block != 0 && world.centers.get(idx).copied().unwrap_or(true) {
                let name = name_for(world, block);
                let mut ok = false;
                if let Some(a) = atlas {
                    ok = draw_block_sprite(a, pages, name, sx, sy, tile);
                }
                if !ok {
                    let color = palette
                        .filter(|p| block >= 0 && (block as u32) < p.width as u32)
                        .map(|p| p.get_pixel(block as u32, 0))
                        .unwrap_or(Color::new(0.45, 0.45, 0.50, 1.0));
                    draw_rectangle(sx + 1.0, sy + 1.0, tile - 2.0, tile - 2.0, color);
                }
            }
        }
    }
}

/// Кнопки быстрого подключения к постоянным серверам.
const QUICK_SERVERS: [(&str, &str); 3] = [
    ("144.31.93.177", "6547"),
    ("46.149.69.116", "5555"),
    ("178.253.45.82", "6567"),
];

fn begin_connect(
    host: String,
    port: u16,
    name: String,
    input: Arc<Mutex<ClientInput>>,
) -> (Receiver<NetEvent>, Sender<String>, Arc<AtomicBool>) {
    let (tx_ev, rx_ev) = std::sync::mpsc::channel();
    let (tx_chat, rx_chat) = std::sync::mpsc::channel();
    *input.lock().unwrap() = ClientInput::default();
    let cancel = Arc::new(AtomicBool::new(false));
    net::start(host, port, name, tx_ev, rx_chat, Arc::clone(&cancel), input);
    (rx_ev, tx_chat, cancel)
}

fn connect_to(
    host: &str,
    port: &str,
    name: &str,
    events: &mut Option<Receiver<NetEvent>>,
    chat_tx: &mut Option<Sender<String>>,
    cancel: &mut Option<Arc<AtomicBool>>,
    input: &Arc<Mutex<ClientInput>>,
    status: &mut String,
    progress: &mut f32,
    net_state: &mut NetState,
) {
    let p: u16 = port.parse().unwrap_or(6567);
    let (rx_ev, tx_chat, flag) =
        begin_connect(host.to_string(), p, name.to_string(), Arc::clone(input));
    *events = Some(rx_ev);
    *chat_tx = Some(tx_chat);
    *cancel = Some(flag);
    *status = format!("Подключаюсь к {}:{} ...", host, p);
    *progress = 0.0;
    *net_state = NetState::Loading;
}

fn disconnect(
    events: &mut Option<Receiver<NetEvent>>,
    chat_tx: &mut Option<Sender<String>>,
    cancel: &mut Option<Arc<AtomicBool>>,
    world: &mut Option<World>,
    chat_log: &mut Vec<String>,
) {
    if let Some(flag) = cancel.take() {
        flag.store(true, Ordering::Relaxed);
    }
    *events = None;
    *chat_tx = None;
    *world = None;
    chat_log.clear();
}

fn draw_button(rect: Rect, label: &str, font: Option<&Font>) -> bool {
    let hover = rect.contains(vec2(mouse_position().0, mouse_position().1));
    draw_rectangle(
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        if hover {
            Color::new(0.22, 0.55, 0.85, 1.0)
        } else {
            Color::new(0.16, 0.42, 0.68, 1.0)
        },
    );
    draw_text_ex(
        label,
        rect.x + 12.0,
        rect.y + rect.h - 12.0,
        TextParams {
            font,
            font_size: 18,
            color: WHITE,
            ..Default::default()
        },
    );
    hover && is_mouse_button_pressed(MouseButton::Left)
}

struct ChatSegment {
    text: String,
    color: Color,
}

fn hex_color(s: &str) -> Option<Color> {
    let n = s.len();
    if !((1..=6).contains(&n) || n == 8) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut value: u32 = 0;
    for b in s.bytes() {
        let digit = match b {
            b'0'..=b'9' => (b - b'0') as u32,
            b'a'..=b'f' => (b - b'a' + 10) as u32,
            b'A'..=b'F' => (b - b'A' + 10) as u32,
            _ => return None,
        };
        value = (value << 4) | digit;
    }
    if n <= 6 {
        value <<= (8 - n) * 4;
    }
    let r = (value >> 24) & 0xff;
    let g = (value >> 16) & 0xff;
    let b = (value >> 8) & 0xff;
    let a = if n == 8 { value & 0xff } else { 0xff };
    Some(Color::new(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    ))
}

fn color_name(name: &str) -> Option<Color> {
    let key = name.to_lowercase().replace('_', "");
    let hex = match key.as_str() {
        // Mindustry
        "accent" | "stat" => "ffd37f",
        "unlaunched" => "8982ed",
        "highlight" => "ffe0a6",
        "negstat" => "e55454",
        // Arc / общие
        "clear" => "00000000",
        "black" => "000000",
        "white" => "ffffff",
        "lightgray" | "lightgrey" => "d3d3d3",
        "gray" | "grey" => "808080",
        "darkgray" | "darkgrey" => "a9a9a9",
        "blue" | "royal" => "4169e1",
        "navy" => "000080",
        "slate" => "708090",
        "sky" => "87ceeb",
        "cyan" => "00ffff",
        "teal" => "008080",
        "green" => "38d667",
        "acid" => "b5ff2f",
        "lime" => "00ff00",
        "forest" => "228b22",
        "olive" => "808000",
        "yellow" => "ffff00",
        "gold" => "ffd700",
        "goldenrod" => "daa520",
        "orange" => "ffa500",
        "brown" => "a52a2a",
        "tan" => "d2b48c",
        "brick" => "9c4b3d",
        "red" => "e55454",
        "scarlet" => "ff2400",
        "crimson" => "dc143c",
        "coral" => "ff7f50",
        "salmon" => "fa8072",
        "pink" => "ffc0cb",
        "magenta" => "ff00ff",
        "purple" => "800080",
        "violet" => "ee82ee",
        "maroon" => "800000",
        _ => return None,
    };
    hex_color(hex)
}

fn push_run(out: &mut Vec<ChatSegment>, run: &mut String, color: Color) {
    if !run.is_empty() {
        out.push(ChatSegment {
            text: std::mem::take(run),
            color,
        });
    }
}

/// Разбор разметки чата Mindustry: [red], [#RRGGBBAA], [] (возврат цвета), [[.
fn parse_chat(text: &str) -> Vec<ChatSegment> {
    let mut out = Vec::new();
    let mut run = String::new();
    let mut stack = vec![WHITE];
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                run.push('[');
                i += 2;
                continue;
            }
            if let Some(rel) = bytes[i + 1..].iter().position(|&b| b == b']') {
                let close = i + 1 + rel;
                let tag = &text[i + 1..close];
                if tag.is_empty() {
                    let current = *stack.last().unwrap();
                    push_run(&mut out, &mut run, current);
                    if stack.len() > 1 {
                        stack.pop();
                    }
                    i = close + 1;
                    continue;
                }
                let new_color = tag
                    .strip_prefix('#')
                    .and_then(hex_color)
                    .or_else(|| color_name(tag));
                if let Some(color) = new_color {
                    let current = *stack.last().unwrap();
                    push_run(&mut out, &mut run, current);
                    stack.push(color);
                    i = close + 1;
                    continue;
                }
            }
        }
        let ch = text[i..].chars().next().unwrap();
        run.push(ch);
        i += ch.len_utf8();
    }
    let current = *stack.last().unwrap();
    push_run(&mut out, &mut run, current);
    out
}

fn draw_chat_text(text: &str, x: f32, y: f32, font: Option<&Font>, size: u16) {
    let mut cursor = x;
    for seg in parse_chat(text) {
        draw_text_ex(
            &seg.text,
            cursor,
            y,
            TextParams {
                font,
                font_size: size,
                color: seg.color,
                ..Default::default()
            },
        );
        cursor += measure_text(&seg.text, font, size, 1.0).width;
    }
}

fn chat_width(text: &str, font: Option<&Font>, size: u16) -> f32 {
    parse_chat(text)
        .iter()
        .map(|seg| measure_text(&seg.text, font, size, 1.0).width)
        .sum()
}

#[macroquad::main(window_conf)]
async fn main() {
    // Ассеты берём из оригинальной игры, чтобы мир выглядел как Mindustry.
    let atlas_data = std::fs::read(asset_path("assets/sprites/sprites.aatls")).ok();
    let atlas = atlas_data
        .as_deref()
        .and_then(|b| match atlas::parse(b) {
            Ok(a) => Some(a),
            Err(e) => {
                eprintln!("[MindaRust] атлас не прочитан: {}", e);
                None
            }
        });

    let mut pages: Vec<Texture2D> = Vec::new();
    if let Some(a) = &atlas {
        for name in &a.pages {
            let path = asset_path(&format!("assets/sprites/{}", name));
            match load_image(&path).await {
                Ok(image) => pages.push(Texture2D::from_image(&image)),
                Err(e) => eprintln!("[MindaRust] картинка {} не загружена: {}", name, e),
            }
        }
    }
    let palette = match load_image(&asset_path("assets/sprites/block_colors.png")).await {
        Ok(image) => Some(image),
        Err(e) => {
            eprintln!("[MindaRust] палитра не загружена: {}", e);
            None
        }
    };

    let font = load_ttf_font(&asset_path("assets/fonts/segoeui.ttf")).await.ok();

    let args: Vec<String> = std::env::args().collect();
    let mut host = TextField::new("Сервер", args.get(1).map(String::as_str).unwrap_or("127.0.0.1"), 40.0, 170.0, 300.0);
    let mut port = TextField::new("Порт", args.get(2).map(String::as_str).unwrap_or("6567"), 355.0, 170.0, 100.0);
    let mut name = TextField::new("Имя", args.get(3).map(String::as_str).unwrap_or("MindaRust"), 470.0, 170.0, 220.0);
    let mut focus = 0usize;

    let connect_rect = Rect::new(40.0, 250.0, 200.0, 48.0);
    let chat_rect = Rect::new(20.0, screen_height() - 56.0, screen_width() - 40.0, 40.0);

    let mut net_state = NetState::Menu;
    let mut events: Option<Receiver<NetEvent>> = None;
    let mut chat_tx: Option<Sender<String>> = None;
    let mut cancel: Option<Arc<AtomicBool>> = None;
    let client_input = Arc::new(Mutex::new(ClientInput::default()));
    let mut own_unit_id = -1i32;
    let mut own_x = 0.0f32;
    let mut own_y = 0.0f32;
    let mut status = String::new();
    let mut progress = 0.0f32;
    let mut world: Option<World> = None;
    let mut players: Vec<PlayerState> = Vec::new();
    let mut units: Vec<UnitState> = Vec::new();
    let mut skipped_entities = 0usize;
    let mut last_status = String::new();
    let mut chat_log: Vec<String> = Vec::new();
    let mut chat_input = String::new();
    let mut chat_open = false;

    let mut cam_x = 0.0f32;
    let mut cam_y = 0.0f32;
    let mut cam_off_x = 0.0f32;
    let mut cam_off_y = 0.0f32;
    let mut zoom = 1.0f32;
    let mut last_mouse = mouse_position();
    let mut fps_instant = Instant::now();
    let mut frames = 0u32;
    let mut fps = 0.0f32;
    let mut bs_latch = KeyLatch::new();

    // Если переданы аргументы (host [port] [name]), подключаемся сразу.
    // Без аргументов открывается меню, где всё можно ввести вручную.
    if args.len() > 1 {
        connect_to(
            &host.text,
            &port.text,
            &name.text,
            &mut events,
            &mut chat_tx,
            &mut cancel,
            &client_input,
            &mut status,
            &mut progress,
            &mut net_state,
        );
    }

    loop {
        frames += 1;
        if fps_instant.elapsed().as_secs_f32() >= 0.5 {
            fps = frames as f32 / fps_instant.elapsed().as_secs_f32();
            frames = 0;
            fps_instant = Instant::now();
        }

        let (mx, my) = mouse_position();
        if is_mouse_button_pressed(MouseButton::Left) {
            if host.clicked() { focus = 0; }
            else if port.clicked() { focus = 1; }
            else if name.clicked() { focus = 2; }
        }

        if let Some(rx) = &events {
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    NetEvent::Status(s) => {
                        status = s.clone();
                        if s != last_status {
                            last_status = s;
                        }
                    }
                    NetEvent::Progress(p) => progress = p,
                    NetEvent::World(w) => {
                        cam_x = w.width as f32 / 2.0;
                        cam_y = w.height as f32 / 2.0;
                        own_unit_id = -1;
                        own_x = w.width as f32 * 4.0;
                        own_y = w.height as f32 * 4.0;
                        cam_off_x = 0.0;
                        cam_off_y = 0.0;
                        chat_open = false;
                        chat_input.clear();
                        world = Some(w);
                        players.clear();
                        units.clear();
                        skipped_entities = 0;
                    }
                    NetEvent::TileBlock { pos, block, team: _, rotation: _ } => {
                        if let Some(w) = &mut world {
                            let x = (pos >> 16) as i16;
                            let y = pos as i16;
                            if x >= 0 && y >= 0 {
                                let idx = y as usize * w.width + x as usize;
                                if idx < w.blocks.len() {
                                    w.blocks[idx] = block;
                                    w.centers[idx] = true;
                                }
                            }
                        }
                    }
                    NetEvent::TileFloor { pos, floor, overlay } => {
                        if let Some(w) = &mut world {
                            let x = (pos >> 16) as i16;
                            let y = pos as i16;
                            if x >= 0 && y >= 0 {
                                let idx = y as usize * w.width + x as usize;
                                if idx < w.floors.len() {
                                    w.floors[idx] = if floor == 0 { 33 } else { floor };
                                    w.ores[idx] = overlay;
                                }
                            }
                        }
                    }
                    NetEvent::BuildDestroyed(pos) => {
                        if let Some(w) = &mut world {
                            let x = (pos >> 16) as i16;
                            let y = pos as i16;
                            if x >= 0 && y >= 0 {
                                let idx = y as usize * w.width + x as usize;
                                if idx < w.blocks.len() {
                                    w.blocks[idx] = 0;
                                    w.centers[idx] = true;
                                    w.health[idx] = -1.0;
                                }
                            }
                        }
                    }
                    NetEvent::BuildHealth(updates) => {
                        if let Some(w) = &mut world {
                            for (pos, hp) in updates {
                                let x = (pos >> 16) as i16;
                                let y = pos as i16;
                                if x >= 0 && y >= 0 {
                                    let idx = y as usize * w.width + x as usize;
                                    if idx < w.health.len() {
                                        w.health[idx] = hp;
                                    }
                                }
                            }
                        }
                    }
                    NetEvent::TeamBlock { pos, team } => {
                        if let Some(w) = &mut world {
                            let x = (pos >> 16) as i16;
                            let y = pos as i16;
                            if x >= 0 && y >= 0 {
                                let idx = y as usize * w.width + x as usize;
                                if idx < w.teams.len() {
                                    w.teams[idx] = team;
                                }
                            }
                        }
                    }
                    NetEvent::Entities { players: update, units: new_units, skipped } => {
                        let my_player_id = world.as_ref().map(|w| w.player_id);
                        let my_unit = my_player_id
                            .and_then(|id| update.iter().find(|p| p.id == id))
                            .and_then(|p| new_units.iter().find(|u| u.id == p.unit_id));
                        if let Some(u) = my_unit {
                            // Первый снимок (или новый юнит после смерти) задаёт позицию.
                            if u.id != own_unit_id || (own_x == 0.0 && own_y == 0.0) {
                                own_x = u.x;
                                own_y = u.y;
                            }
                            own_unit_id = u.id;
                        } else if own_unit_id < 0 {
                            if let Some(p) = my_player_id
                                .and_then(|id| update.iter().find(|pid| pid.id == id))
                            {
                                own_unit_id = p.unit_id;
                                own_x = p.x;
                                own_y = p.y;
                            }
                        }
                        players = update;
                        units = new_units;
                        skipped_entities = skipped;
                    }
                    NetEvent::PlayerDisconnect(id) => {
                        players.retain(|p| p.id != id);
                    }
                    NetEvent::Chat(text) => {
                        chat_log.push(text);
                        if chat_log.len() > 8 {
                            chat_log.remove(0);
                        }
                    }
                    NetEvent::Kick(reason) => {
                        net_state = NetState::Error(format!("Кик с сервера: {}", reason));
                    }
                    NetEvent::Error(e) => net_state = NetState::Error(e),
                    NetEvent::Connected => {
                        net_state = NetState::Playing;
                        status = "Подключено".into();
                    }
                }
            }
        }

        clear_background(Color::new(0.07, 0.08, 0.10, 1.0));

        match net_state {
            NetState::Menu => {
                draw_text_ex(
                    "MindaRust",
                    40.0,
                    80.0,
                    TextParams {
                        font: font.as_ref(),
                        font_size: 54,
                        color: Color::new(0.35, 0.75, 1.0, 1.0),
                        ..Default::default()
                    },
                );
                draw_text_ex(
                    "Клиент для обычных серверов Mindustry 160.8",
                    40.0,
                    120.0,
                    TextParams {
                        font: font.as_ref(),
                        font_size: 20,
                        color: Color::new(0.70, 0.75, 0.82, 1.0),
                        ..Default::default()
                    },
                );

                if focus == 0 { host.input(&mut bs_latch); }
                if focus == 1 { port.input(&mut bs_latch); }
                if focus == 2 { name.input(&mut bs_latch); }
                host.draw(font.as_ref(), focus == 0);
                port.draw(font.as_ref(), focus == 1);
                name.draw(font.as_ref(), focus == 2);

                draw_rectangle(connect_rect.x, connect_rect.y, connect_rect.w, connect_rect.h,
                    if connect_rect.contains(vec2(mx, my)) {
                        Color::new(0.22, 0.55, 0.85, 1.0)
                    } else {
                        Color::new(0.16, 0.42, 0.68, 1.0)
                    });
                draw_text_ex(
                    "Подключиться",
                    connect_rect.x + 18.0,
                    connect_rect.y + 32.0,
                    TextParams {
                        font: font.as_ref(),
                        font_size: 22,
                        color: WHITE,
                        ..Default::default()
                    },
                );

                let clicked_connect = connect_rect.contains(vec2(mx, my))
                    && is_mouse_button_pressed(MouseButton::Left);
                if clicked_connect || is_key_pressed(KeyCode::Enter) {
                    connect_to(
                        &host.text,
                        &port.text,
                        &name.text,
                        &mut events,
                        &mut chat_tx,
                        &mut cancel,
                        &client_input,
                        &mut status,
                        &mut progress,
                        &mut net_state,
                    );
                }

                draw_text_ex(
                    "Быстрые сервера",
                    40.0,
                    315.0,
                    TextParams {
                        font: font.as_ref(),
                        font_size: 18,
                        color: Color::new(0.65, 0.72, 0.80, 1.0),
                        ..Default::default()
                    },
                );
                for (i, (quick_host, quick_port)) in QUICK_SERVERS.iter().enumerate() {
                    let rect = Rect::new(40.0 + i as f32 * 250.0, 335.0, 235.0, 42.0);
                    if draw_button(
                        rect,
                        &format!("{}:{}", quick_host, quick_port),
                        font.as_ref(),
                    ) {
                        host.text = (*quick_host).to_string();
                        port.text = (*quick_port).to_string();
                        connect_to(
                            quick_host,
                            quick_port,
                            &name.text,
                            &mut events,
                            &mut chat_tx,
                            &mut cancel,
                            &client_input,
                            &mut status,
                            &mut progress,
                            &mut net_state,
                        );
                    }
                }

                draw_text_ex(
                    "Управление: WASD/стрелки — движение, ЛКМ — стрельба, ПКМ — камера, Enter — чат.",
                    40.0,
                    screen_height() - 28.0,
                    TextParams {
                        font: font.as_ref(),
                        font_size: 16,
                        color: Color::new(0.50, 0.55, 0.62, 1.0),
                        ..Default::default()
                    },
                );
            }

            NetState::Loading => {
                draw_text_ex(
                    "MindaRust",
                    40.0,
                    80.0,
                    TextParams {
                        font: font.as_ref(),
                        font_size: 44,
                        color: Color::new(0.35, 0.75, 1.0, 1.0),
                        ..Default::default()
                    },
                );
                draw_text_ex(
                    &status,
                    40.0,
                    150.0,
                    TextParams {
                        font: font.as_ref(),
                        font_size: 24,
                        color: WHITE,
                        ..Default::default()
                    },
                );
                draw_rectangle(40.0, 190.0, 600.0, 18.0, Color::new(0.15, 0.18, 0.22, 1.0));
                draw_rectangle(40.0, 190.0, 600.0 * progress.clamp(0.0, 1.0), 18.0,
                    Color::new(0.25, 0.65, 1.0, 1.0));

                if draw_button(Rect::new(40.0, 240.0, 170.0, 44.0), "Отмена", font.as_ref())
                    || is_key_pressed(KeyCode::Escape)
                {
                    disconnect(&mut events, &mut chat_tx, &mut cancel, &mut world, &mut chat_log);
                    chat_open = false;
                    chat_input.clear();
                    net_state = NetState::Menu;
                }
            }

            NetState::Playing => {
                // Чат модальный: Enter открывает и отправляет, Esc закрывает.
                if is_key_pressed(KeyCode::Enter) {
                    if chat_open {
                        if !chat_input.trim().is_empty() {
                            if let Some(tx) = &chat_tx {
                                let _ = tx.send(chat_input.trim().to_string());
                            }
                        }
                        chat_input.clear();
                        chat_open = false;
                    } else {
                        chat_input.clear();
                        chat_open = true;
                    }
                }
                if is_key_pressed(KeyCode::Escape) && chat_open {
                    chat_input.clear();
                    chat_open = false;
                }

                if let Some(w) = &world {
                    let wheel = mouse_wheel().1;
                    if wheel != 0.0 && !chat_open {
                        zoom = (zoom + wheel * 0.1).clamp(0.4, 3.0);
                    }
                    if is_mouse_button_down(MouseButton::Right)
                        && !chat_open
                        && my > 40.0
                        && my < screen_height() - 70.0
                    {
                        cam_off_x -= (mx - last_mouse.0) / (16.0 * zoom);
                        cam_off_y += (my - last_mouse.1) / (16.0 * zoom);
                    }
                    if own_unit_id >= 0 {
                        cam_x = own_x / 8.0 + cam_off_x;
                        cam_y = own_y / 8.0 + cam_off_y;
                    }
                    let tile = 16.0 * zoom;

                    // WASD/стрелки двигают юнита, ПКМ задаёт смещение камеры,
                    // камера следует за юнитом, Shift ускоряет.
                    let mut dir_x = 0.0f32;
                    let mut dir_y = 0.0f32;
                    if !chat_open {
                        if is_key_down(KeyCode::A) || is_key_down(KeyCode::Left) { dir_x -= 1.0; }
                        if is_key_down(KeyCode::D) || is_key_down(KeyCode::Right) { dir_x += 1.0; }
                        if is_key_down(KeyCode::W) || is_key_down(KeyCode::Up) { dir_y += 1.0; }
                        if is_key_down(KeyCode::S) || is_key_down(KeyCode::Down) { dir_y -= 1.0; }
                    }
                    let len = (dir_x * dir_x + dir_y * dir_y).sqrt();
                    if len > 0.0 {
                        dir_x /= len;
                        dir_y /= len;
                    }
                    let speed = 18.0f32;
                    let mut step = speed * get_frame_time();
                    if chat_open {
                        step = 0.0;
                    }
                    own_x += dir_x * step;
                    own_y += dir_y * step;

                    let mouse_world_x = (cam_x + (mx - screen_width() / 2.0) / tile) * 8.0;
                    let mouse_world_y = (cam_y - (my - screen_height() / 2.0) / tile) * 8.0;
                    let rotation = (mouse_world_y - own_y).atan2(mouse_world_x - own_x);
                    let shooting = !chat_open
                        && is_mouse_button_down(MouseButton::Left)
                        && my > 40.0
                        && my < screen_height() - 70.0
                        && !chat_rect.contains(vec2(mx, my));
                    let boosting = !chat_open
                        && (is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift));

                    {
                        let mut input = client_input.lock().unwrap();
                        input.unit_id = own_unit_id;
                        input.x = own_x;
                        input.y = own_y;
                        input.pointer_x = mouse_world_x;
                        input.pointer_y = mouse_world_y;
                        input.rotation = rotation;
                        input.vx = dir_x * speed;
                        input.vy = dir_y * speed;
                        input.shooting = shooting;
                        input.boosting = boosting;
                        input.view_x = cam_x * 8.0;
                        input.view_y = cam_y * 8.0;
                        input.view_w = screen_width() / (2.0 * zoom);
                        input.view_h = screen_height() / (2.0 * zoom);
                    }

                    draw_world(w, atlas.as_ref(), &pages, palette.as_ref(), cam_x, cam_y, zoom);
                    for p in &players {
                        // Позиции сущностей заданы в пикселях мира: 1 тайл = 8 единиц.
                        let px = p.x / 8.0;
                        let py = p.y / 8.0;
                        let sx = (px - cam_x) * tile + screen_width() / 2.0;
                        let sy = (cam_y - py) * tile + screen_height() / 2.0;
                        if sx > -40.0 && sy > -40.0 && sx < screen_width() + 40.0 && sy < screen_height() + 40.0 {
                            draw_player(p, sx, sy, tile, font.as_ref());
                        }
                    }
                    for u in &units {
                        let ux = if u.id == own_unit_id { own_x } else { u.x / 8.0 };
                        let uy = if u.id == own_unit_id { own_y } else { u.y / 8.0 };
                        let sx = (ux - cam_x) * tile + screen_width() / 2.0;
                        let sy = (cam_y - uy) * tile + screen_height() / 2.0;
                        if sx > -40.0 && sy > -40.0 && sx < screen_width() + 40.0 && sy < screen_height() + 40.0 {
                            let name = usize::try_from(u.unit_type)
                                .ok()
                                .and_then(|i| w.unit_names.get(i))
                                .map(String::as_str);
                            draw_unit(
                                u,
                                name,
                                atlas.as_ref(),
                                &pages,
                                vec2(sx, sy),
                                tile,
                                font.as_ref(),
                            );
                        }
                    }
                }

                // HUD
                draw_rectangle(0.0, 0.0, screen_width(), 36.0, Color::new(0.0, 0.0, 0.0, 0.55));
                let entity_note = if skipped_entities == 0 {
                    String::new()
                } else {
                    format!(" | ENT не разобрано: {}", skipped_entities)
                };
                draw_text_ex(
                    &format!(
                        "MindaRust | FPS {:.0} | юнит {} ({:.0},{:.0}) | {}{}",
                        fps, own_unit_id, own_x, own_y, status, entity_note
                    ),
                    12.0,
                    25.0,
                    TextParams {
                        font: font.as_ref(),
                        font_size: 18,
                        color: Color::new(0.60, 0.95, 0.70, 1.0),
                        ..Default::default()
                    },
                );

                if draw_button(
                    Rect::new(screen_width() - 160.0, 4.0, 148.0, 28.0),
                    "Выйти с сервера",
                    font.as_ref(),
                ) || (is_key_pressed(KeyCode::Escape) && !chat_open)
                {
                    disconnect(&mut events, &mut chat_tx, &mut cancel, &mut world, &mut chat_log);
                    chat_open = false;
                    chat_input.clear();
                    net_state = NetState::Menu;
                }

                // Чат
                let log_y = screen_height() - 70.0 - chat_log.len() as f32 * 24.0;
                for (i, line) in chat_log.iter().enumerate() {
                    let y = log_y + i as f32 * 24.0;
                    draw_rectangle(
                        12.0,
                        y,
                        chat_width(line, font.as_ref(), 18) + 16.0,
                        22.0,
                        Color::new(0.0, 0.0, 0.0, 0.45),
                    );
                    draw_chat_text(line, 20.0, y + 17.0, font.as_ref(), 18);
                }

                if chat_open {
                    draw_rectangle(chat_rect.x, chat_rect.y, chat_rect.w, chat_rect.h,
                        Color::new(0.05, 0.06, 0.08, 0.85));
                    if let Some(ch) = get_char_pressed() {
                        if !ch.is_control() && chat_input.len() < 120 {
                            chat_input.push(ch);
                        }
                    }
                    if backspace_pressed(&mut bs_latch) {
                        chat_input.pop();
                    }
                    let mut shown = chat_input.clone();
                    shown.push('|');
                    draw_text_ex(
                        &shown,
                        chat_rect.x + 10.0,
                        chat_rect.y + 28.0,
                        TextParams {
                            font: font.as_ref(),
                            font_size: 20,
                            color: WHITE,
                            ..Default::default()
                        },
                    );
                } else {
                    draw_text_ex(
                        "Enter — чат | Esc — выход с сервера",
                        chat_rect.x + 10.0,
                        chat_rect.y + 28.0,
                        TextParams {
                            font: font.as_ref(),
                            font_size: 18,
                            color: Color::new(0.75, 0.78, 0.82, 1.0),
                            ..Default::default()
                        },
                    );
                }
            }

            NetState::Error(ref msg) => {
                draw_text_ex(
                    "Ошибка подключения",
                    40.0,
                    80.0,
                    TextParams {
                        font: font.as_ref(),
                        font_size: 44,
                        color: Color::new(1.0, 0.35, 0.35, 1.0),
                        ..Default::default()
                    },
                );
                draw_text_ex(
                    msg,
                    40.0,
                    150.0,
                    TextParams {
                        font: font.as_ref(),
                        font_size: 24,
                        color: WHITE,
                        ..Default::default()
                    },
                );
                if draw_button(Rect::new(40.0, 210.0, 170.0, 44.0), "В меню", font.as_ref())
                    || is_key_pressed(KeyCode::Escape)
                {
                    disconnect(&mut events, &mut chat_tx, &mut cancel, &mut world, &mut chat_log);
                    chat_open = false;
                    chat_input.clear();
                    net_state = NetState::Menu;
                }
            }
        }

        last_mouse = (mx, my);
        next_frame().await;
    }
}

#[cfg(test)]
mod tests {
    use super::{floor_variant, hex_color, parse_chat};

    #[test]
    fn chat_named_color_and_pop() {
        let parts = parse_chat("[red]привет[] мир");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].text, "привет");
        assert!((parts[0].color.r - (0xe5 as f32) / 255.0).abs() < 0.01);
        assert_eq!(parts[1].text, " мир");
        assert!(parts[1].color.a > 0.99);
    }

    #[test]
    fn chat_hex_and_escaped_bracket() {
        let parts = parse_chat("[#00ff00]a[[b]");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].text, "a[b]");
        assert!(parts[0].color.g > 0.99);
        assert_eq!(hex_color("ff000080").unwrap().a, 128.0 / 255.0);
        assert!(hex_color("1234567").is_none());
    }

    #[test]
    fn floor_variant_matches_arc_rand() {
        assert_eq!(floor_variant(0, 0, 6), 4);
        assert_eq!(floor_variant(1, 1, 6), 1);
        assert_eq!(floor_variant(10, 20, 6), 1);
        assert_eq!(floor_variant(200, 200, 6), 2);
        assert_eq!(floor_variant(202, 200, 6), 0);
        assert_eq!(floor_variant(-3, 50, 6), 3);
        assert_eq!(floor_variant(0, 0, 5), 0);
    }
}
