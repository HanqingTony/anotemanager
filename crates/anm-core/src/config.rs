//! 配置管理：`~/.anm/config.toml`

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

/// 配置目录名
pub const CONFIG_DIR_NAME: &str = ".anm";
/// 配置文件
pub const CONFIG_FILE_NAME: &str = "config.toml";
/// 索引文件
pub const INDEX_FILE_NAME: &str = "index.jsonl";
/// 默认编辑器
pub const DEFAULT_EDITOR: &str = "vim";
/// 默认 inbox 文件名
pub const DEFAULT_SKATCH_NAME: &str = "skatch.md";

/// 落盘的配置文件内容
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigFile {
    /// 笔记系统根目录
    pub root: PathBuf,
    /// TUI 打开笔记使用的编辑器
    pub editor: String,
    /// skatch.md 路径；缺省为 root/skatch.md
    pub skatch: Option<PathBuf>,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            root: PathBuf::new(),
            editor: DEFAULT_EDITOR.to_string(),
            skatch: None,
        }
    }
}

/// 运行时配置（含派生路径）
#[derive(Debug, Clone)]
pub struct Config {
    /// `~/.anm`
    pub home: PathBuf,
    /// `~/.anm/config.toml`
    pub config_path: PathBuf,
    /// 笔记系统根目录
    pub root: PathBuf,
    /// 默认编辑器
    pub editor: String,
    /// skatch.md 路径
    pub skatch: PathBuf,
    /// 索引文件路径（`~/.anm/index.jsonl`）
    pub index_path: PathBuf,
}

impl Config {
    /// `~/.anm` 目录
    pub fn anm_home() -> Result<PathBuf> {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow!("无法确定 HOME 目录"))?;
        Ok(home.join(CONFIG_DIR_NAME))
    }

    /// 加载配置；不存在时返回错误（提示先 `anm init`）
    pub fn load() -> Result<Self> {
        let home = Self::anm_home()?;
        let config_path = home.join(CONFIG_FILE_NAME);
        if !config_path.exists() {
            return Err(anyhow!(
                "未找到配置 {}，请先运行 `anm init <笔记系统根目录>`",
                config_path.display()
            ));
        }
        let raw = std::fs::read_to_string(&config_path)
            .with_context(|| format!("读取配置失败: {}", config_path.display()))?;
        let file: ConfigFile = toml::from_str(&raw)
            .with_context(|| format!("解析配置失败: {}", config_path.display()))?;
        if file.root.as_os_str().is_empty() {
            return Err(anyhow!("配置中缺少 root，请重新运行 `anm init`"));
        }
        let skatch = file.skatch.unwrap_or_else(|| file.root.join(DEFAULT_SKATCH_NAME));
        let index_path = home.join(INDEX_FILE_NAME);
        Ok(Self {
            home,
            config_path,
            root: file.root,
            editor: file.editor,
            skatch,
            index_path,
        })
    }

    /// 初始化：写入 `~/.anm/config.toml`（默认注册一个笔记系统）
    pub fn init(root: &Path, editor: &str) -> Result<Self> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        if !root.is_dir() {
            return Err(anyhow!("根目录不存在: {}", root.display()));
        }
        let home = Self::anm_home()?;
        std::fs::create_dir_all(&home)
            .with_context(|| format!("创建配置目录失败: {}", home.display()))?;
        let file = ConfigFile {
            root: root.clone(),
            editor: if editor.is_empty() { DEFAULT_EDITOR.to_string() } else { editor.to_string() },
            skatch: None,
        };
        let config_path = home.join(CONFIG_FILE_NAME);
        let raw = toml::to_string_pretty(&file)
            .with_context(|| "序列化配置失败")?;
        std::fs::write(&config_path, raw)
            .with_context(|| format!("写入配置失败: {}", config_path.display()))?;
        let index_path = home.join(INDEX_FILE_NAME);
        Ok(Self {
            home,
            config_path,
            root,
            editor: file.editor,
            skatch: file.root.join(DEFAULT_SKATCH_NAME),
            index_path,
        })
    }

    /// 保存当前配置（用于修改 root / editor 后落盘）
    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(&self.home)
            .with_context(|| format!("创建配置目录失败: {}", self.home.display()))?;
        let file = ConfigFile {
            root: self.root.clone(),
            editor: self.editor.clone(),
            skatch: Some(self.skatch.clone()),
        };
        let raw = toml::to_string_pretty(&file).with_context(|| "序列化配置失败")?;
        std::fs::write(&self.config_path, raw)
            .with_context(|| format!("写入配置失败: {}", self.config_path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anm_home_is_under_home() {
        let home = Config::anm_home().unwrap();
        assert_eq!(home.file_name().unwrap(), CONFIG_DIR_NAME);
    }
}
