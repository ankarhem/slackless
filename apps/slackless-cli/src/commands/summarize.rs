use std::collections::HashMap;
use std::process::ExitCode;

use anyhow::{Context, Result};
use slackless_core::auth::{AI_SUMMARY_API_KEY_ACCOUNT, SecretStore, SessionStore};
use slackless_core::summarize::{
    AiProvider, AiSummaryConfig, SummaryTarget, load_channel_target, load_thread_target,
    load_transcript_target, summarize,
};
use slackless_core::types::time::now_epoch_secs;
use slackless_core::types::{FileId, TeamId, Ts};

use crate::archive::Archive;
use crate::cli::SummarizeArgs;
use crate::commands::browse::{build_catchup_for_archives, parse_catchup_window};
use crate::commands::{all_workspace_ids, empty_local, open_archive_with_hint, raw_channel_id};
use crate::{Ctx, fail, output};

fn infer_provider_from_endpoint(endpoint: Option<&str>) -> AiProvider {
    let Some(raw) = endpoint else {
        return AiProvider::Ollama;
    };
    let Ok(parsed) = slackless_core::summarize::validate_base_url(raw) else {
        return AiProvider::Custom;
    };
    if parsed.scheme() == "https"
        && parsed
            .host_str()
            .is_some_and(|h| h.eq_ignore_ascii_case("api.openai.com"))
    {
        return AiProvider::OpenAi;
    }
    if slackless_core::summarize::is_loopback_url(&parsed) {
        match parsed.port() {
            Some(1234) => return AiProvider::LmStudio,
            Some(11434) => return AiProvider::Ollama,
            _ => {}
        }
    }
    AiProvider::Custom
}

fn resolve_ai_config(args: &SummarizeArgs) -> Result<AiSummaryConfig, String> {
    let endpoint_trimmed = args
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let provider = match args.provider {
        Some(p) => AiProvider::from(p),
        None => infer_provider_from_endpoint(endpoint_trimmed),
    };

    let base_url = endpoint_trimmed
        .map(str::to_owned)
        .unwrap_or_else(|| provider.default_base_url().to_owned());

    let model = args
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| provider.default_model().to_owned());

    let api_key = if let Some(var_name) = args
        .api_key_env
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match std::env::var(var_name) {
            Ok(val) if !val.trim().is_empty() => Some(val.trim().to_owned()),
            _ => {
                return Err(format!(
                    "environment variable `{var_name}` (passed to --api-key-env) is unset or empty"
                ));
            }
        }
    } else if provider.requires_api_key() {
        std::env::var("OPENAI_API_KEY")
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .or_else(|| {
                SecretStore::new()
                    .ok()
                    .and_then(|store| store.get(AI_SUMMARY_API_KEY_ACCOUNT).ok().flatten())
            })
    } else {
        None
    };

    let config = AiSummaryConfig {
        provider,
        base_url,
        model,
        api_key,
        timeout_secs: args.timeout,
        ..AiSummaryConfig::default()
    };
    config.validate().map_err(|err| err.to_string())?;
    Ok(config)
}

pub(crate) async fn cmd_summarize(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    args: &SummarizeArgs,
) -> Result<ExitCode> {
    let config = match resolve_ai_config(args) {
        Ok(cfg) => cfg,
        Err(msg) => return Ok(fail(ctx, "invalid_config", &msg)),
    };

    if args.catchup {
        let now_secs = now_epoch_secs();
        let window = match parse_catchup_window(args.since.as_deref(), now_secs) {
            Ok(w) => w,
            Err(msg) => return Ok(fail(ctx, "invalid_args", &msg)),
        };
        let team_ids: Vec<TeamId> = if args.all_workspaces {
            all_workspace_ids()
        } else {
            workspace.into_iter().cloned().collect()
        };
        if team_ids.is_empty() {
            return Ok(empty_local(
                ctx,
                serde_json::json!({"ok": false, "error": "no_workspace"}),
                "summarizing catch-up",
            ));
        }
        let session_users: HashMap<TeamId, slackless_core::types::UserId> = SessionStore::new()
            .and_then(|s| s.load())
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.team_id, s.user_id))
            .collect();
        let mut opened = Vec::new();
        for ws in team_ids {
            let Some(archive) =
                Archive::open(&ws).with_context(|| format!("opening local archive for {ws}"))?
            else {
                continue;
            };
            let self_user = session_users.get(&ws).cloned();
            opened.push((ws, self_user, archive));
        }
        let refs: Vec<(&TeamId, Option<&slackless_core::types::UserId>, &Archive)> = opened
            .iter()
            .map(|(ws, user, arch)| (ws, user.as_ref(), arch))
            .collect();
        let briefing = build_catchup_for_archives(&refs, window, now_secs, args.limit)?;
        return run_summarize_target(ctx, &config, SummaryTarget::CatchUp { briefing }).await;
    }

    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"ok": false, "error": "no_workspace"}),
            "summarizing",
        ));
    };
    let archive = open_archive_with_hint(ctx, workspace)?;
    cmd_summarize_in_archive(ctx, workspace, archive.as_ref(), &config, args).await
}

