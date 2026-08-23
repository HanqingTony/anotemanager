//! 全局快捷键（跨平台表示）：字符串与（修饰键位掩码 + 虚拟键码）互转。
//!
//! 字符串形式与 Windows `RegisterHotKey` 参数一一对应，持久化在托盘配置里，
//! 如 `"Alt+Shift+Z"`。其他平台外壳（未来的 wayland/android）可以复用
//! 同样的字符串格式，只需在各自平台映射为对应实现。

/// 修饰键位掩码：与 Windows `MOD_*` 常量一致（跨平台文档化约定）。
pub mod mods {
    /// Alt
    pub const ALT: u32 = 0x1;
    /// Ctrl
    pub const CTRL: u32 = 0x2;
    /// Shift
    pub const SHIFT: u32 = 0x4;
    /// Win（Meta）
    pub const WIN: u32 = 0x8;
}

/// 一个快捷键：修饰键位掩码 + 虚拟键码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    /// 修饰键位掩码（`mods::*` 按位或；不允许为空）
    pub mods: u32,
    /// 虚拟键码（Windows `VK_*`；非 Windows 平台按键码表映射）
    pub vk: u32,
}

impl Hotkey {
    /// 常用键码常量。
    pub const VK_ESCAPE: u32 = 0x1B;
    pub const VK_ENTER: u32 = 0x0D;
    pub const VK_SPACE: u32 = 0x20;
    pub const VK_TAB: u32 = 0x09;

    /// 构造；要求至少一个修饰键，否则返回 None（全局快捷键必须有修饰键，
    /// 避免吞掉普通按键输入）。
    pub fn new(mods: u32, vk: u32) -> Option<Self> {
        if mods == 0 || vk == 0 {
            return None;
        }
        Some(Self { mods, vk })
    }
}

/// 把键码转成可读名称：A-Z / 0-9 / F1-F12 / 常用功能键；其余返回 None。
fn vk_name(vk: u32) -> Option<String> {
    match vk {
        0x41..=0x5A => Some(((vk as u8) as char).to_string()),
        0x30..=0x39 => Some(((vk as u8) as char).to_string()),
        0x70..=0x7B => Some(format!("F{}", vk - 0x70 + 1)),
        Hotkey::VK_ESCAPE => Some("Esc".into()),
        Hotkey::VK_ENTER => Some("Enter".into()),
        Hotkey::VK_SPACE => Some("Space".into()),
        Hotkey::VK_TAB => Some("Tab".into()),
        0x25..=0x28 => Some(["Left", "Up", "Right", "Down"][(vk - 0x25) as usize].into()),
        _ => None,
    }
}

/// 把可读名称转回键码：A-Z / 0-9 / F1-F12 / 常用功能键。
fn parse_vk(name: &str) -> Option<u32> {
    let upper = name.trim().to_ascii_uppercase();
    match upper.as_str() {
        "ESC" => Some(Hotkey::VK_ESCAPE),
        "ENTER" => Some(Hotkey::VK_ENTER),
        "SPACE" => Some(Hotkey::VK_SPACE),
        "TAB" => Some(Hotkey::VK_TAB),
        "LEFT" => Some(0x25),
        "UP" => Some(0x26),
        "RIGHT" => Some(0x27),
        "DOWN" => Some(0x28),
        _ => {
            let mut chars = upper.chars();
            let single = (chars.next(), chars.next());
            if let (Some(c), None) = single {
                if c.is_ascii_uppercase() {
                    return Some(c as u32);
                }
                if c.is_ascii_digit() {
                    return Some(c as u32);
                }
            }
            if let Some(rest) = upper.strip_prefix('F') {
                if let Ok(n) = rest.parse::<u32>() {
                    if (1..=12).contains(&n) {
                        return Some(0x70 + n - 1);
                    }
                }
            }
            None
        }
    }
}

/// 格式化快捷键为可读字符串，如 `"Alt+Shift+Z"`。
pub fn format(hk: &Hotkey) -> String {
    let mut parts: Vec<String> = Vec::new();
    if hk.mods & mods::CTRL != 0 {
        parts.push("Ctrl".to_string());
    }
    if hk.mods & mods::ALT != 0 {
        parts.push("Alt".to_string());
    }
    if hk.mods & mods::SHIFT != 0 {
        parts.push("Shift".to_string());
    }
    if hk.mods & mods::WIN != 0 {
        parts.push("Win".to_string());
    }
    if let Some(name) = vk_name(hk.vk) {
        parts.push(name);
    } else {
        parts.push(format!("键码{}", hk.vk));
    }
    parts.join("+")
}

/// 解析字符串（如 `"Alt+Shift+Z"`）为快捷键；非法时返回 None。
///
/// 修饰键顺序不限；大小写不敏感；至少需要一个修饰键。
pub fn parse(s: &str) -> Option<Hotkey> {
    let mut mods = 0u32;
    let mut vk: Option<u32> = None;
    for part in s.split('+') {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= mods::CTRL,
            "alt" => mods |= mods::ALT,
            "shift" => mods |= mods::SHIFT,
            "win" | "meta" | "super" => mods |= mods::WIN,
            _ => {
                if vk.is_some() {
                    return None; // 主键只能有一个
                }
                vk = parse_vk(part);
                if vk.is_none() {
                    return None;
                }
            }
        }
    }
    Hotkey::new(mods, vk?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_common() {
        for s in ["Alt+Shift+Z", "Ctrl+Alt+X", "Alt+Shift+F2", "Alt+Shift+1"] {
            let hk = parse(s).unwrap();
            assert_eq!(format(&hk), s);
        }
    }

    #[test]
    fn parse_flexible() {
        let hk = parse("shift+alt+z").unwrap();
        assert_eq!(hk.mods, mods::ALT | mods::SHIFT);
        assert_eq!(hk.vk, b'Z' as u32);
        assert_eq!(format(&hk), "Alt+Shift+Z");
        // 大小写不敏感的主键
        assert_eq!(parse("Alt+Shift+a").unwrap().vk, b'A' as u32);
    }

    #[test]
    fn reject_invalid() {
        assert!(parse("").is_none());
        assert!(parse("Z").is_none()); // 无修饰键
        assert!(parse("Alt+Shift").is_none()); // 无主键
        assert!(parse("Alt+Z+X").is_none()); // 两个主键
        assert!(parse("Alt+Shift+阿").is_none()); // 非法主键
        assert!(parse("Alt++Shift+Z").is_none()); // 空段
        assert!(parse("Alt+Shift+F13").is_none()); // 超出 F1-F12
    }

    #[test]
    fn vk_names() {
        assert_eq!(vk_name(0x41), Some("A".to_string()));
        assert_eq!(vk_name(0x70), Some("F1".to_string()));
        assert_eq!(vk_name(Hotkey::VK_ESCAPE), Some("Esc".to_string()));
        assert_eq!(vk_name(0x25), Some("Left".to_string()));
        assert!(vk_name(0x13).is_none()); // Pause 不支持
    }
}
