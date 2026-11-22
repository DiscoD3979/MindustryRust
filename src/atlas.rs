//! Чтение бинарного атласа Arc (`sprites.aatls`).
//! Формат описан в `arc.graphics.g2d.TextureAtlas.TextureAtlasData`.

use std::collections::HashMap;

use crate::wire::*;

#[derive(Clone, Debug)]
pub struct Region {
    pub page: usize,
    pub x: i16,
    pub y: i16,
    pub w: i16,
    pub h: i16,
    pub offset_x: f32,
    pub offset_y: f32,
    pub original_w: i16,
    pub original_h: i16,
}

#[derive(Debug)]
pub struct Atlas {
    pub pages: Vec<String>,
    pub regions: HashMap<String, Region>,
}

fn take(data: &[u8], pos: &mut usize, n: usize, what: &str) -> Result<(), String> {
    if *pos + n > data.len() {
        return Err(format!("атлас обрезан при чтении: {}", what));
    }
    *pos += n;
    Ok(())
}

pub fn parse(data: &[u8]) -> Result<Atlas, String> {
    if data.len() < 6 || &data[..5] != b"AATLS" {
        return Err("неверный заголовок sprites.aatls".into());
    }
    let mut p = 6usize; // 5 байт заголовка + байт версии

    let mut pages = Vec::new();
    let mut regions = HashMap::new();

    while p < data.len() {
        // TextureAtlasData читает байт-маркер через checkEOF().
        let marker = get_u8(data, &mut p).ok_or("нет маркера страницы")?;
        if marker != 1 {
            return Err(format!("неожиданный маркер страницы: {}", marker));
        }

        let image = get_utf_string(data, &mut p).ok_or("нет имени страницы")?;
        let _pw = get_i16(data, &mut p).ok_or("нет ширины страницы")?;
        let _ph = get_i16(data, &mut p).ok_or("нет высоты страницы")?;
        take(data, &mut p, 4, "фильтры и обёртки")?;

        let count = get_i32(data, &mut p).ok_or("нет числа регионов")?;
        if count < 0 {
            return Err("отрицательное число регионов".into());
        }
        let page_index = pages.len();
        pages.push(image);

        for _ in 0..count {
            let name = get_utf_string(data, &mut p).ok_or("нет имени региона")?;
            let x = get_i16(data, &mut p).ok_or("нет x региона")?;
            let y = get_i16(data, &mut p).ok_or("нет y региона")?;
            let w = get_i16(data, &mut p).ok_or("нет ширины региона")?;
            let h = get_i16(data, &mut p).ok_or("нет высоты региона")?;

            let has_offsets = get_bool(data, &mut p).ok_or("нет флага offsets")?;
            let mut offset_x = 0.0f32;
            let mut offset_y = 0.0f32;
            let mut original_w = w;
            let mut original_h = h;
            if has_offsets {
                offset_x = get_i16(data, &mut p).ok_or("offsetX")? as f32;
                offset_y = get_i16(data, &mut p).ok_or("offsetY")? as f32;
                original_w = get_i16(data, &mut p).ok_or("originalWidth")?;
                original_h = get_i16(data, &mut p).ok_or("originalHeight")?;
            }

            if get_bool(data, &mut p).ok_or("нет флага splits")? {
                take(data, &mut p, 8, "splits")?;
            }
            if get_bool(data, &mut p).ok_or("нет флага pads")? {
                take(data, &mut p, 8, "pads")?;
            }

            regions.insert(
                name,
                Region {
                    page: page_index,
                    x,
                    y,
                    w,
                    h,
                    offset_x,
                    offset_y,
                    original_w,
                    original_h,
                },
            );
        }
    }

    Ok(Atlas { pages, regions })
}