pub(crate) async fn cmd_summarize_in_archive(
    ctx: &Ctx,
    workspace: &TeamId,
    archive: Option<&Archive>,
    config: &AiSummaryConfig,
    args: &SummarizeArgs,
) -> Result<ExitCode> {
    if let Some(file_id) = args.transcript.as_deref() {
        let Some(archive) = archive else {
            return Ok(fail(
                ctx,
                "not_found",
                &format!(
                    "no local archive for workspace {workspace}; transcript {file_id} not found"
                ),
            ));
        };
        let fid = FileId::from(file_id);
        let target = match load_transcript_target(archive.store(), workspace, &fid) {
            Ok(t) => t,
            Err(err) => return Ok(fail(ctx, "not_found", &err.to_string())),
        };
        return run_summarize_target(ctx, config, target).await;
    }

    let Some(channel_arg) = args.channel.as_deref() else {
        return Ok(fail(
            ctx,
            "invalid_args",
            "provide a <CHANNEL> (optionally with --thread <TS>), --transcript <FILE_ID>, or --catchup",
        ));
    };

    let Some(archive) = archive else {
        return Ok(fail(
            ctx,
            "not_found",
            &format!("no local archive for workspace {workspace}; cannot summarize {channel_arg}"),
        ));
    };

    let resolved = archive
        .resolve_channel(workspace, channel_arg)
        .with_context(|| format!("resolving channel {channel_arg}"))?;
    let channel_id = resolved
        .as_ref()
        .map_or_else(|| raw_channel_id(channel_arg), |row| row.id.clone());

    let target = if let Some(thread_ts_arg) = args.thread.as_deref() {
        let thread_ts = Ts::from(thread_ts_arg.to_owned());
        match load_thread_target(archive.store(), workspace, &channel_id, &thread_ts) {
            Ok(t) => t,
            Err(err) => return Ok(fail(ctx, "empty_input", &err.to_string())),
        }
    } else {
        match load_channel_target(archive.store(), workspace, &channel_id, args.limit) {
            Ok(t) => t,
            Err(err) => return Ok(fail(ctx, "empty_input", &err.to_string())),
        }
    };

    run_summarize_target(ctx, config, target).await
}

