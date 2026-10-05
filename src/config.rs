use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub comment_template: String,
}

pub fn load() -> Result<Config> {
    let path = Path::new("config.json");
    if !path.exists() {
        anyhow::bail!(
            "缺少配置文件 {}，请创建：\n{{\n  \"comment_template\": \"主观评价内容\"\n}}",
            path.display()
        );
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("读取 {} 失败", path.display()))?;
    let cfg: Config = serde_json::from_str(&text)
        .with_context(|| format!("解析 {} 失败", path.display()))?;
    Ok(cfg)
}
