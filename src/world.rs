//! Разбор потока мира, который присылает ванильный сервер Mindustry.
//! Формат — тот же, что у сохранения (`NetworkIO.writeWorld`), но сжат zlib.

use std::io::Read;

use flate2::read::ZlibDecoder;

use crate::wire::*;

#[derive(Clone, Debug)]
pub struct World {
    /// id собственного игрока из потока мира (NetworkIO: stream.writeInt(player.id)).
    pub player_id: i32,
    pub width: usize,
    pub height: usize,
    /// id пола для каждой клетки.
    pub floors: Vec<i16>,
    /// id руды для каждой клетки (-1, если руды нет).
    pub ores: Vec<i16>,
    /// id блока для каждой клетки.
    pub blocks: Vec<i16>,
    /// только для этих клеток нужно рисовать спрайт блока (центр многоклеточного).
    pub centers: Vec<bool>,
    /// команда владельца здания для каждой клетки (обновляется пакетами SetTeam).
    pub teams: Vec<u8>,
    /// прочность здания для каждой клетки (-1, если неизвестна).
    pub health: Vec<f32>,
    /// вариант текстуры пола (floorData) для каждой клетки.
    pub floor_data: Vec<u8>,
    /// имена блоков, полученные от сервера в content header (могут отличаться
    /// от ванильных на модифицированном сервере).
    pub block_names: Vec<String>,
    /// имена юнитов из content header (ordinal 6).
    pub unit_names: Vec<String>,
    /// имена статусов из content header (ordinal 5).
    pub status_names: Vec<String>,
}

impl World {
    pub fn tile(&self, x: usize, y: usize) -> Option<(i16, i16, i16)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = y * self.width + x;
        Some((self.floors[i], self.ores[i], self.blocks[i]))
    }
}

fn need(cond: bool, what: &str) -> Result<(), String> {
    if cond {
        Ok(())
    } else {
        Err(format!("мир обрезан при чтении: {}", what))
    }
}

/// Пропустить `DataAsset` из блока патчей. На ванильном сервере патчей обычно 0.
fn skip_asset(data: &[u8], pos: &mut usize) -> Result<(), String> {
    let type_id = get_u8(data, pos).ok_or("тип патча")?;
    need(skip_utf(data, pos), "путь патча")?;
    let embedded = get_bool(data, pos).ok_or("флаг патча")?;

    if embedded {
        // ContentAsset (type 1) дополнительно хранит тип контента (i16).
        if type_id == 1 {
            need(get_i16(data, pos).is_some(), "тип контента патча")?;
        }
        // Asset.read / PatchAsset.read / ContentAsset.read: i32 длина + байты.
        let len = get_i32(data, pos).ok_or("длина патча")?;
        need(len >= 0 && *pos + len as usize <= data.len(), "данные патча")?;
        *pos += len as usize;
    } else {
        need(*pos + 32 <= data.len(), "хэш патча")?;
        *pos += 32;
    }
    Ok(())
}

/// Пропустить игрока (NetworkIO.writeWorld → player.write).
fn skip_player(data: &[u8], pos: &mut usize) -> Result<(), String> {
    let rev = get_i16(data, pos).ok_or("нет версии игрока")?;
    if rev > 2 {
        return Err(format!("неизвестная версия игрока: {}", rev));
    }
    // admin, boosting
    need(get_bool(data, pos).is_some(), "admin")?;
    need(get_bool(data, pos).is_some(), "boosting")?;
    // color (i32)
    need(get_i32(data, pos).is_some(), "color")?;
    // lastCommand (только в rev >= 1)
    if rev >= 1 {
        need(get_u8(data, pos).is_some(), "command")?;
    }
    // mouseX, mouseY
    need(get_f32(data, pos).is_some(), "mouseX")?;
    need(get_f32(data, pos).is_some(), "mouseY")?;
    // name
    need(get_typeio_string(data, pos).is_some(), "name")?;
    // selectedBlock ( rev >= 2 ), selectedRotation
    if rev >= 2 {
        need(get_i16(data, pos).is_some(), "selectedBlock")?;
        need(get_i32(data, pos).is_some(), "selectedRotation")?;
    }
    // shooting, team, typing
    need(get_bool(data, pos).is_some(), "shooting")?;
    need(get_u8(data, pos).is_some(), "team")?;
    need(get_bool(data, pos).is_some(), "typing")?;
    // unit: тип (u8) + id (i32)
    need(get_u8(data, pos).is_some(), "unit type")?;
    need(get_i32(data, pos).is_some(), "unit id")?;
    // x, y
    need(get_f32(data, pos).is_some(), "x")?;
    need(get_f32(data, pos).is_some(), "y")?;
    Ok(())
}

/// Прочитать content header: список имён контента с переопределёнными ID.
/// Возвращает пары (тип контента, имена), чтобы не угадывать списки по длине.
/// ContentType: 1 = block, 6 = unit.
fn read_content_header(data: &[u8], pos: &mut usize) -> Result<Vec<(u8, Vec<String>)>, String> {
    let mapped = get_u8(data, pos).ok_or("нет content header")?;
    let mut all = Vec::new();
    for _ in 0..mapped {
        let typ = get_u8(data, pos).ok_or("тип контента")?;
        let total = get_i16(data, pos).ok_or("размер content header")?;
        let mut names = Vec::new();
        for _ in 0..total.max(0) {
            names.push(get_utf_string(data, pos).ok_or("имя контента")?);
        }
        all.push((typ, names));
    }
    Ok(all)
}

