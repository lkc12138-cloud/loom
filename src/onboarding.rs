//! Local, hidden-input onboarding and interactive terminal control center.
use crate::{
    admin,
    config::{self, Result},
    desktop, install,
    manage::{self, PRESETS, PresetInfo, models_json, preset_provider},
    network, validate_key,
};
use serde_json::{Value, json};
use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
};

trait Input {
    fn line(&mut self, prompt: &str) -> Result<String>;
    fn secret(&mut self, prompt: &str) -> Result<String>;
    fn say(&mut self, message: &str);
}
struct Terminal;
impl Input for Terminal {
    fn line(&mut self, prompt: &str) -> Result<String> {
        config::prompt(prompt)
    }
    fn secret(&mut self, prompt: &str) -> Result<String> {
        Ok(rpassword::prompt_password(format!("{prompt}: "))?)
    }
    fn say(&mut self, message: &str) {
        println!("{message}");
    }
}

fn selection(s: &str) -> Result<Vec<usize>> {
    if s.trim().is_empty() || s.trim().eq_ignore_ascii_case("all") || s.trim() == "全部" {
        return Ok((0..PRESETS.len()).collect());
    }
    if s.trim() == "0" {
        return Ok(vec![]);
    }
    let mut chosen = vec![];
    for part in s
        .split(|c: char| c == ',' || c == '，' || c.is_whitespace())
        .filter(|p| !p.is_empty())
    {
        let index = part
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|n| *n < PRESETS.len())
            .or_else(|| PRESETS.iter().position(|p| p.id.eq_ignore_ascii_case(part)))
            .ok_or("请选择列表中的编号或供应商名称")?;
        if !chosen.contains(&index) {
            chosen.push(index);
        }
    }
    if chosen.is_empty() {
        return Err("empty selection".into());
    }
    Ok(chosen)
}
pub fn needs_setup(d: &Path) -> Result<bool> {
    if !d.join("providers.json").exists() {
        return Ok(true);
    }
    let r = config::read(&d.join("providers.json"))?;
    Ok(r["providers"]
        .as_object()
        .ok_or("providers required")?
        .is_empty())
}
pub fn configure(d: &Path, requested: &[String], custom: bool) -> Result<()> {
    let selected = if requested.is_empty() {
        None
    } else {
        let chosen = selection(&requested.join(",")).map_err(|_| {
            eprintln!("供应商可选：zenmux / glm / minimax / deepseek / kimi / longcat；其他供应商用 loom add --custom。");
            "unknown provider"
        })?;
        Some(chosen)
    };
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!("请在本机交互终端运行 loom add；API Key 只接受隐藏输入，不接受命令参数或管道。");
        return Err("interactive terminal required".into());
    }
    wizard(d, &config::home(), selected, custom, &mut Terminal)
}
fn wizard(
    d: &Path,
    h: &Path,
    selected: Option<Vec<usize>>,
    custom: bool,
    io: &mut dyn Input,
) -> Result<()> {
    config::init(d)?;
    // Freeze absolute key paths even when --data-dir is relative.
    let d = std::fs::canonicalize(d)?;
    let original = config::read(&d.join("providers.json"))?;
    config::catalog(&original)?;
    if custom {
        custom_provider(&d, io)?;
    } else {
        let selected = match selected {
            Some(s) => s,
            None => {
                io.say("\n选择要配置的模型厂商（默认全选，无 Key 的厂商直接回车跳过即可）：");
                let configured_map = original["providers"].as_object();
                for (i, p) in PRESETS.iter().enumerate() {
                    let is_configured = configured_map.is_some_and(|m| {
                        m.iter()
                            .any(|(pid, v)| config::provider_prefix(pid, v) == p.id)
                    });
                    let badge = if is_configured {
                        "已配置"
                    } else {
                        "未配置"
                    };
                    io.say(&format!(
                        "  [{}] {}. {}（{} · {}）",
                        badge,
                        i + 1,
                        p.name,
                        p.region_hint,
                        p.key_url
                    ));
                }
                loop {
                    let answer = io.line("回车全部 / 编号多选如 1,4 / 0 退出")?;
                    match selection(&answer) {
                        Ok(s) => break s,
                        Err(_) => io.say(&format!(
                            "请输入 1–{} 的编号（逗号分隔）或供应商名称；回车选择全部。",
                            PRESETS.len()
                        )),
                    }
                }
            }
        };
        for index in selected {
            configure_preset(&d, &PRESETS[index], io)?;
        }
    }
    if config::catalog(&config::read(&d.join("providers.json"))?)?.is_empty() {
        io.say("未添加供应商。之后运行 loom add、loom 或在桌面/网页端随时可继续配置。");
    } else {
        config::sync(&d, h).inspect_err(|_| {
            io.say("Key 已保存在本机；客户端同步未完成，处理客户端配置后运行 loom sync。")
        })?;
        io.say("已保存并同步客户端。配置即时生效，可运行 loom doctor --ping 验证连通性。");
    }
    Ok(())
}
fn read_key(io: &mut dyn Input, prompt: &str) -> Result<String> {
    loop {
        let key = io.secret(prompt)?;
        if key.trim().is_empty() {
            return Ok(String::new());
        }
        if validate_key(&key).is_ok() {
            return Ok(key.trim().to_string());
        }
        io.say("Key 格式无效，请重新粘贴；回车跳过。未保存此次输入。");
    }
}
fn configure_preset(d: &Path, p: &PresetInfo, io: &mut dyn Input) -> Result<()> {
    let registry = d.join("providers.json");
    let before = std::fs::read(&registry)?;
    let mut r: Value = serde_json::from_slice(&before)?;
    let matches: Vec<_> = r["providers"]
        .as_object()
        .ok_or("providers required")?
        .iter()
        .filter(|(pid, v)| config::provider_prefix(pid, v) == p.id)
        .map(|(pid, _)| pid.clone())
        .collect();
    if matches.len() > 1 {
        return Err("ambiguous existing providers; use loom set-key with exact id".into());
    }
    let existing = matches.first();
    let pid = existing
        .cloned()
        .unwrap_or_else(|| format!("local-{}", p.id));
    let key_path = if existing.is_some() {
        PathBuf::from(
            r["providers"][&pid]["keyFile"]
                .as_str()
                .ok_or("keyFile required")?,
        )
    } else {
        if r["providers"].get(&pid).is_some() {
            return Err("provider id already occupied".into());
        }
        d.join("keys").join(format!("{pid}.key"))
    };
    let action = if existing.is_some() {
        "已配置，回车保留 / 输入新 Key 更换"
    } else {
        "回车跳过"
    };
    io.say(&format!(
        "\n{} · {}\n  获取地址：{}\n  提示：{}",
        p.name, p.key_hint, p.key_url, p.pitfall_hint
    ));
    let key = read_key(io, &format!("API Key（隐藏输入；{action}）"))?;
    if key.is_empty() {
        return Ok(());
    }
    if std::fs::read(&registry)? != before {
        return Err("configuration changed; retry".into());
    }
    if existing.is_some() {
        // Key rotation must preserve every user-defined field, path, disabled state and model.
        config::backup(&key_path, d)?;
        config::write(&key_path, key.as_bytes())?;
    } else {
        if key_path.exists() {
            return Err("key file already exists; not overwritten".into());
        }
        r["providers"][&pid] = preset_provider(p, &key_path);
        config::catalog(&r)?;
        config::backup(&registry, d)?;
        config::write(&key_path, key.as_bytes())?;
        if let Err(e) = config::save(&registry, &r) {
            let _ = std::fs::remove_file(&key_path);
            return Err(e);
        }
    }
    io.say("已保存。Key 只保留在本机。");
    Ok(())
}
fn with_default(io: &mut dyn Input, prompt: &str, default: &str) -> Result<String> {
    let s = io.line(&format!("{prompt} [{default}]"))?;
    Ok(if s.is_empty() { default.into() } else { s })
}
fn custom_provider(d: &Path, io: &mut dyn Input) -> Result<()> {
    let prefix = io.line("自定义供应商标识（小写字母、数字、短横线）")?;
    let name = with_default(io, "显示名称", &prefix)?;
    let base = config::endpoint(&io.line("OpenAI 兼容 Base URL（HTTPS）")?)?;
    let native = io.line("Anthropic Base URL（可留空）")?;
    if !native.is_empty() {
        config::endpoint(&native)?;
    }
    let billing = with_default(io, "计费来源", "独立 API 钱包")?;
    let ids = io.line("Model ID（逗号分隔）")?;
    let ids: Vec<_> = ids
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| (s, s))
        .collect();
    if ids.is_empty() {
        return Err("models required".into());
    }
    let registry = d.join("providers.json");
    let before = std::fs::read(&registry)?;
    let mut r: Value = serde_json::from_slice(&before)?;
    let pid = format!("local-{prefix}");
    let key_path = d.join("keys").join(format!("{pid}.key"));
    if r["providers"].get(&pid).is_some() || key_path.exists() {
        return Err("provider exists; use loom set-key".into());
    }
    let mut v = json!({"prefix":prefix,"name":name,"baseURL":base,"billing":billing,
        "keyFile":key_path,"enabled":true,"models":models_json(&ids, false)});
    if !native.is_empty() {
        v["anthropicBaseURL"] = json!(native);
    }
    r["providers"][&pid] = v;
    config::catalog(&r)?;
    let key = read_key(io, "推理 API Key（隐藏输入；回车取消）")?;
    if key.is_empty() {
        return Ok(());
    }
    if std::fs::read(&registry)? != before {
        return Err("configuration changed; retry".into());
    }
    config::backup(&registry, d)?;
    config::write(&key_path, key.as_bytes())?;
    if let Err(e) = config::save(&registry, &r) {
        let _ = std::fs::remove_file(&key_path);
        return Err(e);
    }
    Ok(())
}

