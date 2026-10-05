mod cli;
mod config;
mod urp;

use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use serde_json::Value;

use crate::cli::{Cli, Command};
use crate::urp::{Credentials, NetMode, SubmitOutcome, Urp};

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("\n错误: {err:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let cfg = config::load()?;

    let net = match cli.net.as_deref() {
        Some(choice) => choice.to_string(),
        None => cli::choose_net_mode()?,
    };
    let mode = NetMode::from_choice(&net)?;

    let cred = cli::input_credentials()?;
    let headless = !cli.headed;

    let (action, dry_run) = match cli.command {
        Some(Command::List) => ("list", false),
        Some(Command::Run) => ("run", cli.dry_run),
        None => {
            let (action, dry) = cli::choose_action()?;
            (action, dry || cli.dry_run)
        }
    };

    let keyword = cli.keyword.unwrap_or_default();

    match action {
        "list" => cmd_list(mode, cred, headless).await,
        _ => cmd_run(mode, cred, headless, dry_run, &keyword, &cfg.comment_template).await,
    }
}

async fn cmd_list(mode: NetMode, cred: Credentials, headless: bool) -> Result<()> {
    let urp = Urp::login(mode, &cred, headless).await?;
    let records = urp.list_evaluations("ktjs").await?;

    if records.is_empty() {
        println!("当前无评测任务。");
        urp.close().await;
        return Ok(());
    }

    let pending = records.iter().filter(|r| !is_evaluated(r)).count();
    println!("课堂教师评测: 共 {} 条, 待评估 {} 条\n", records.len(), pending);
    for r in &records {
        let mark = if is_evaluated(r) { "已评估" } else { "待评估" };
        println!(
            "  [{mark}] {:8} | {} | 课程号 {}-{}",
            field_str(r, "JSM"),
            field_str(r, "KCM"),
            field_str(r, "KCH"),
            field_str(r, "KXH"),
        );
    }

    urp.close().await;
    Ok(())
}

async fn cmd_run(
    mode: NetMode,
    cred: Credentials,
    headless: bool,
    dry_run: bool,
    keyword: &str,
    comment: &str,
) -> Result<()> {
    let urp = Urp::login(mode, &cred, headless).await?;
    let records = urp.list_evaluations("ktjs").await?;

    let mut pending: Vec<&Value> = records.iter().filter(|r| !is_evaluated(r)).collect();
    if !keyword.is_empty() {
        let kw = keyword.to_lowercase();
        pending.retain(|r| {
            let haystack = format!("{}{}", field_str(r, "JSM"), field_str(r, "KCM")).to_lowercase();
            haystack.contains(&kw)
        });
    }

    let total = pending.len();
    if total == 0 {
        if keyword.is_empty() {
            println!("没有待评估的评测任务。");
        } else {
            println!("没有匹配「{keyword}」的待评估任务。");
        }
        urp.close().await;
        return Ok(());
    }

    println!("\n开始评测 {total} 门课程 ...\n");
    let mut ok = 0usize;
    for (i, rec) in pending.iter().enumerate() {
        println!(
            "[{}/{}] {} - {}",
            i + 1,
            total,
            field_str(rec, "JSM"),
            field_str(rec, "KCM")
        );

        let ktid = field_str(rec, "KTID");
        let form = match urp.open_evaluation(&ktid).await {
            Ok(form) => form,
            Err(err) => {
                println!("    跳过: {err}");
                continue;
            }
        };
        if let Some(err) = form.error.clone() {
            println!("    跳过: {err}");
            continue;
        }

        match urp.submit(&form, comment, dry_run).await {
            Ok(SubmitOutcome::DryRun { payload, .. }) => {
                println!("    [dry-run] 题目 {} 道", payload.len());
                for (key, value) in &payload {
                    match value {
                        Value::Array(items) => {
                            let first = items.first().and_then(|v| v.as_str()).unwrap_or("");
                            println!(
                                "      {} 多选 {} 项: {} ...",
                                truncate(key, 24),
                                items.len(),
                                truncate(first, 14)
                            );
                        }
                        Value::String(s) => {
                            println!("      {} = {}", truncate(key, 24), truncate(s, 20));
                        }
                        other => {
                            println!(
                                "      {} = {}",
                                truncate(key, 24),
                                truncate(&other.to_string(), 20)
                            );
                        }
                    }
                }
                ok += 1;
            }
            Ok(SubmitOutcome::Done(res)) => {
                if res.get("result").and_then(|v| v.as_str()) == Some("ok") {
                    println!("    ✓ 评估成功");
                    ok += 1;
                } else {
                    println!("    ✗ {}", msg_of(&res));
                }
            }
            Err(err) => println!("    ✗ {err}"),
        }

        tokio::time::sleep(Duration::from_millis(1500)).await;
    }

    println!("\n完成: 成功 {ok}/{total}");
    urp.close().await;
    Ok(())
}

fn field_str(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn is_evaluated(value: &Value) -> bool {
    field_str(value, "SFPG") == "1"
}

fn msg_of(value: &Value) -> String {
    for key in ["msg2", "msg", "result"] {
        if let Some(val) = value.get(key) {
            let text = match val {
                Value::String(s) => s.clone(),
                Value::Null => continue,
                other => other.to_string(),
            };
            if !text.is_empty() {
                return text;
            }
        }
    }
    value.to_string()
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
