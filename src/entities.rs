//! Разбор синхронизации сущностей из пакета EntitySnapshot (id 48).
//! Поддерживаются classId 12 (Player) и 3 (UnitEntity). Остальные типы не
//! угадываются: длина их записи зависит от конкретного класса.

use crate::wire::*;

/// Игрок, прочитанный из `Player.readSync`.
#[derive(Clone, Debug)]
pub struct PlayerState {
    pub id: i32,
    pub admin: bool,
    pub boosting: bool,
    pub color: i32,
    pub mouse_x: f32,
    pub mouse_y: f32,
    pub name: String,
    pub selected_block: i16,
    pub selected_rotation: i32,
    pub shooting: bool,
    pub team: u8,
    pub typing: bool,
    pub unit_type: u8,
    pub unit_id: i32,
    pub x: f32,
    pub y: f32,
}

/// Юнит, прочитанный из `UnitEntity.readSync`. Поля, нужные для отрисовки,
/// оставлены; остальные байты пропускаются в точном порядке протокола.
#[derive(Clone, Debug)]
pub struct UnitState {
    pub id: i32,
    pub team: u8,
    pub unit_type: i16,
    pub health: f32,
    pub shield: f32,
    pub rotation: f32,
    pub shooting: bool,
    pub x: f32,
    pub y: f32,
}

/// Результат разбора снимка: игроки, юниты и число сущностей, которые
/// пропущены, потому что их classId нам неизвестен или данные обрезаны.
pub struct EntitySnapshot {
    pub players: Vec<PlayerState>,
    pub units: Vec<UnitState>,
    pub skipped: usize,
}

fn skip_bytes(data: &[u8], pos: &mut usize, n: usize) -> bool {
    if *pos + n > data.len() {
        return false;
    }
    *pos += n;
    true
}

/// Пропустить `TypeIO.readObjectSafe`. Возвращает false при неизвестном типе
/// или обрезанных данных — тогда снимок нельзя разбирать дальше.
fn skip_object(data: &[u8], pos: &mut usize, allow_arrays: bool, depth: u32) -> bool {
    if depth > 16 {
        return false;
    }
    let kind = match get_u8(data, pos) {
        Some(v) => v,
        None => return false,
    };
    match kind {
        0 => true,                      // null
        1 => get_i32(data, pos).is_some(),
        2 => get_i64(data, pos).is_some(),
        3 => get_f32(data, pos).is_some(),
        4 => skip_utf(data, pos),       // у TypeIO есть флаг наличия строки
        5 => get_u8(data, pos).is_some() && get_i16(data, pos).is_some(),
        6 | 21 => {
            if !allow_arrays {
                return false;
            }
            let n = match get_i16(data, pos) {
                Some(v) if v >= 0 => v as usize,
                _ => return false,
            };
            skip_bytes(data, pos, n * 4)
        }
        7 => skip_bytes(data, pos, 8),
        8 => {
            let n = match get_u8(data, pos) {
                Some(v) => v as usize,
                None => return false,
            };
            skip_bytes(data, pos, n * 4)
        }
        9 => get_u8(data, pos).is_some() && get_i16(data, pos).is_some(),
        10 => get_u8(data, pos).is_some(),
        11 => get_f64(data, pos).is_some(),
        12 | 17 => get_i32(data, pos).is_some(),
        13 => get_i16(data, pos).is_some(),
        14 | 16 => {
            if !allow_arrays {
                return false;
            }
            let n = match get_i32(data, pos) {
                Some(v) if v >= 0 => v as usize,
                _ => return false,
            };
            skip_bytes(data, pos, n)
        }
        15 => get_u8(data, pos).is_some(),
        18 => {
            if !allow_arrays {
                return false;
            }
            let n = match get_i16(data, pos) {
                Some(v) if v >= 0 => v as usize,
                _ => return false,
            };
            skip_bytes(data, pos, n * 8)
        }
        19 => skip_bytes(data, pos, 8),
        20 => get_u8(data, pos).is_some(),
        22 => {
            if !allow_arrays {
                return false;
            }
            let n = match get_i32(data, pos) {
                Some(v) if v >= 0 => v as usize,
                _ => return false,
            };
            for _ in 0..n {
                if !skip_object(data, pos, allow_arrays, depth + 1) {
                    return false;
                }
            }
            true
        }
        23 => get_u16(data, pos).is_some(),
        _ => false,
    }
}