pub async fn print_summary(d: &Path) -> Result<()> {
    let home = config::home();
    let snap = manage::setup_snapshot(d, &home, 4319, None).await?;
    let gw_ok = snap["gateway"]["healthy"] == true;
    let ready = snap["ready_model_count"].as_u64().unwrap_or(0);
    let providers_len = snap["providers"].as_array().map_or(0, Vec::len);
    println!("Loom 本机统一模型网关");
    println!(
        "  网关服务: {}",
        if gw_ok {
            "● 运行中 (http://127.0.0.1:4319/v1)"
        } else {
            "○ 未运行（运行 loom install 或 loom serve 启动）"
        }
    );
    println!("  模型配置: {providers_len} 个厂商 · {ready} 个可用模型");
    println!(
        "  出站网络: {}",
        if snap["proxy"]["configured"] == true {
            "已配置持久代理"
        } else {
            "直连"
        }
    );
    println!("\n常用命令：");
    println!("  loom add            选择预设厂商并粘贴 API Key");
    println!("  loom doctor --ping  全面检查配置并实测上游连通性");
    println!("  loom console        打开本机可视化工作台（浏览器）");
    if cfg!(target_os = "macos") {
        println!("  loom app            启动/安装 macOS 原生应用 Loom.app");
    }
    println!("  loom install        安装并启动开机自启后台服务");
    println!("  loom sync           同步 pi / OpenCode / Claude Code 配置");
    Ok(())
}

