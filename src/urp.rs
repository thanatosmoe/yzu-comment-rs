use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::page::Page;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Copy)]
pub struct NetMode {
    pub name: &'static str,
    pub entry: &'static str,
    pub host: &'static str,
    pub use_portal: bool,
}

impl NetMode {
    pub fn from_choice(choice: &str) -> Result<Self> {
        match choice {
            "1" | "webvpn" => Ok(Self {
                name: "外网 (WebVPN)",
                entry: "https://webvpn.yzu.edu.cn/login?cas_login=true",
                host: "webvpn.yzu.edu.cn",
                use_portal: true,
            }),
            "2" | "intranet" => Ok(Self {
                name: "内网 (校园网直连)",
                entry: "http://ydjwxs.yzu.edu.cn/",
                host: "ydjwxs.yzu.edu.cn",
                use_portal: false,
            }),
            other => Err(anyhow!("未知的网络方式: {other}（应为 1 或 2）")),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct FormOption {
    pub value: String,
    #[allow(dead_code)]
    pub label: String,
}

#[derive(Debug, Deserialize)]
pub struct FormQuestion {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub options: Vec<FormOption>,
}

#[derive(Debug, Deserialize)]
pub struct FormTextarea {
    pub name: String,
    #[allow(dead_code)]
    pub maxlength: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct FormResult {
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub hidden: HashMap<String, String>,
    #[serde(default)]
    pub questions: Vec<FormQuestion>,
    #[serde(default)]
    pub textarea: Option<FormTextarea>,
}

pub enum SubmitOutcome {
    DryRun { payload: Map<String, Value> },
    Done(Value),
}

pub struct Urp {
    browser: Browser,
    handler: JoinHandle<()>,
    page: Page,
    base: String,
}

impl Urp {
    pub async fn login(mode: NetMode, cred: &Credentials, headless: bool) -> Result<Self> {
        let mut builder = BrowserConfig::builder();
        if !headless {
            builder = builder.with_head();
        }
        let config = builder.build().map_err(|e| anyhow!(e))?;
        let (browser, mut handler) = Browser::launch(config)
            .await
            .context("无法启动浏览器（请确认已安装 Chrome / Chromium / Edge）")?;
        let handle = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        });

        let page = browser.new_page("about:blank").await?;

        println!("[1/4] 通过 {} 访问统一认证登录页 ...", mode.name);
        page.goto(mode.entry).await?;
        wait_for_selector(&page, "input[name=\"username\"]", 60_000)
            .await
            .context("未找到统一认证登录表单")?;
        sleep_ms(2000).await;

        page.find_element("input[name=\"username\"]")
            .await?
            .click()
            .await?
            .type_str(&cred.username)
            .await?;
        page.find_element("input[type=\"password\"]")
            .await?
            .click()
            .await?
            .type_str(&cred.password)
            .await?;
        sleep_ms(300).await;

        page.find_xpath("//button[contains(., '登')]")
            .await
            .context("未找到登录按钮")?
            .click()
            .await?;

        sleep_ms(2000).await;
        wait_for_login(&page, mode.host, 60_000).await?;
        sleep_ms(3000).await;
        println!("      统一认证登录成功");

        let (jw_page, base) = if mode.use_portal {
            enter_via_portal(&browser, &page).await?
        } else {
            enter_intranet(&page).await?
        };

        Ok(Self {
            browser,
            handler: handle,
            page: jw_page,
            base,
        })
    }

    pub async fn list_evaluations(&self, flag: &str) -> Result<Vec<Value>> {
        let api = format!("{}/student/teachingAssessment/evaluation/queryAll", self.base);
        let js = format!(
            r#"async () => {{
                const r = await fetch({api}, {{
                    method: 'POST',
                    headers: {{'Content-Type': 'application/x-www-form-urlencoded'}},
                    body: 'pageNum=1&pageSize=200&flag=' + {flag}
                }});
                return await r.text();
            }}"#,
            api = serde_json::to_string(&api)?,
            flag = serde_json::to_string(flag)?,
        );
        let text: String = self.page.evaluate_function(js.as_str()).await?.into_value()?;
        let data: Value = serde_json::from_str(&text)?;
        let records = data
            .get("data")
            .and_then(|d| d.get("records"))
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(records)
    }

    pub async fn open_evaluation(&self, ktid: &str) -> Result<FormResult> {
        let url = format!(
            "{}/student/teachingEvaluation/newEvaluation/evaluation/{ktid}",
            self.base
        );
        self.page.goto(url.as_str()).await?;
        sleep_ms(2000).await;
        self.extract_form().await
    }

