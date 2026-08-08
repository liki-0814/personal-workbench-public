//! Interactive `pwcli config` wizard. Prompts for a single provider and saves
//! it to the user config. Intended for first-run / quick reconfigure.

use anyhow::Result;

use crate::ai::config::ProviderConfig;
use crate::app::config::{RuntimeConfig, UserConfig};
use crate::app::platform::identity::{resolve_user_slug_with_source, slugify};

/// Apply `--email <x>` non-interactively: write `config.user.{email,slug}` only.
pub async fn run_config_user(email: String) -> Result<()> {
    let mut config = RuntimeConfig::load();
    let mut user = config.user.clone().unwrap_or_default();
    user.email = Some(email.clone());
    user.id = Some(email.clone());
    user.slug = Some(slugify(&email));
    config.user = Some(user);
    config.save()?;
    println!("✅ 已写入 user.email = {}", email);
    println!(
        "   slug: {}",
        config
            .user
            .as_ref()
            .and_then(|u| u.slug.clone())
            .unwrap_or_default()
    );
    Ok(())
}

pub async fn run_config_wizard() -> Result<()> {
    use std::io::{stdin, stdout, Write};

    println!("🔧 pwcli 配置向导");
    println!("==================\n");

    let mut config = RuntimeConfig::load();
    let mut input = String::new();

    print!("Provider 名称 (如 OpenAI): ");
    stdout().flush()?;
    input.clear();
    stdin().read_line(&mut input)?;
    let name = input.trim().to_string();
    if name.is_empty() {
        println!("❌ 名称不能为空");
        return Ok(());
    }

    print!("Base URL (如 https://api.openai.com/v1): ");
    stdout().flush()?;
    input.clear();
    stdin().read_line(&mut input)?;
    let base_url = input.trim().to_string();
    if base_url.is_empty() {
        println!("❌ Base URL 不能为空");
        return Ok(());
    }

    print!("API Key: ");
    stdout().flush()?;
    input.clear();
    stdin().read_line(&mut input)?;
    let api_key = input.trim().to_string();
    if api_key.is_empty() {
        println!("❌ API Key 不能为空");
        return Ok(());
    }

    print!("协议类型 [openai/anthropic] (默认 openai): ");
    stdout().flush()?;
    input.clear();
    stdin().read_line(&mut input)?;
    let protocol = if input.trim() == "anthropic" {
        "anthropic".to_string()
    } else {
        "openai".to_string()
    };

    print!("模型 ID (如 gpt-4o): ");
    stdout().flush()?;
    input.clear();
    stdin().read_line(&mut input)?;
    let model = input.trim().to_string();
    if model.is_empty() {
        println!("❌ 模型 ID 不能为空");
        return Ok(());
    }

    let provider = ProviderConfig {
        name: name.clone(),
        base_url,
        api_key,
        protocol,
        model,
        models: Vec::new(),
        use_proxy: None,
        compat_profile: None,
    };

    config.providers = Some(vec![provider]);
    config.active_provider = Some(name.clone());

    // 可选：用户身份（用于长期记忆隔离 ~/.pwcli/memory/<slug>）
    let (auto_slug, auto_source) = resolve_user_slug_with_source(&config);
    print!(
        "\n用户身份 [推断 slug: {}, 源: {}]，是否手动指定 (y/N)? ",
        auto_slug,
        auto_source.label()
    );
    stdout().flush()?;
    input.clear();
    stdin().read_line(&mut input)?;
    if input.trim().eq_ignore_ascii_case("y") {
        print!("Email (例 alice@corp.com): ");
        stdout().flush()?;
        input.clear();
        stdin().read_line(&mut input)?;
        let email = input.trim().to_string();
        print!("姓名 (可选): ");
        stdout().flush()?;
        input.clear();
        stdin().read_line(&mut input)?;
        let user_name = input.trim().to_string();
        if !email.is_empty() {
            let slug = slugify(&email);
            config.user = Some(UserConfig {
                id: Some(email.clone()),
                name: if user_name.is_empty() {
                    None
                } else {
                    Some(user_name)
                },
                email: Some(email),
                slug: Some(slug),
            });
        }
    }

    config.save()?;

    println!("\n✅ 配置已保存到 ~/.pwcli/config.json");
    println!("   Provider: {}", name);
    if let Some(u) = config.user.as_ref().and_then(|u| u.slug.as_deref()) {
        println!("   User slug: {}", u);
    }
    println!("   现在可以运行 `pwcli` 开始使用了");

    Ok(())
}