/// Пропустить `TypeIO.readController` (только байты, без создания AI).
fn skip_controller(data: &[u8], pos: &mut usize) -> bool {
    let typ = match get_u8(data, pos) {
        Some(v) => v,
        None => return false,
    };
    match typ {
        0 | 1 | 3 => get_i32(data, pos).is_some(),
        4 | 6 | 7 | 8 | 9 => {
            let has_attack = match get_bool(data, pos) {
                Some(v) => v,
                None => return false,
            };
            let has_pos = match get_bool(data, pos) {
                Some(v) => v,
                None => return false,
            };
            if has_pos && !skip_bytes(data, pos, 8) {
                return false;
            }
            if has_attack && !(get_u8(data, pos).is_some() && get_i32(data, pos).is_some()) {
                return false;
            }
            if typ >= 6 && get_u8(data, pos).is_none() {
                return false;
            }
            if typ == 7 || typ == 8 || typ == 9 {
                let n = match get_u8(data, pos) {
                    Some(v) => v as usize,
                    None => return false,
                };
                for _ in 0..n {
                    let command = match get_u8(data, pos) {
                        Some(v) => v,
                        None => return false,
                    };
                    match command {
                        0 | 1 => {
                            if get_i32(data, pos).is_none() {
                                return false;
                            }
                        }
                        2 if !skip_bytes(data, pos, 8) => return false,
                        2 => {}
                        _ => {}
                    }
                }
            }
            if typ == 8 && get_u8(data, pos).is_none() {
                return false;
            }
            if typ == 9 {
                let n = match get_u8(data, pos) {
                    Some(v) => v as usize,
                    None => return false,
                };
                if !skip_bytes(data, pos, n) {
                    return false;
                }
            }
            true
        }
        // Тип 5 (AssemblerAI) и все прочие не читают дополнительных байт.
        _ => true,
    }
}

/// Пропустить `TypeIO.readPlansQueueNet`.
fn skip_plans(data: &[u8], pos: &mut usize) -> bool {
    let used = match get_i32(data, pos) {
        Some(v) => v,
        None => return false,
    };
    if used < 0 {
        return true;
    }
    if used > 1000 {
        return false;
    }
    for _ in 0..used {
        let breaking = match get_u8(data, pos) {
            Some(v) => v != 0,
            None => return false,
        };
        if get_i32(data, pos).is_none() {
            return false;
        }
        if !breaking {
            if get_i16(data, pos).is_none() || get_u8(data, pos).is_none() || get_u8(data, pos).is_none() {
                return false;
            }
            if !skip_object(data, pos, true, 0) {
                return false;
            }
        }
    }
    true
}

fn read_player(data: &[u8], pos: &mut usize, id: i32) -> Option<PlayerState> {
    let admin = get_bool(data, pos)?;
    let boosting = get_bool(data, pos)?;
    let color = get_i32(data, pos)?;
    let mouse_x = get_f32(data, pos)?;
    let mouse_y = get_f32(data, pos)?;
    let name = get_typeio_string(data, pos).unwrap_or_default();
    let selected_block = get_i16(data, pos)?;
    let selected_rotation = get_i32(data, pos)?;
    let shooting = get_bool(data, pos)?;
    let team = get_u8(data, pos)?;
    let typing = get_bool(data, pos)?;
    // TypeIO.readUnit: u8 type (0=null, 1=building, 2=unit) + i32 id.
    let unit_type = get_u8(data, pos)?;
    let unit_id = get_i32(data, pos)?;
    let x = get_f32(data, pos)?;
    let y = get_f32(data, pos)?;

    Some(PlayerState {
        id,
        admin,
        boosting,
        color,
        mouse_x,
        mouse_y,
        name,
        selected_block,
        selected_rotation,
        shooting,
        team,
        typing,
        unit_type,
        unit_id,
        x,
        y,
    })
}

/// `status_names` берётся из content header (ordinal 5): только статус
/// `dynamic` читает дополнительные поля, и это нельзя угадывать по id.
fn read_unit(data: &[u8], pos: &mut usize, id: i32, status_names: &[String]) -> Option<UnitState> {
    let abilities = get_u8(data, pos)? as usize;
    if !skip_bytes(data, pos, abilities * 4) {
        return None;
    }
    let _aim_x = get_f32(data, pos)?;
    let _aim_y = get_f32(data, pos)?;
    if !skip_controller(data, pos) {
        return None;
    }
    let _elevation = get_f32(data, pos)?;
    let _flag = get_f64(data, pos)?;
    let health = get_f32(data, pos)?;
    let shooting = get_bool(data, pos)?;
    let _mine_tile = get_i32(data, pos)?;
    let mounts = get_u8(data, pos)? as usize;
    if !skip_bytes(data, pos, mounts * 9) {
        return None;
    }
    if !skip_plans(data, pos) {
        return None;
    }
    let rotation = get_f32(data, pos)?;
    let shield = get_f32(data, pos)?;
    let _spawned_by_core = get_bool(data, pos)?;
    let _item = get_i16(data, pos)?;
    let _amount = get_i32(data, pos)?;
    let statuses = get_i32(data, pos)?;
    if !(0..=1000).contains(&statuses) {
        return None;
    }
    for _ in 0..statuses {
        let effect = get_i16(data, pos)?;
        get_f32(data, pos)?;
        let dynamic = status_names
            .get(effect as usize)
            // Неизвестный статус: нельзя знать, читаются ли дополнительные поля.
            .map(|name| name == "dynamic")?;
        if dynamic {
            let flags = get_u8(data, pos)?;
            for bit in 0..7 {
                if flags & (1 << bit) != 0 && get_f32(data, pos).is_none() {
                    return None;
                }
            }
        }
    }
    let team = get_u8(data, pos)?;
    let unit_type = get_i16(data, pos)?;
    let _update_building = get_bool(data, pos)?;
    let _vel_x = get_f32(data, pos)?;
    let _vel_y = get_f32(data, pos)?;
    let x = get_f32(data, pos)?;
    let y = get_f32(data, pos)?;

    Some(UnitState {
        id,
        team,
        unit_type,
        health,
        shield,
        rotation,
        shooting,
        x,
        y,
    })
}

