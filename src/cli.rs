use std::io::{self, IsTerminal, Write};

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};

use crate::urp::Credentials;

#[derive(Debug, Parser)]
#[command(
    name = "yzu-comment",
    version,
    about = "扬州大学教务系统「课堂教师」教学评估自动完成工具"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// 网络方式: 1 = 外网(WebVPN), 2 = 内网(校园网直连)
    #[arg(long, global = true, value_name = "1|2")]
    pub net: Option<String>,

    /// 演练模式，只预览将提交的答案，不真正提交
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// 按教师姓名 / 课程名筛选
    #[arg(short = 'k', long = "keyword", value_name = "关键字", global = true)]
    pub keyword: Option<String>,

    /// 显示浏览器窗口（默认无头）
    #[arg(long, global = true)]
    pub headed: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 查看评测任务列表
    List,
    /// 自动评测所有待评估任务
    Run,
}

fn prompt_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

pub fn choose_net_mode() -> Result<String> {
    println!("请选择访问方式：");
    println!("  1. 外网 (WebVPN)  (https://webvpn.yzu.edu.cn/login?cas_login=true)");
    println!("  2. 内网 (校园网直连)  (http://ydjwxs.yzu.edu.cn/)");
    loop {
        match prompt_line("请输入编号: ")?.as_str() {
            "1" => return Ok("1".to_string()),
            "2" => return Ok("2".to_string()),
            _ => println!("无效输入，请输入 1 或 2。"),
        }
    }
}

pub fn choose_action() -> Result<(&'static str, bool)> {
    println!("\n请选择操作：");
    println!("  1. 查看评测任务列表");
    println!("  2. 自动评测所有待评估任务");
    println!("  3. 演练(只查看将提交的答案，不真正提交)");
    loop {
        match prompt_line("请输入编号: ")?.as_str() {
            "1" => return Ok(("list", false)),
            "2" => return Ok(("run", false)),
            "3" => return Ok(("run", true)),
            _ => println!("无效输入，请输入 1、2 或 3。"),
        }
    }
}

pub fn input_credentials() -> Result<Credentials> {
    println!("\n请输入统一认证账号：");
    let username = prompt_line("  学号: ")?;
    if username.is_empty() {
        bail!("学号不能为空");
    }

    let password = if io::stdin().is_terminal() {
        rpassword::prompt_password("  密码: ")?
    } else {
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        input.trim_end_matches(['\r', '\n']).to_string()
    };
    if password.is_empty() {
        bail!("密码不能为空");
    }

    Ok(Credentials { username, password })
}