pub async fn interactive_menu(d: &Path) -> Result<()> {
    let home = config::home();
    loop {
        let snap = manage::setup_snapshot(d, &home, 4319, None).await?;
        let gw_ok = snap["gateway"]["healthy"] == true;
        let ready = snap["ready_model_count"].as_u64().unwrap_or(0);
        let providers = snap["providers"].as_array().cloned().unwrap_or_default();
        let proxy_on = snap["proxy"]["configured"] == true;
        let pi_ok = snap["clients"]["pi"]["synced"] == true;
        let oc_ok = snap["clients"]["opencode2"]["synced"] == true;
        let cc_ok = snap["clients"]["claude_code"]["synced"] == true;

        println!("\n╭──────────────────────────────────────────────╮");
        println!("│  ⌘ Loom 本机模型网关 · 新手控制中心           │");
        println!("╰──────────────────────────────────────────────╯");
        println!(
            "  后台网关: {}   可用模型: {} 个",
            if gw_ok {
                "● 运行中 (127.0.0.1:4319)"
            } else {
                "○ 未运行"
            },
            ready
        );
        println!(
            "  出站网络: {}   客户端同步: pi {} · OpenCode {} · Claude {}",
            if proxy_on {
                "已配置持久代理"
            } else {
                "直连"
            },
            if pi_ok { "✓" } else { "-" },
            if oc_ok { "✓" } else { "-" },
            if cc_ok { "✓" } else { "-" }
        );
        if providers.is_empty() {
            println!(
                "  提示: 尚未添加任何模型厂商，建议先选 [1] 添加 API Key 或选 [4] 在网页向导中完成配置。"
            );
        } else {
            let names: Vec<String> = providers
                .iter()
                .map(|p| {
                    let n = p["name"].as_str().unwrap_or("?");
                    let mark = if p["enabled"] == true && p["has_key"] == true {
                        "✓"
                    } else {
                        "!"
                    };
                    format!("{n}[{mark}]")
                })
                .collect();
            println!("  已配置厂商: {}", names.join(" · "));
        }

        println!("\n请选择操作：");
        println!(
            "  1. 配置 / 更新预设模型厂商 Key（ZenMux / GLM / MiniMax / DeepSeek / Kimi / LongCat）"
        );
        println!("  2. 添加自定义 HTTPS 兼容厂商");
        println!("  3. 运行环境诊断与真实连通性测试 (doctor --ping)");
        println!("  4. 打开 Web 可视化工作台 (console)");
        if cfg!(target_os = "macos") {
            println!("  5. 启动 / 安装 macOS 原生应用 Loom.app");
        } else {
            println!("  5. 查看桌面端支持说明");
        }
        println!("  6. 设置或关闭本机出站代理 (proxy)");
        println!("  7. 安装 / 重启后台自启动网关服务 (install)");
        println!("  8. 同步本地 AI 编程客户端 (sync)");
        println!("  0. 退出\n");

        let choice = config::prompt("输入选项编号 [0-8]")?;
        let res: Result<()> = match choice.trim() {
            "" | "0" | "q" | "exit" => {
                println!("已退出。后台网关如已启动将继续在 127.0.0.1:4319 运行。");
                break;
            }
            "1" => wizard(d, &home, None, false, &mut Terminal),
            "2" => wizard(d, &home, None, true, &mut Terminal),
            "3" => manage::run_doctor(d, &home, 4319, 4320, true, None).await,
            "4" => {
                if !gw_ok {
                    println!("提示：后台服务尚未运行，建议先选 [7] 安装并启动后台服务。");
                    Ok(())
                } else {
                    admin::open_console(d, 4320, "loom.localhost", None).await
                }
            }
            "5" => desktop::launch_app(d, None, false, false),
            "6" => {
                let raw = config::prompt(
                    "输入 HTTP/HTTPS 代理地址（如 http://127.0.0.1:7890；输入 off 恢复直连；直接回车取消）",
                )?;
                if raw.trim().is_empty() {
                    Ok(())
                } else {
                    network::proxy(d, Some(raw.trim()))
                }
            }
            "7" => install::install(d.to_path_buf(), false).await,
            "8" => {
                config::sync(d, &home)?;
                println!("已同步 pi / OpenCode 和已有 Claude Code 配置。");
                Ok(())
            }
            _ => {
                println!("无效选项，请输入 0–8 的数字。");
                Ok(())
            }
        };
        if let Err(e) = res {
            eprintln!("提示：{e}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Script {
        lines: VecDeque<&'static str>,
        keys: VecDeque<&'static str>,
        output: String,
    }
    impl Script {
        fn new(lines: &[&'static str], keys: &[&'static str]) -> Self {
            Self {
                lines: lines.iter().copied().collect(),
                keys: keys.iter().copied().collect(),
                output: String::new(),
            }
        }
    }
    impl Input for Script {
        fn line(&mut self, _: &str) -> Result<String> {
            Ok(self
                .lines
                .pop_front()
                .expect("unexpected non-Key prompt")
                .into())
        }
        fn secret(&mut self, _: &str) -> Result<String> {
            Ok(self.keys.pop_front().expect("unexpected Key prompt").into())
        }
        fn say(&mut self, s: &str) {
            self.output.push_str(s);
        }
    }
    #[test]
    fn selects_all_by_default_and_supports_names_numbers_and_deduplication() {
        assert_eq!(
            selection("").unwrap(),
            (0..PRESETS.len()).collect::<Vec<_>>()
        );
        assert_eq!(selection("1,4,1").unwrap(), vec![0, 3]);
        assert_eq!(selection("kimi，DeepSeek").unwrap(), vec![4, 3]);
        assert!(selection("0").unwrap().is_empty());
        assert_eq!(selection("LongCat").unwrap(), vec![5]);
        for bad in ["7", "1,unknown", "0,1", ","] {
            assert!(selection(bad).is_err());
        }
    }
    #[test]
    fn default_flow_only_asks_for_selection_and_keys_skips_empty_and_syncs() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("data");
        let h = t.path().join("home");
        config::save(
            &h.join(".claude/settings.json"),
            &json!({"permissions":{"defaultMode":"manual"}}),
        )
        .unwrap();
        let mut io = Script::new(
            &[""],
            &["ZENMUX_FIXTURE", "", "", "DEEPSEEK_FIXTURE", "", ""],
        );
        wizard(&d, &h, None, false, &mut io).unwrap();
        assert!(io.lines.is_empty() && io.keys.is_empty());
        let r = config::read(&d.join("providers.json")).unwrap();
        assert_eq!(r["providers"].as_object().unwrap().len(), 2);
        assert_eq!(r["providers"]["local-zenmux"]["baseURL"], PRESETS[0].base);
        assert_eq!(
            r["providers"]["local-zenmux"]["models"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            config::catalog(&r)
                .unwrap()
                .iter()
                .filter(|m| m.id.starts_with("zenmux/"))
                .count(),
            7
        );
        assert!(!io.output.contains("ZENMUX_FIXTURE"));
        for file in [
            ".pi/agent/models.json",
            ".config/opencode/opencode.json",
            ".claude/settings.json",
        ] {
            let bytes = std::fs::read_to_string(h.join(file)).unwrap();
            assert!(!bytes.contains("ZENMUX_FIXTURE") && !bytes.contains("DEEPSEEK_FIXTURE"));
        }
        assert_eq!(
            config::read(&h.join(".claude/settings.json")).unwrap()["permissions"]["defaultMode"],
            "manual"
        );
    }
    #[test]
    fn all_presets_validate_with_separate_billing_and_native_routes() {
        let t = tempfile::tempdir().unwrap();
        let mut io = Script::new(
            &[],
            &[
                "FIXTURE_A",
                "FIXTURE_B",
                "FIXTURE_C",
                "FIXTURE_D",
                "FIXTURE_E",
                "FIXTURE_F",
            ],
        );
        wizard(
            t.path(),
            &t.path().join("home"),
            Some((0..PRESETS.len()).collect()),
            false,
            &mut io,
        )
        .unwrap();
        let r = config::read(&t.path().join("providers.json")).unwrap();
        let catalog = config::catalog(&r).unwrap();
        assert_eq!(catalog.len(), 28);
        for m in catalog {
            assert!(config::anthropic(&m.p).unwrap().is_some());
        }
        assert_eq!(
            r["providers"]["local-glm"]["baseURL"],
            "https://open.bigmodel.cn/api/coding/paas/v4"
        );
        assert_eq!(
            r["providers"]["local-longcat"]["models"][0]["id"],
            "LongCat-2.5"
        );
        assert_eq!(
            config::anthropic(&r["providers"]["local-longcat"])
                .unwrap()
                .unwrap(),
            "https://api.longcat.chat/anthropic/v1/messages"
        );
        assert_eq!(
            r["providers"]["local-longcat"]["piCompat"]["supportsReasoningEffort"],
            false
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in PRESETS {
                let mode = std::fs::metadata(t.path().join(format!("keys/local-{}.key", p.id)))
                    .unwrap()
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o777, 0o600);
            }
        }
    }
    #[test]
    fn keep_and_rotate_legacy_provider_without_replacing_any_configuration() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let h = d.join("home");
        config::init(d).unwrap();
        let kp = d.join("keys/original.key");
        config::write(&kp, b"ORIGINAL_FIXTURE").unwrap();
        let r = json!({"proxy":"https://proxy.invalid", "extra":"keep", "providers":{"local-glm-plan":{
            "enabled":true,"baseURL":PRESETS[1].base,"keyFile":kp,"name":"My Name","models":[{"id":"custom-model"}],"extra":"keep"}}});
        config::save(&d.join("providers.json"), &r).unwrap();
        let original = std::fs::read(d.join("providers.json")).unwrap();
        let token = std::fs::read(d.join("hub.key")).unwrap();
        let mut io = Script::new(&[], &[""]);
        wizard(d, &h, Some(vec![1]), false, &mut io).unwrap();
        assert_eq!(std::fs::read(&kp).unwrap(), b"ORIGINAL_FIXTURE");
        let mut io = Script::new(&[], &["NEW_FIXTURE"]);
        wizard(d, &h, Some(vec![1]), false, &mut io).unwrap();
        assert_eq!(std::fs::read(&kp).unwrap(), b"NEW_FIXTURE");
        assert_eq!(std::fs::read(d.join("providers.json")).unwrap(), original);
        assert_eq!(std::fs::read(d.join("hub.key")).unwrap(), token);
        assert!(
            std::fs::read_dir(d.join("backups"))
                .unwrap()
                .any(|p| std::fs::read(p.unwrap().path()).unwrap() == b"ORIGINAL_FIXTURE")
        );
    }
    #[test]
    fn no_key_means_no_provider_or_client_configuration() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().join("home");
        let mut io = Script::new(&[""], &["", "", "", "", "", ""]);
        wizard(t.path(), &h, None, false, &mut io).unwrap();
        assert!(needs_setup(t.path()).unwrap());
        assert!(!h.exists());
        assert!(!t.path().join("keys").exists());
    }
    #[test]
    fn invalid_input_retries_without_saving_or_printing_it() {
        let t = tempfile::tempdir().unwrap();
        let mut io = Script::new(&["7", "4"], &["BAD FIXTURE", "VALID_FIXTURE"]);
        wizard(t.path(), &t.path().join("home"), None, false, &mut io).unwrap();
        assert_eq!(
            std::fs::read(t.path().join("keys/local-deepseek.key")).unwrap(),
            b"VALID_FIXTURE"
        );
        assert!(!io.output.contains("BAD FIXTURE") && !io.output.contains("VALID_FIXTURE"));
    }
    #[test]
    fn custom_provider_keeps_manual_routes_with_default_name_and_billing() {
        let t = tempfile::tempdir().unwrap();
        let mut io = Script::new(
            &[
                "demo",
                "",
                "https://example.invalid/v1",
                "",
                "",
                "demo-model",
            ],
            &["CUSTOM_FIXTURE"],
        );
        wizard(t.path(), &t.path().join("home"), None, true, &mut io).unwrap();
        let r = config::read(&t.path().join("providers.json")).unwrap();
        assert_eq!(r["providers"]["local-demo"]["name"], "demo");
        assert_eq!(r["providers"]["local-demo"]["billing"], "独立 API 钱包");
    }
    #[test]
    fn ambiguous_existing_routes_are_rejected_before_requesting_a_key() {
        let t = tempfile::tempdir().unwrap();
        config::init(t.path()).unwrap();
        let r = json!({"providers":{
            "first":{"prefix":"zenmux","enabled":false,"keyFile":"first-fixture"},
            "second":{"prefix":"zenmux","enabled":false,"keyFile":"second-fixture"}
        }});
        config::save(&t.path().join("providers.json"), &r).unwrap();
        let mut io = Script::new(&[], &[]);
        assert!(
            wizard(
                t.path(),
                &t.path().join("home"),
                Some(vec![0]),
                false,
                &mut io
            )
            .is_err()
        );
        assert_eq!(config::read(&t.path().join("providers.json")).unwrap(), r);
        assert!(!t.path().join("keys").exists());
    }
}