/// `bytes` — содержимое TypeIO.writeBytes пакета (без i16 длины).
pub fn parse_entity_snapshot(
    amount: i16,
    bytes: &[u8],
    status_names: &[String],
) -> EntitySnapshot {
    let mut players = Vec::new();
    let mut units = Vec::new();
    let mut skipped = 0usize;
    let mut pos = 0usize;
    let total = amount.max(0) as usize;

    for _ in 0..total {
        let id = match get_i32(bytes, &mut pos) {
            Some(id) => id,
            None => {
                skipped += 1;
                break;
            }
        };
        let type_id = match get_u8(bytes, &mut pos) {
            Some(t) => t,
            None => {
                skipped += 1;
                break;
            }
        };
        match type_id {
            12 => {
                if let Some(p) = read_player(bytes, &mut pos, id) {
                    players.push(p);
                } else {
                    skipped += 1;
                    break;
                }
            }
            3 | 0 | 16 | 18 | 30 | 31 => {
                if let Some(u) = read_unit(bytes, &mut pos, id, status_names) {
                    units.push(u);
                } else {
                    skipped += 1;
                    break;
                }
            }
            _ => {
                // Неизвестный classId: нельзя безопасно определить длину записи,
                // поэтому остаток снимка пропускаем целиком.
                skipped = total - players.len() - units.len();
                break;
            }
        }
    }

    EntitySnapshot {
        players,
        units,
        skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{put_f32, put_i16, put_i32, put_typeio_string};

    fn put_f64(buf: &mut Vec<u8>, v: f64) {
        buf.extend_from_slice(&v.to_be_bytes());
    }

    fn player_bytes(id: i32, name: &str, x: f32, y: f32) -> Vec<u8> {
        let mut b = Vec::new();
        put_i32(&mut b, id);
        b.push(12);
        b.push(0); // admin
        b.push(0); // boosting
        put_i32(&mut b, -1); // color
        put_f32(&mut b, 0.0); // mouseX
        put_f32(&mut b, 0.0); // mouseY
        put_typeio_string(&mut b, name);
        put_i16(&mut b, -1); // selectedBlock
        put_i32(&mut b, 0); // selectedRotation
        b.push(0); // shooting
        b.push(0); // team
        b.push(0); // typing
        b.push(2); // unit type: unit
        put_i32(&mut b, 77); // unit id
        put_f32(&mut b, x);
        put_f32(&mut b, y);
        b
    }

    fn unit_bytes(id: i32, unit_type: i16, x: f32, y: f32) -> Vec<u8> {
        let mut b = Vec::new();
        put_i32(&mut b, id);
        b.push(3); // classId UnitEntity
        b.push(0); // abilities
        put_f32(&mut b, 0.0); // aimX
        put_f32(&mut b, 0.0); // aimY
        b.push(5); // controller: AssemblerAI, без дополнительных байт
        put_f32(&mut b, 0.0); // elevation
        put_f64(&mut b, 0.0); // flag
        put_f32(&mut b, 120.0); // health
        b.push(1); // shooting
        put_i32(&mut b, -1); // mineTile
        b.push(0); // mounts
        put_i32(&mut b, -1); // plans
        put_f32(&mut b, 1.5); // rotation
        put_f32(&mut b, 25.0); // shield
        b.push(0); // spawnedByCore
        put_i16(&mut b, -1); // item
        put_i32(&mut b, 0); // amount
        put_i32(&mut b, 0); // statuses
        b.push(1); // team
        put_i16(&mut b, unit_type);
        b.push(0); // updateBuilding
        put_f32(&mut b, 0.0); // velX
        put_f32(&mut b, 0.0); // velY
        put_f32(&mut b, x);
        put_f32(&mut b, y);
        b
    }

    #[test]
    fn parses_two_players() {
        let mut bytes = player_bytes(1, "Первый", 10.5, 20.0);
        bytes.extend_from_slice(&player_bytes(2, "Второй", -3.0, 4.25));
        let snap = parse_entity_snapshot(2, &bytes, &[]);
        assert_eq!(snap.skipped, 0);
        assert_eq!(snap.players.len(), 2);
        assert_eq!(snap.players[0].name, "Первый");
        assert_eq!(snap.players[1].id, 2);
        assert_eq!(snap.players[1].unit_id, 77);
        assert_eq!(snap.players[1].y, 4.25);
    }

    #[test]
    fn unknown_class_is_skipped_without_guessing() {
        let mut bytes = Vec::new();
        put_i32(&mut bytes, 5);
        bytes.push(6); // Building — не разбираем
        bytes.extend_from_slice(&[1, 2, 3]);
        let snap = parse_entity_snapshot(1, &bytes, &[]);
        assert!(snap.players.is_empty());
        assert!(snap.units.is_empty());
        assert_eq!(snap.skipped, 1);
    }

    #[test]
    fn parses_unit_entity() {
        let bytes = unit_bytes(9, 0, 12.5, -7.25);
        let snap = parse_entity_snapshot(1, &bytes, &[]);
        assert_eq!(snap.skipped, 0);
        assert_eq!(snap.players.len(), 0);
        assert_eq!(snap.units.len(), 1);
        let u = &snap.units[0];
        assert_eq!(u.id, 9);
        assert_eq!(u.unit_type, 0);
        assert_eq!(u.team, 1);
        assert_eq!(u.health, 120.0);
        assert_eq!(u.shield, 25.0);
        assert_eq!(u.rotation, 1.5);
        assert!(u.shooting);
        assert_eq!(u.x, 12.5);
        assert_eq!(u.y, -7.25);
    }

    #[test]
    fn parses_unit_entity_legacy_gamma_class() {
        let mut bytes = unit_bytes(9, 0, 12.5, -7.25);
        bytes[4] = 31; // UnitEntityLegacyGamma использует тот же readSync
        let snap = parse_entity_snapshot(1, &bytes, &[]);
        assert_eq!(snap.skipped, 0);
        assert_eq!(snap.units.len(), 1);
        assert_eq!(snap.units[0].id, 9);
    }

    #[test]
    fn parses_player_and_unit_in_one_snapshot() {
        let mut bytes = player_bytes(1, "Игрок", 1.0, 2.0);
        bytes.extend_from_slice(&unit_bytes(42, 3, 30.0, 40.0));
        let snap = parse_entity_snapshot(2, &bytes, &[]);
        assert_eq!(snap.skipped, 0);
        assert_eq!(snap.players.len(), 1);
        assert_eq!(snap.units.len(), 1);
        assert_eq!(snap.units[0].id, 42);
        assert_eq!(snap.units[0].unit_type, 3);
    }

    #[test]
    fn unit_with_dynamic_status_is_skipped_with_names() {
        let mut bytes = Vec::new();
        put_i32(&mut bytes, 1);
        bytes.push(3);
        bytes.push(0); // abilities
        put_f32(&mut bytes, 0.0);
        put_f32(&mut bytes, 0.0);
        bytes.push(5); // controller
        put_f32(&mut bytes, 0.0);
        put_f64(&mut bytes, 0.0);
        put_f32(&mut bytes, 100.0);
        bytes.push(0);
        put_i32(&mut bytes, -1);
        bytes.push(0);
        put_i32(&mut bytes, -1);
        put_f32(&mut bytes, 0.0);
        put_f32(&mut bytes, 0.0);
        bytes.push(0);
        put_i16(&mut bytes, -1);
        put_i32(&mut bytes, 0);
        put_i32(&mut bytes, 1); // statuses
        put_i16(&mut bytes, 0); // dynamic status id
        put_f32(&mut bytes, 5.0);
        bytes.push(0b0000_0011); // damage + health
        put_f32(&mut bytes, 2.0);
        put_f32(&mut bytes, 3.0);
        bytes.push(0);
        put_i16(&mut bytes, 0);
        bytes.push(0);
        put_f32(&mut bytes, 0.0);
        put_f32(&mut bytes, 0.0);
        put_f32(&mut bytes, 1.0);
        put_f32(&mut bytes, 2.0);
        let names = vec!["dynamic".to_string()];
        let snap = parse_entity_snapshot(1, &bytes, &names);
        assert_eq!(snap.skipped, 0);
        assert_eq!(snap.units.len(), 1);
        assert_eq!(snap.units[0].x, 1.0);
    }
}