async fn run_summarize_target(
    ctx: &Ctx,
    config: &AiSummaryConfig,
    target: SummaryTarget,
) -> Result<ExitCode> {
    match summarize(config, &target).await {
        Ok(res) => {
            let mut human = format!(
                "{} [{} · {}]\n\n{}\n",
                res.title,
                res.provider.short_label(),
                res.model,
                res.summary
            );
            if res.truncated {
                human.push_str("\n(input truncated to fit model context budget)\n");
            }
            output::emit(ctx.json, &res, &human);
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(fail(ctx, "summarize_failed", &err.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;
    use slackless_core::store::Store;
    use slackless_core::types::{ChannelId, Conversation, Message, TranscriptCue, User};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::cli::{Cli, Commands};

    fn seeded_archive() -> (TeamId, Archive) {
        let ws = TeamId::from("T1");
        let mut store = Store::open_in_memory().expect("in-memory store");
        store
            .upsert_conversations(
                &ws,
                &[Conversation {
                    id: ChannelId::from("C1"),
                    name: Some("engineering".to_owned()),
                    is_channel: true,
                    ..Conversation::default()
                }],
            )
            .expect("upsert conv");
        store
            .upsert_users(
                &ws,
                &[User {
                    id: slackless_core::types::UserId::from("U1"),
                    name: "alice".to_owned(),
                    real_name: Some("Alice".to_owned()),
                    ..User::default()
                }],
            )
            .expect("upsert user");
        store
            .upsert_messages(
                &ws,
                &ChannelId::from("C1"),
                &[
                    Message {
                        ts: Ts::from("1750000001.000100"),
                        user: Some(slackless_core::types::UserId::from("U1")),
                        text: "Root: should we enable local AI summaries?".to_owned(),
                        thread_ts: Some(Ts::from("1750000001.000100")),
                        reply_count: Some(1),
                        ..Message::default()
                    },
                    Message {
                        ts: Ts::from("1750000002.000100"),
                        user: Some(slackless_core::types::UserId::from("U1")),
                        text: "Decision: yes, ship with Ollama and OpenAI presets.".to_owned(),
                        thread_ts: Some(Ts::from("1750000001.000100")),
                        ..Message::default()
                    },
                ],
            )
            .expect("upsert msgs");
        store
            .upsert_file_transcript_with_cues(
                &ws,
                &FileId::from("F99"),
                "local",
                "Intro and launch checklist.",
                &[
                    TranscriptCue {
                        start_secs: 0.0,
                        end_secs: 4.0,
                        text: "Intro.".to_owned(),
                    },
                    TranscriptCue {
                        start_secs: 65.0,
                        end_secs: 70.0,
                        text: "Launch checklist approved.".to_owned(),
                    },
                ],
            )
            .expect("upsert transcript");
        (ws, Archive::from_store(store))
    }

    #[test]
    fn cli_parses_summarize_variants() {
        let cli = Cli::try_parse_from([
            "slackless",
            "summarize",
            "#engineering",
            "--thread",
            "1750000001.000100",
            "--endpoint",
            "http://localhost:11434/v1",
            "--model",
            "llama3.2",
            "--json",
        ])
        .expect("parse thread");
        assert!(cli.json);
        match cli.command {
            Commands::Summarize(args) => {
                assert_eq!(args.channel.as_deref(), Some("#engineering"));
                assert_eq!(args.thread.as_deref(), Some("1750000001.000100"));
                assert_eq!(args.model.as_deref(), Some("llama3.2"));
            }
            _ => panic!("expected Summarize command"),
        }

        let cli_tr = Cli::try_parse_from([
            "slackless",
            "summarize",
            "--transcript",
            "F99",
            "--api-key-env",
            "MY_LLM_KEY",
        ])
        .expect("parse transcript");
        match cli_tr.command {
            Commands::Summarize(args) => {
                assert_eq!(args.transcript.as_deref(), Some("F99"));
                assert_eq!(args.api_key_env.as_deref(), Some("MY_LLM_KEY"));
            }
            _ => panic!("expected Summarize command"),
        }

        let cli_cu = Cli::try_parse_from(["slackless", "summarize", "--catchup", "--since", "24h"])
            .expect("parse catchup");
        match cli_cu.command {
            Commands::Summarize(args) => {
                assert!(args.catchup);
                assert_eq!(args.since.as_deref(), Some("24h"));
            }
            _ => panic!("expected Summarize command"),
        }
    }

    #[test]
    fn resolve_ai_config_rejects_bad_url_and_missing_env_var() {
        let bad_url = SummarizeArgs {
            channel: Some("#eng".to_owned()),
            thread: None,
            transcript: None,
            catchup: false,
            since: None,
            all_workspaces: false,
            limit: 50,
            provider: Some(crate::cli::AiProviderArg::Custom),
            endpoint: Some("file:///etc/passwd".to_owned()),
            model: Some("m".to_owned()),
            api_key_env: None,
            timeout: 30,
        };
        assert!(resolve_ai_config(&bad_url).is_err());

        let missing_env = SummarizeArgs {
            channel: Some("#eng".to_owned()),
            thread: None,
            transcript: None,
            catchup: false,
            since: None,
            all_workspaces: false,
            limit: 50,
            provider: Some(crate::cli::AiProviderArg::Ollama),
            endpoint: None,
            model: Some("llama3.2".to_owned()),
            api_key_env: Some("SLACKLESS_DEFINITELY_UNSET_KEY_987654".to_owned()),
            timeout: 30,
        };
        let err = resolve_ai_config(&missing_env).expect_err("unset env must error");
        assert!(err.contains("SLACKLESS_DEFINITELY_UNSET_KEY_987654"));
    }

    #[test]
    fn provider_inference_rejects_openai_lookalike_hosts_and_does_not_auto_attach_key() {
        assert_eq!(
            infer_provider_from_endpoint(Some("https://api.openai.com/v1")),
            AiProvider::OpenAi
        );
        assert_eq!(
            infer_provider_from_endpoint(Some("http://api.openai.com/v1")),
            AiProvider::Custom
        );
        assert_eq!(
            infer_provider_from_endpoint(Some("https://evil.com/api.openai.com/v1")),
            AiProvider::Custom
        );
        assert_eq!(
            infer_provider_from_endpoint(Some("https://api.openai.com.evil.com/v1")),
            AiProvider::Custom
        );
        assert_eq!(
            infer_provider_from_endpoint(Some("http://localhost:11434/v1")),
            AiProvider::Ollama
        );
        assert_eq!(
            infer_provider_from_endpoint(Some("http://127.0.0.1:1234/v1")),
            AiProvider::LmStudio
        );
        assert_eq!(
            infer_provider_from_endpoint(Some("https://evil.com/11434/v1")),
            AiProvider::Custom
        );

        let lookalike = SummarizeArgs {
            channel: Some("#eng".to_owned()),
            thread: None,
            transcript: None,
            catchup: false,
            since: None,
            all_workspaces: false,
            limit: 50,
            provider: None,
            endpoint: Some("https://evil.com/api.openai.com/v1".to_owned()),
            model: Some("custom-model".to_owned()),
            api_key_env: None,
            timeout: 30,
        };
        let cfg = resolve_ai_config(&lookalike).expect("custom endpoint resolves");
        assert_eq!(cfg.provider, AiProvider::Custom);
        assert!(
            cfg.api_key.is_none(),
            "lookalike URL must never auto-attach OPENAI_API_KEY or Keychain key"
        );
    }

    #[tokio::test]
    async fn summarize_in_archive_hits_wiremock_for_thread_channel_and_transcript() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer test-cli-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "test-llm",
                "choices": [{
                    "message": { "role": "assistant", "content": "Decisions: ship local AI summaries." }
                }]
            })))
            .expect(3)
            .mount(&server)
            .await;

        let (ws, archive) = seeded_archive();
        let cfg = AiSummaryConfig {
            provider: AiProvider::Custom,
            base_url: format!("{}/v1", server.uri()),
            model: "test-llm".to_owned(),
            api_key: Some("test-cli-token".to_owned()),
            ..AiSummaryConfig::default()
        };
        let ctx = Ctx { json: true };

        // 1. Thread
        let thread_args = SummarizeArgs {
            channel: Some("#engineering".to_owned()),
            thread: Some("1750000001.000100".to_owned()),
            transcript: None,
            catchup: false,
            since: None,
            all_workspaces: false,
            limit: 50,
            provider: None,
            endpoint: None,
            model: None,
            api_key_env: None,
            timeout: 30,
        };
        assert_eq!(
            cmd_summarize_in_archive(&ctx, &ws, Some(&archive), &cfg, &thread_args)
                .await
                .expect("thread"),
            ExitCode::SUCCESS
        );

        // 2. Channel
        let ch_args = SummarizeArgs {
            thread: None,
            ..thread_args
        };
        assert_eq!(
            cmd_summarize_in_archive(&ctx, &ws, Some(&archive), &cfg, &ch_args)
                .await
                .expect("channel"),
            ExitCode::SUCCESS
        );

        // 3. Transcript
        let tr_args = SummarizeArgs {
            channel: None,
            transcript: Some("F99".to_owned()),
            ..ch_args
        };
        assert_eq!(
            cmd_summarize_in_archive(&ctx, &ws, Some(&archive), &cfg, &tr_args)
                .await
                .expect("transcript"),
            ExitCode::SUCCESS
        );
    }

    #[tokio::test]
    async fn summarize_catchup_briefing_hits_wiremock() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "llama3.2",
                "choices": [{
                    "message": { "role": "assistant", "content": "Needs Your Attention: check #engineering." }
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let (ws, archive) = seeded_archive();
        let briefing = build_catchup_for_archives(
            &[(&ws, None, &archive)],
            slackless_core::catchup::CatchUpWindow::Custom { since_secs: 1 },
            1_760_000_000,
            25,
        )
        .expect("catchup briefing");
        assert!(!briefing.is_empty());

        let cfg = AiSummaryConfig {
            provider: AiProvider::Ollama,
            base_url: format!("{}/v1", server.uri()),
            model: "llama3.2".to_owned(),
            api_key: None,
            ..AiSummaryConfig::default()
        };
        let ctx = Ctx { json: true };
        assert_eq!(
            run_summarize_target(&ctx, &cfg, SummaryTarget::CatchUp { briefing })
                .await
                .expect("catchup summary"),
            ExitCode::SUCCESS
        );
    }
}