/// Пропустить чанк: i32 длины + данные.
fn skip_chunk(data: &[u8], pos: &mut usize) -> Result<(), String> {
    let len = get_i32(data, pos).ok_or("нет длины чанка")?;
    if len < 0 || *pos + len as usize > data.len() {
        return Err(format!("чанк: len={} pos={} data_len={}", len, *pos, data.len()));
    }
    *pos += len as usize;
    Ok(())
}

pub fn parse_world(compressed: &[u8]) -> Result<World, String> {
    let mut decoder = ZlibDecoder::new(compressed);
    let mut data = Vec::new();
    decoder
        .read_to_end(&mut data)
        .map_err(|e| format!("не удалось распаковать мир: {}", e))?;

    let mut p = 0usize;

    // writeDataPatches
    need(get_i32(&data, &mut p).is_some(), "версия патчей")?;
    let patches = get_i32(&data, &mut p).ok_or("число патчей")?;
    need(patches >= 0, "отрицательное число патчей")?;
    for _ in 0..patches {
        skip_asset(&data, &mut p)?;
    }

    // state.rules и state.mapLocales — Java writeUTF
    need(skip_utf(&data, &mut p), "rules")?;
    need(skip_utf(&data, &mut p), "mapLocales")?;

    // writeStringMap(state.map.tags)
    let tags = get_i16(&data, &mut p).ok_or("размер tags")?;
    need(tags >= 0, "отрицательный размер tags")?;
    for _ in 0..tags {
        need(skip_utf(&data, &mut p), "ключ tags")?;
        need(skip_utf(&data, &mut p), "значение tags")?;
    }

    // wave, wavetime, tick, seed0, seed1
    need(get_i32(&data, &mut p).is_some(), "wave")?;
    need(get_f32(&data, &mut p).is_some(), "wavetime")?;
    need(get_f64(&data, &mut p).is_some(), "tick")?;
    need(get_i64(&data, &mut p).is_some(), "seed0")?;
    need(get_i64(&data, &mut p).is_some(), "seed1")?;

    // player id + player.read
    let player_id = get_i32(&data, &mut p).ok_or("player id")?;
    skip_player(&data, &mut p)?;

    // writeContentHeader
    let content = read_content_header(&data, &mut p)?;
    let block_names = content
        .iter()
        .find(|(typ, _)| *typ == 1)
        .map(|(_, names)| names.clone())
        .unwrap_or_default();
    let unit_names = content
        .iter()
        .find(|(typ, _)| *typ == 6)
        .map(|(_, names)| names.clone())
        .unwrap_or_default();
    let status_names = content
        .iter()
        .find(|(typ, _)| *typ == 5)
        .map(|(_, names)| names.clone())
        .unwrap_or_default();

    // writeMap
    let width = get_u16(&data, &mut p).ok_or("ширина карты")? as usize;
    let height = get_u16(&data, &mut p).ok_or("высота карты")? as usize;
    need(width > 0 && height > 0, "нулевой размер карты")?;
    need(width <= 4096 && height <= 4096, "слишком большая карта")?;
    let total = width * height;

    let mut floors = vec![-1i16; total];
    let mut ores = vec![-1i16; total];
    let mut floor_data = vec![0u8; total];

    // Пол и руда с RLE.
    let mut i = 0usize;
    while i < total {
        let floor = get_i16(&data, &mut p).ok_or("floor id")?;
        let ore = get_i16(&data, &mut p).ok_or("ore id")?;
        let consec = get_u8(&data, &mut p).ok_or("floor RLE")? as usize;
        for j in 0..=consec {
            let idx = i + j;
            if idx < total {
                // В движке air (id 0) как пол заменяется на stone (id 33).
                floors[idx] = if floor == 0 { 33 } else { floor };
                ores[idx] = ore;
            }
        }
        i += 1 + consec;
    }

    // Блоки. В движке есть RLE: если нет ни entity, ни data, после флагов
    // идёт байт consecutives, и следующие тайлы имеют тот же блок.
    let mut blocks = vec![0i16; total];
    let mut centers = vec![true; total];
    let teams = vec![1u8; total];
    let health = vec![-1.0f32; total];
    i = 0;
    while i < total {
        let block = get_i16(&data, &mut p).ok_or("block id")?;
        let packed = get_u8(&data, &mut p).ok_or("block flags")?;
        let had_data = packed & 4 != 0;
        let had_entity = packed & 1 != 0;

        if had_data {
            // data, floorData, overlayData, extraData
            let _data = get_u8(&data, &mut p).ok_or("block data")?;
            let fdata = get_u8(&data, &mut p).ok_or("floor data")?;
            if i < total {
                floor_data[i] = fdata;
            }
            need(get_u8(&data, &mut p).is_some(), "overlay data")?;
            need(get_i32(&data, &mut p).is_some(), "extra data")?;
        }

        if had_entity {
            let is_center = get_bool(&data, &mut p).ok_or("isCenter")?;
            centers[i] = is_center;
            if is_center {
                skip_chunk(&data, &mut p)?;
            }
        } else if !had_data {
            let consec = get_u8(&data, &mut p).ok_or("block RLE")? as usize;
            for j in 1..=consec {
                if i + j < total {
                    blocks[i + j] = block;
                }
            }
            blocks[i] = block;
            i += 1 + consec;
            continue;
        }

        blocks[i] = block;
        i += 1;
    }

    Ok(World {
        player_id,
        width,
        height,
        floors,
        ores,
        blocks,
        centers,
        teams,
        health,
        floor_data,
        block_names,
        unit_names,
        status_names,
    })
}
