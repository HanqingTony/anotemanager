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
/// 默认 MCP 传输方式（本地 HTTP）
pub const DEFAULT_MCP_TRANSPORT: McpTransport = McpTransport::Http;
/// 默认 MCP HTTP 绑定地址
pub const DEFAULT_MCP_HOST: &str = "127.0.0.1";
/// 默认 MCP HTTP 端口
pub const DEFAULT_MCP_PORT: u16 = 17371;

/// MCP 传输方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    /// Streamable HTTP，默认绑定 127.0.0.1
    #[default]
    Http,
    /// stdio（供 Claude Desktop / Cursor / opencode 等 spawn）
    Stdio,
}

/// MCP server 配置段（`[mcp]`）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    /// 传输方式：http（默认）| stdio
    pub transport: McpTransport,
    /// HTTP 绑定地址
    pub host: String,
    /// HTTP 端口
    pub port: u16,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            transport: DEFAULT_MCP_TRANSPORT,
            host: DEFAULT_MCP_HOST.to_string(),
            port: DEFAULT_MCP_PORT,
        }
    }
}

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
    /// MCP server 配置（默认本地 HTTP）
    pub mcp: McpConfig,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            root: PathBuf::new(),
            editor: DEFAULT_EDITOR.to_string(),
            skatch: None,
            mcp: McpConfig::default(),
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
    /// MCP server 配置
    pub mcp: McpConfig,
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
            mcp: file.mcp,
        })
    }

    /// 初始化：写入 `~/.anm/config.toml`（默认注册一个笔记系统，MCP 默认本地 HTTP）
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
            mcp: McpConfig::default(),
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
            mcp: file.mcp,
        })
    }

    /// 保存当前配置（用于修改 root / editor / mcp 后落盘）
    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(&self.home)
            .with_context(|| format!("创建配置目录失败: {}", self.home.display()))?;
        let file = ConfigFile {
            root: self.root.clone(),
            editor: self.editor.clone(),
            skatch: Some(self.skatch.clone()),
            mcp: self.mcp.clone(),
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

    #[test]
    fn mcp_config_defaults_to_local_http() {
        let mcp = McpConfig::default();
        assert_eq!(mcp.transport, McpTransport::Http);
        assert_eq!(mcp.host, "127.0.0.1");
        assert_eq!(mcp.port, 17371);
    }

    #[test]
    fn old_config_without_mcp_section_falls_back_to_default() {
        // 老配置文件没有 [mcp] 段：应回退到默认（本地 HTTP）
        let raw = "root = \"/tmp/notes\"\neditor = \"vim\"\n";
        let file: ConfigFile = toml::from_str(raw).unwrap();
        assert_eq!(file.mcp, McpConfig::default());
        assert_eq!(file.mcp.transport, McpTransport::Http);
    }

    #[test]
    fn mcp_section_round_trips() {
        let raw = r#"
root = "/tmp/notes"
editor = "vim"

[mcp]
transport = "stdio"
"#;
        let file: ConfigFile = toml::from_str(raw).unwrap();
        assert_eq!(file.mcp.transport, McpTransport::Stdio);
        // host/port 缺失时仍为默认
        assert_eq!(file.mcp.host, "127.0.0.1");
        let out = toml::to_string(&file).unwrap();
        assert!(out.contains("transport = \"stdio\""));
    }
}
