//! `ironteams-cli accounts …`: several signed-in accounts and organization (tenant) switching.
//!
//! `accounts`, `accounts tenants` only read. `use`, `add`, `remove` and `org` change the config
//! (and the keyring), so they act on the account you name.

use anyhow::{Context, Result, bail};
use session::{Config, Paths, SystemKeyring, TokenStore};
use teams_proto::Client;

pub const USAGE: &str =
    "ironteams-cli accounts [tenants|use <label>|add <label>|remove <label>|org <tenant-id|home>]";

fn when(ms: Option<i64>) -> String {
    ms.map_or("never".into(), |ms| {
        format!("{}s ago", (teams_proto::time::now_ms() - ms) / 1000)
    })
}

/// One line per account.
fn list(cfg: &Config, store: &dyn TokenStore) -> Result<Vec<String>> {
    cfg.accounts
        .iter()
        .map(|a| {
            let probe = Config {
                account: a.label.clone(),
                tenant: a.tenant.clone(),
                epid: a.epid.clone(),
                accounts: vec![a.clone()],
                ..Config::default()
            };
            let signed_in = session::stored_session_in(store, &probe)?.is_some();
            Ok(format!(
                "{} {:<12} tenant {:<14} {} orgs cached {}, last used {}{}",
                if a.label == cfg.account { "*" } else { " " },
                a.label,
                a.tenant,
                if signed_in { "signed in" } else { "signed out" },
                a.orgs.len(),
                when(a.last_used_ms),
                a.display_name
                    .as_deref()
                    .map(|n| format!(", {n}"))
                    .unwrap_or_default(),
            ))
        })
        .collect()
}

pub async fn run(paths: &Paths, args: &[String]) -> Result<()> {
    let mut cfg = paths.load_config()?;
    let store = SystemKeyring;
    match args.first().map(String::as_str) {
        None | Some("list") => {
            for l in list(&cfg, &store)? {
                println!("{l}");
            }
        }
        Some("tenants") => {
            let (client, cfg) = crate::connect(paths).await?;
            let tenants = client.tenants().await;
            // Keep a refreshed token even when the list call failed.
            session::persist(&cfg, &client).await?;
            let tenants = tenants?;
            println!("{} tenants", tenants.len());
            for t in &tenants {
                let active = cfg.tenant == t.id
                    || (t.is_home && cfg.tenant == teams_proto::client_info::DEFAULT_TENANT);
                println!(
                    "{} {} {:<8} {:<8} {}",
                    if active { "*" } else { " " },
                    t.id,
                    if t.is_home { "home" } else { "guest" },
                    t.user_type.as_deref().unwrap_or("-"),
                    t.name
                );
            }
        }
        Some("use") => {
            let label = args.get(1).context("usage: accounts use <label>")?;
            cfg.switch_account(label)?;
            paths.save_config(&cfg)?;
            println!("Active account: {label}");
        }
        Some("add") => {
            let label = args.get(1).context("usage: accounts add <label>")?;
            cfg.add_account(label)?;
            cfg.switch_account(label)?;
            let client = Client::sign_in_device_code(&cfg.tenant, &cfg.epid, |dc| {
                println!(
                    "Open {} and enter the code: {}",
                    dc.verification_url, dc.user_code
                );
            })
            .await?;
            session::persist(&cfg, &client).await?;
            let me = client.self_mri().await?;
            let profile = client.profiles(std::slice::from_ref(&me)).await?;
            cfg.self_mri = Some(me.clone());
            cfg.display_name = profile.first().map(|p| p.display_name.clone());
            cfg.sync_active();
            if let Some(a) = cfg.accounts.iter_mut().find(|a| a.label == *label) {
                a.upn = profile.first().and_then(|p| p.email.clone());
            }
            paths.save_config(&cfg)?;
            println!("Signed in as {me}; {label} is now the active account.");
        }
        Some("remove") => {
            let label = args.get(1).context("usage: accounts remove <label>")?;
            let account = cfg
                .account_by_label(label)
                .with_context(|| format!("no account {label:?}"))?
                .clone();
            session::forget_account_tokens(&store, &account)?;
            cfg.remove_account(label)?;
            paths.save_config(&cfg)?;
            println!("Removed {label}. Active account: {}", cfg.account);
        }
        Some("org") => {
            let target = args
                .get(1)
                .context("usage: accounts org <tenant-id|home>")?;
            let tenant = if target == "home" {
                teams_proto::client_info::DEFAULT_TENANT
            } else {
                target.as_str()
            };
            cfg.switch_tenant(tenant)?;
            let client = match session::stored_session_in(&store, &cfg)? {
                Some(s) => match Client::resume(s).await {
                    Ok(c) => c,
                    // Conditional access or MFA of the guest tenant: sign in there instead.
                    Err(
                        teams_proto::Error::Auth(msg)
                        | teams_proto::Error::PolicyBlocked { detail: msg, .. },
                    ) => {
                        eprintln!("{tenant} needs a fresh sign-in ({msg}); starting a device code");
                        device_code(&cfg).await?
                    }
                    Err(e) => return Err(e.into()),
                },
                None => bail!("account {:?} is not signed in", cfg.account),
            };
            session::persist(&cfg, &client).await?;
            paths.save_config(&cfg)?;
            println!("Account {} now works in {tenant}", cfg.account);
        }
        Some(other) => bail!("unknown accounts command {other:?}; usage: {USAGE}"),
    }
    Ok(())
}

async fn device_code(cfg: &Config) -> Result<Client> {
    Ok(Client::sign_in_device_code(&cfg.tenant, &cfg.epid, |dc| {
        println!(
            "Open {} and enter the code: {}",
            dc.verification_url, dc.user_code
        );
    })
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use session::MemoryStore;

    #[test]
    fn list_marks_the_active_account_and_who_is_signed_in() {
        let store = MemoryStore::default();
        store.set("default", "rt").unwrap();
        let mut cfg = Config::default();
        cfg.normalize();
        cfg.add_account("work").unwrap();
        let lines = list(&cfg, &store).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("* default") && lines[0].contains("signed in"));
        assert!(lines[1].starts_with("  work") && lines[1].contains("signed out"));
    }

    #[test]
    fn list_looks_at_the_accounts_own_tenant_key() {
        let store = MemoryStore::default();
        store.set("work@guest-1", "rt").unwrap();
        let mut cfg = Config::default();
        cfg.normalize();
        cfg.add_account("work").unwrap();
        cfg.switch_account("work").unwrap();
        cfg.switch_tenant("guest-1").unwrap();
        let lines = list(&cfg, &store).unwrap();
        assert!(lines[1].contains("signed in") && lines[1].contains("guest-1"));
    }
}