    async fn extract_form(&self) -> Result<FormResult> {
        let js = r#"() => {
            const form = document.getElementById('saveEvaluation');
            if (!form) return {error: '未找到评测表单(可能已评估或已过期)'};
            const out = {hidden: {}, questions: [], textarea: null};
            form.querySelectorAll('input[type="hidden"]').forEach(i => out.hidden[i.name] = i.value);
            const qmap = {};
            form.querySelectorAll('input').forEach(i => {
                if (i.type === 'radio' || i.type === 'checkbox') {
                    if (!qmap[i.name]) qmap[i.name] = {type: i.type, options: []};
                    qmap[i.name].options.push({value: i.value, label: (i.parentElement ? i.parentElement.innerText : '').replace(/\s+/g, ' ').trim()});
                }
            });
            out.questions = Object.keys(qmap).map(k => ({name: k, type: qmap[k].type, options: qmap[k].options}));
            const ta = form.querySelector('textarea');
            out.textarea = ta ? {name: ta.name, maxlength: ta.maxLength} : null;
            return out;
        }"#;
        let form: FormResult = self.page.evaluate_function(js).await?.into_value()?;
        Ok(form)
    }

    pub async fn submit(
        &self,
        form: &FormResult,
        comment: &str,
        dry_run: bool,
    ) -> Result<SubmitOutcome> {
        let mut payload = Map::new();
        for q in &form.questions {
            if q.kind == "radio" {
                if let Some(first) = q.options.first() {
                    payload.insert(q.name.clone(), Value::String(first.value.clone()));
                }
            } else if q.kind == "checkbox" {
                let values: Vec<Value> = q
                    .options
                    .iter()
                    .map(|o| Value::String(o.value.clone()))
                    .collect();
                payload.insert(q.name.clone(), Value::Array(values));
            }
        }
        if let Some(ta) = &form.textarea {
            payload.insert(ta.name.clone(), Value::String(comment.to_string()));
        }

        let token = form.hidden.get("tokenValue").cloned().unwrap_or_default();

        if dry_run {
            return Ok(SubmitOutcome::DryRun { payload });
        }

        let js = format!(
            r#"async () => {{
                const hidden = {hidden};
                const payload = {payload};
                const fd = new FormData();
                fd.append('tjcs', hidden['tjcs'] || '1');
                fd.append('wjbm', hidden['wjbm'] || '');
                fd.append('ktid', hidden['ktid'] || '');
                fd.append('compare', '');
                for (const [k, v] of Object.entries(payload)) {{
                    if (Array.isArray(v)) {{ for (const x of v) fd.append(k, x); }}
                    else {{ fd.append(k, v); }}
                }}
                const r = await fetch('/student/teachingAssessment/baseInformation/questionsAdd/doSave?tokenValue=' + {token}, {{ method: 'POST', body: fd }});
                return await r.text();
            }}"#,
            hidden = serde_json::to_string(&form.hidden)?,
            payload = serde_json::to_string(&payload)?,
            token = serde_json::to_string(&token)?,
        );
        let text: String = self.page.evaluate_function(js.as_str()).await?.into_value()?;
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        Ok(SubmitOutcome::Done(value))
    }

    pub async fn close(mut self) {
        let _ = self.browser.close().await;
        self.handler.abort();
    }
}

async fn enter_via_portal(browser: &Browser, page: &Page) -> Result<(Page, String)> {
    println!("[2/4] 打开教务系统学生端 ...");
    page.find_xpath("//a[contains(., '教务系统学生端')]")
        .await
        .context("门户中未找到「教务系统学生端」入口")?
        .click()
        .await?;

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut jw_page: Option<Page> = None;
    while Instant::now() < deadline {
        sleep_ms(800).await;
        for p in browser.pages().await? {
            if let Some(url) = p.url().await? {
                if url.contains("/http/") {
                    jw_page = Some(p);
                    break;
                }
            }
        }
        if jw_page.is_some() {
            break;
        }
    }

    let jw_page = jw_page.ok_or_else(|| anyhow!("未能打开教务系统页面"))?;
    let url = jw_page.url().await?.unwrap_or_default();
    let base = url.replace("/index", "");
    println!("      教务系统就绪: {base}");
    Ok((jw_page, base))
}

async fn enter_intranet(page: &Page) -> Result<(Page, String)> {
    println!("[2/4] 进入教务系统 ...");
    sleep_ms(3000).await;
    let url = page.url().await?.unwrap_or_default();
    let base = url.trim_end_matches('/').to_string();
    println!("      教务系统就绪: {base}");
    Ok((page.clone(), base))
}

async fn wait_for_selector(page: &Page, selector: &str, timeout_ms: u64) -> Result<()> {
    let start = Instant::now();
    loop {
        if page.find_element(selector).await.is_ok() {
            return Ok(());
        }
        if start.elapsed().as_millis() as u64 >= timeout_ms {
            return Err(anyhow!("等待元素超时: {selector}"));
        }
        sleep_ms(300).await;
    }
}

async fn wait_for_login(page: &Page, host: &str, timeout_ms: u64) -> Result<String> {
    let start = Instant::now();
    loop {
        let url = page.url().await?.unwrap_or_default();
        if url.contains(host) && !url.contains("sso.yzu.edu.cn") {
            return Ok(url);
        }
        if start.elapsed().as_millis() as u64 >= timeout_ms {
            return Err(anyhow!("登录超时，当前页面: {url}"));
        }
        sleep_ms(500).await;
    }
}

async fn sleep_ms(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}
