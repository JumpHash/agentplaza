//! Amazon Bedrock's Converse API (not streamed). Auth is a Bedrock API key
//! (bearer) when one is configured, else SigV4 from the `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`
//! (and optional `AWS_SESSION_TOKEN`) secrets, which come from the secret source, not the environment.

use std::time::SystemTime;

use apz_proto::chat::{Block, ChatRequest, ChatResponse, Delta, Message, Role, StopReason, Usage};
use serde_json::{Value, json};

use crate::http::Req;
use crate::sigv4::{self, Credentials, uri_encode};
use crate::{Ctx, GatewayError, ModelInfo, replayable, results_first};

const TAG: &str = "bedrock";
const DEFAULT_MAX_TOKENS: u32 = 8192;

fn region(ctx: &Ctx) -> Result<String, GatewayError> {
    ctx.p
        .options
        .as_ref()
        .and_then(|o| o.region.as_deref())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| ctx.env("AWS_REGION"))
        .or_else(|| ctx.env("AWS_DEFAULT_REGION"))
        .ok_or_else(|| {
            GatewayError::Config(format!(
                "provider `{}` needs options.region (or AWS_REGION)",
                ctx.p.id
            ))
        })
}

/// Adds `extra_headers`, then the credential: the provider's configured one
/// or `AWS_BEARER_TOKEN_BEDROCK` as a bearer token, else a SigV4 signature.
fn authorize(ctx: &Ctx, req: Req, region: &str, service: &str) -> Result<Req, GatewayError> {
    let mut req = req.extra_headers(ctx.p.extra_headers.iter());
    if let Some(token) = ctx
        .credential()?
        .or_else(|| ctx.env("AWS_BEARER_TOKEN_BEDROCK"))
    {
        return Ok(req.bearer(&token));
    }
    let creds = pair(ctx)?;
    sigv4::sign(&mut req, &creds, region, service, SystemTime::now());
    Ok(req)
}

/// The AWS key pair this provider signs with, read for this provider's id (vault binding `provider:<id>`):
/// both keys are required, the session token is optional. A vault refusal is passed on as it is; a
/// missing key is named, with where to set it.
fn pair(ctx: &Ctx) -> Result<Credentials, GatewayError> {
    let missing = |err: GatewayError| match err {
        GatewayError::Auth(what) => GatewayError::Auth(format!(
            "no AWS credentials: {what}; set AWS_BEARER_TOKEN_BEDROCK, or AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY in the vault bound to provider `{}`",
            ctx.p.id
        )),
        other => other,
    };
    let access_key = ctx.required_secret("AWS_ACCESS_KEY_ID").map_err(missing)?;
    let secret_key = ctx
        .required_secret("AWS_SECRET_ACCESS_KEY")
        .map_err(missing)?;
    let session_token = ctx.optional_secret("AWS_SESSION_TOKEN")?;
    Ok(Credentials {
        access_key,
        secret_key,
        session_token,
    })
}

pub(crate) fn complete(
    ctx: &Ctx,
    req: &ChatRequest,
    model: &str,
    sink: &mut dyn FnMut(Delta),
) -> Result<ChatResponse, GatewayError> {
    let region = region(ctx)?;
    let base = ctx.p.base().map_or_else(
        || format!("https://bedrock-runtime.{region}.amazonaws.com"),
        str::to_string,
    );
    let url = format!("{base}/model/{}/converse", uri_encode(model));
    let body = ctx.finish_body(body(req));
    let answer = ctx.http().json(&authorize(
        ctx,
        Req::post_json(url, &body),
        &region,
        "bedrock",
    )?)?;
    Ok(decode(&answer, model, sink))
}

fn body(req: &ChatRequest) -> Value {
    let max_tokens = if req.max_tokens == 0 {
        DEFAULT_MAX_TOKENS
    } else {
        req.max_tokens
    };
    let mut body =
        json!({"messages": messages(&req.messages), "inferenceConfig": {"maxTokens": max_tokens}});
    if let Some(t) = req.temperature {
        body["inferenceConfig"]["temperature"] = json!((f64::from(t) * 1000.0).round() / 1000.0);
    }
    if let Some(system) = req.system_text() {
        body["system"] = json!([{"text": system}]);
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| json!({"toolSpec": {"name": t.name, "description": t.description, "inputSchema": {"json": t.input_schema}}}))
            .collect();
        body["toolConfig"] = json!({"tools": tools});
    }
    body
}

fn messages(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|m| {
            let content: Vec<Value> = results_first(&m.content)
                .into_iter()
                .filter_map(|b| match b {
                    Block::Text { text } if text.is_empty() => None,
                    Block::Text { text } => Some(json!({"text": text})),
                    Block::ToolUse {
                        id, name, input, ..
                    } => Some(json!({"toolUse": {"toolUseId": id, "name": name, "input": input}})),
                    Block::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        // Converse rejects blank text.
                        let text = if content.trim().is_empty() {
                            "(no output)"
                        } else {
                            content
                        };
                        let mut result =
                            json!({"toolUseId": tool_use_id, "content": [{"text": text}]});
                        if *is_error {
                            result["status"] = json!("error");
                        }
                        Some(json!({"toolResult": result}))
                    }
                    other => replayable(other, TAG).cloned(),
                })
                .collect();
            let role = if m.role == Role::User {
                "user"
            } else {
                "assistant"
            };
            (!content.is_empty()).then(|| json!({"role": role, "content": content}))
        })
        .collect()
}

fn decode(v: &Value, model: &str, sink: &mut dyn FnMut(Delta)) -> ChatResponse {
    let mut content = Vec::new();
    for block in v["output"]["message"]["content"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if let Some(text) = block["text"].as_str() {
            sink(Delta::Text {
                text: text.to_string(),
            });
            content.push(Block::Text {
                text: text.to_string(),
            });
        } else if let Some(tool) = block.get("toolUse") {
            let name = tool["name"].as_str().unwrap_or("").to_string();
            let input = if tool["input"].is_object() {
                tool["input"].clone()
            } else {
                json!({})
            };
            let id = tool["toolUseId"].as_str().unwrap_or("").to_string();
            // A call comes whole in the answer: its start, its input and its end go out together.
            let index = content.len();
            sink(Delta::ToolUse {
                name: name.clone(),
                index,
                id: id.clone(),
            });
            sink(Delta::ToolInput {
                index,
                json: input.to_string(),
            });
            sink(Delta::BlockEnd { index });
            content.push(Block::ToolUse {
                id,
                name,
                input,
                meta: None,
            });
        } else {
            if let Some(text) = block["reasoningContent"]["reasoningText"]["text"]
                .as_str()
                .filter(|t| !t.is_empty())
            {
                sink(Delta::Thinking {
                    text: text.to_string(),
                });
                content.push(Block::Thinking {
                    text: text.to_string(),
                });
            }
            content.push(Block::Opaque {
                kind: TAG.into(),
                raw: block.clone(),
            });
        }
    }
    let stop = match v["stopReason"].as_str().unwrap_or("end_turn") {
        "end_turn" | "stop_sequence" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" | "model_context_window_exceeded" => StopReason::MaxTokens,
        "guardrail_intervened" | "content_filtered" => StopReason::Refusal,
        other => StopReason::Other(other.to_string()),
    };
    let u = &v["usage"];
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    let usage = Usage {
        input_tokens: n("inputTokens"),
        output_tokens: n("outputTokens"),
        cache_read_tokens: n("cacheReadInputTokens"),
        cache_write_tokens: n("cacheWriteInputTokens"),
    };
    ChatResponse {
        model: model.to_string(),
        content,
        stop,
        usage,
    }
}

/// `GET /foundation-models` on the control-plane host.
pub(crate) fn list_models(ctx: &Ctx) -> Result<Vec<ModelInfo>, GatewayError> {
    let region = region(ctx)?;
    let base = ctx.p.base().map_or_else(
        || format!("https://bedrock.{region}.amazonaws.com"),
        str::to_string,
    );
    let page = ctx.http().json(&authorize(
        ctx,
        Req::get(format!("{base}/foundation-models")),
        &region,
        "bedrock",
    )?)?;
    Ok(page["modelSummaries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(ModelInfo {
                id: m["modelId"].as_str()?.to_string(),
                name: m["modelName"].as_str().map(str::to_string),
                context: None,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Auth, ProviderConfig, ProviderKind};
    use crate::testutil::{FakeServer, Reply, gateway_with_env, provider, request};
    use crate::{Gateway, SecretRead, SecretReason, SecretSource};
    use apz_proto::chat::ToolSpec;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// Fake AWS values: no test asserts anything but that these reach the signer, or stay out of it.
    const AWS_KEY: &str = "AKIAFAKESENTINEL0001";
    const AWS_SECRET: &str = "SENTINEL-AWS-SECRET-7";
    const AWS_TOKEN: &str = "SENTINEL-AWS-TOKEN-7";

    /// A gateway whose secret source answers `secrets` as (provider id, name, value) and whose environment
    /// answers `env`. Also returns every environment name the gateway asked for.
    fn gateway_with_secrets(
        providers: Vec<ProviderConfig>,
        secrets: &[(&str, &str, &str)],
        env: &[(&str, &str)],
    ) -> (Gateway, Arc<Mutex<Vec<String>>>) {
        let secrets: Vec<(String, String, String)> = secrets
            .iter()
            .map(|(p, k, v)| (p.to_string(), k.to_string(), v.to_string()))
            .collect();
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let env_reads = Arc::new(Mutex::new(Vec::new()));
        let asked = Arc::clone(&env_reads);
        let gw = Gateway::builder()
            .providers(providers)
            .secrets(move |name: &str, read: &SecretRead<'_>| {
                secrets
                    .iter()
                    .find(|(p, k, _)| p == read.provider && k == name)
                    .map(|(_, _, v)| v.clone())
            })
            .env(move |name: &str| {
                asked.lock().unwrap().push(name.to_string());
                env.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
            })
            .retry(2, Duration::from_millis(5))
            .build();
        (gw, env_reads)
    }

    #[test]
    fn converse_with_sigv4_and_reasoning_replay() {
        let reasoning =
            json!({"reasoningContent": {"reasoningText": {"text": "think", "signature": "SIG"}}});
        let answer = json!({
            "output": {"message": {"role": "assistant", "content": [
                reasoning, {"text": "ok"},
                {"toolUse": {"toolUseId": "t1", "name": "bash", "input": {"command": "ls"}}},
            ]}},
            "stopReason": "tool_use",
            "usage": {"inputTokens": 11, "outputTokens": 7, "cacheReadInputTokens": 3},
        });
        let server = FakeServer::start(vec![
            Reply::json(200, &answer),
            Reply::json(
                200,
                &json!({"modelSummaries": [{"modelId": "amazon.nova-pro-v1:0", "modelName": "Nova Pro"}]}),
            ),
        ]);
        let mut p = provider("bedrock", ProviderKind::Bedrock, &server, Auth::None);
        p.options = Some(serde_json::from_value(json!({"region": "us-west-2"})).unwrap());
        p.models = vec!["us.anthropic.claude-sonnet-4-5-20250929-v1:0".into()];
        let pair = [
            ("bedrock", "AWS_ACCESS_KEY_ID", "AKIDTEST"),
            ("bedrock", "AWS_SECRET_ACCESS_KEY", "sekrit"),
            ("bedrock", "AWS_SESSION_TOKEN", "tok"),
        ];
        let (gw, _) = gateway_with_secrets(vec![p], &pair, &[]);
        let mut req = ChatRequest {
            system_parts: Vec::new(),
            cache_last_message: false,
            model: "bedrock/us.anthropic.claude-sonnet-4-5-20250929-v1:0".into(),
            system: Some("sys".into()),
            tools: vec![ToolSpec {
                deferred: false,
                name: "bash".into(),
                description: "d".into(),
                input_schema: json!({"type": "object"}),
            }],
            ..request("hi")
        };
        let resp = gw.complete(&req, &mut |_| {}).unwrap();
        assert_eq!(resp.stop, StopReason::ToolUse);
        assert_eq!(
            resp.usage,
            Usage {
                input_tokens: 11,
                output_tokens: 7,
                cache_read_tokens: 3,
                cache_write_tokens: 0
            }
        );
        assert_eq!(
            resp.content[..2],
            [
                Block::Thinking {
                    text: "think".into()
                },
                Block::Opaque {
                    kind: TAG.into(),
                    raw: reasoning.clone()
                }
            ]
        );

        let sent = &server.requests()[0];
        assert_eq!(
            sent.path,
            "/model/us.anthropic.claude-sonnet-4-5-20250929-v1%3A0/converse"
        );
        let auth = sent.header("authorization").unwrap();
        assert!(
            auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDTEST/"),
            "{auth}"
        );
        assert!(auth.contains("/us-west-2/bedrock/aws4_request, SignedHeaders=content-type;host;x-amz-date;x-amz-security-token, Signature="));
        assert_eq!(sent.header("x-amz-security-token"), Some("tok"));
        let b = sent.json();
        assert_eq!(b["system"], json!([{"text": "sys"}]));
        assert_eq!(
            b["inferenceConfig"],
            json!({"maxTokens": DEFAULT_MAX_TOKENS})
        );
        assert_eq!(
            b["toolConfig"]["tools"][0],
            json!({"toolSpec": {"name": "bash", "description": "d", "inputSchema": {"json": {"type": "object"}}}})
        );

        req.messages.push(Message::assistant(resp.content));
        req.messages.push(Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: "t1".into(),
                content: "".into(),
                is_error: true,
            }],
        });
        assert_eq!(
            messages(&req.messages)[1..],
            [
                json!({"role": "assistant", "content": [reasoning, {"text": "ok"}, {"toolUse": {"toolUseId": "t1", "name": "bash", "input": {"command": "ls"}}}]}),
                json!({"role": "user", "content": [{"toolResult": {"toolUseId": "t1", "content": [{"text": "(no output)"}], "status": "error"}}]}),
            ]
        );

        let models = gw.list_models("bedrock").unwrap();
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            [
                "amazon.nova-pro-v1:0",
                "us.anthropic.claude-sonnet-4-5-20250929-v1:0"
            ]
        );
        let sent = server.requests();
        assert_eq!(
            (sent[0].method.as_str(), sent[1].method.as_str()),
            ("POST", "GET")
        );
        assert_eq!(sent[1].path, "/foundation-models");
    }

    #[test]
    fn bedrock_api_key_is_a_bearer_token() {
        let server = FakeServer::start(vec![Reply::json(
            200,
            &json!({"output": {"message": {"content": [{"text": "hi"}]}}, "stopReason": "end_turn"}),
        )]);
        let gw = gateway_with_env(
            vec![provider("aws", ProviderKind::Bedrock, &server, Auth::None)],
            &[
                ("AWS_REGION", "eu-west-1"),
                ("AWS_BEARER_TOKEN_BEDROCK", "bedrock-key"),
            ],
        );
        let resp = gw
            .complete(
                &ChatRequest {
                    system_parts: Vec::new(),
                    cache_last_message: false,
                    model: "aws/amazon.nova-pro-v1:0".into(),
                    ..request("hi")
                },
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(
            (resp.text().as_str(), resp.model.as_str()),
            ("hi", "aws/amazon.nova-pro-v1:0")
        );
        assert_eq!(
            server.requests()[0].header("authorization"),
            Some("Bearer bedrock-key")
        );
    }

    #[test]
    fn sigv4_signs_with_the_pair_from_the_secret_source_and_not_the_environment() {
        let server = FakeServer::start(vec![Reply::json(
            200,
            &json!({"output": {"message": {"content": [{"text": "ok"}]}}, "stopReason": "end_turn"}),
        )]);
        let mut p = provider("bedrock", ProviderKind::Bedrock, &server, Auth::None);
        p.options = Some(serde_json::from_value(json!({"region": "eu-west-1"})).unwrap());
        let pair = [
            ("bedrock", "AWS_ACCESS_KEY_ID", AWS_KEY),
            ("bedrock", "AWS_SECRET_ACCESS_KEY", AWS_SECRET),
            ("bedrock", "AWS_SESSION_TOKEN", AWS_TOKEN),
        ];
        let (gw, env_reads) = gateway_with_secrets(vec![p], &pair, &[]);
        let resp = gw
            .complete(
                &ChatRequest {
                    system_parts: Vec::new(),
                    cache_last_message: false,
                    model: "bedrock/amazon.nova-pro-v1:0".into(),
                    ..request("hi")
                },
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(resp.text(), "ok");

        let sent = &server.requests()[0];
        let auth = sent.header("authorization").unwrap();
        assert!(
            auth.starts_with(&format!("AWS4-HMAC-SHA256 Credential={AWS_KEY}/")),
            "{auth}"
        );
        assert_eq!(sent.header("x-amz-security-token"), Some(AWS_TOKEN));
        // The pair is never asked of the environment; only the bearer-token name is, as before.
        let asked = env_reads.lock().unwrap();
        assert!(
            asked.iter().all(|n| !n.starts_with("AWS_ACCESS")
                && !n.starts_with("AWS_SECRET")
                && n != "AWS_SESSION_TOKEN"),
            "{asked:?}"
        );
    }

    #[test]
    fn a_missing_bedrock_pair_is_the_auth_error_for_its_own_provider() {
        // A missing key pair is the auth error, even when the environment holds one.
        {
            let server = FakeServer::start(vec![]);
            let mut p = provider("bedrock", ProviderKind::Bedrock, &server, Auth::None);
            p.options = Some(serde_json::from_value(json!({"region": "eu-west-1"})).unwrap());
            let (gw, _) = gateway_with_secrets(
                vec![p],
                &[("bedrock", "AWS_ACCESS_KEY_ID", AWS_KEY)],
                &[("AWS_SECRET_ACCESS_KEY", AWS_SECRET)],
            );
            match gw.complete(
                &ChatRequest {
                    system_parts: Vec::new(),
                    cache_last_message: false,
                    model: "bedrock/amazon.nova-pro-v1:0".into(),
                    ..request("hi")
                },
                &mut |_| {},
            ) {
                Err(GatewayError::Auth(message)) => {
                    assert!(message.contains("AWS_ACCESS_KEY_ID"), "{message}")
                }
                other => panic!("expected an auth error, got {:?}", other.map(|r| r.model)),
            }
            assert!(
                server.requests().is_empty(),
                "no request is sent without the pair"
            );
        }

        // A Bedrock provider does not borrow the key pair of another provider.
        {
            let server = FakeServer::start(vec![]);
            let mut other = provider("aws-b", ProviderKind::Bedrock, &server, Auth::None);
            other.options = Some(serde_json::from_value(json!({"region": "eu-west-1"})).unwrap());
            let (gw, _) = gateway_with_secrets(
                vec![other],
                &[
                    ("aws", "AWS_ACCESS_KEY_ID", AWS_KEY),
                    ("aws", "AWS_SECRET_ACCESS_KEY", AWS_SECRET),
                ],
                &[],
            );
            match gw.complete(
                &ChatRequest {
                    system_parts: Vec::new(),
                    cache_last_message: false,
                    model: "aws-b/amazon.nova-pro-v1:0".into(),
                    ..request("hi")
                },
                &mut |_| {},
            ) {
                Err(GatewayError::Auth(message)) => assert!(
                    message.contains("AWS_ACCESS_KEY_ID") && message.contains("`aws-b`"),
                    "{message}"
                ),
                other => panic!("expected an auth error, got {:?}", other.map(|r| r.model)),
            }
            assert!(server.requests().is_empty());
        }
    }

    fn converse_ok() -> Reply {
        Reply::json(
            200,
            &json!({"output": {"message": {"content": [{"text": "ok"}]}}, "stopReason": "end_turn"}),
        )
    }

    #[test]
    fn each_bedrock_provider_signs_with_the_pair_bound_to_its_own_id() {
        let server = FakeServer::start(vec![converse_ok(), converse_ok()]);
        let mut eu = provider("bedrock-eu", ProviderKind::Bedrock, &server, Auth::None);
        eu.options = Some(serde_json::from_value(json!({"region": "eu-west-1"})).unwrap());
        let mut us = provider("aws", ProviderKind::Bedrock, &server, Auth::None);
        us.options = Some(serde_json::from_value(json!({"region": "us-east-1"})).unwrap());
        let (gw, _) = gateway_with_secrets(
            vec![eu, us],
            &[
                ("bedrock-eu", "AWS_ACCESS_KEY_ID", "AKIAFAKEEU0001"),
                ("bedrock-eu", "AWS_SECRET_ACCESS_KEY", "SENTINEL-EU-SECRET"),
                ("aws", "AWS_ACCESS_KEY_ID", "AKIAFAKEUS0001"),
                ("aws", "AWS_SECRET_ACCESS_KEY", "SENTINEL-US-SECRET"),
            ],
            &[],
        );
        gw.complete(
            &ChatRequest {
                system_parts: Vec::new(),
                cache_last_message: false,
                model: "bedrock-eu/amazon.nova-pro-v1:0".into(),
                ..request("hi")
            },
            &mut |_| {},
        )
        .unwrap();
        gw.complete(
            &ChatRequest {
                system_parts: Vec::new(),
                cache_last_message: false,
                model: "aws/amazon.nova-pro-v1:0".into(),
                ..request("hi")
            },
            &mut |_| {},
        )
        .unwrap();
        let sent = server.requests();
        let (eu_auth, us_auth) = (
            sent[0].header("authorization").unwrap(),
            sent[1].header("authorization").unwrap(),
        );
        assert!(
            eu_auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIAFAKEEU0001/")
                && eu_auth.contains("/eu-west-1/"),
            "{eu_auth}"
        );
        assert!(
            us_auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIAFAKEUS0001/")
                && us_auth.contains("/us-east-1/"),
            "{us_auth}"
        );
    }

    /// A secret source that refuses every name, as a sealed vault does.
    struct Sealed;

    impl SecretSource for Sealed {
        fn get(&self, _name: &str, _read: &SecretRead<'_>) -> Option<String> {
            None
        }

        fn fetch(&self, _name: &str, _read: &SecretRead<'_>) -> Result<String, SecretReason> {
            Err(SecretReason::Sealed)
        }
    }

    #[test]
    fn a_refused_pair_fails_with_the_refusal_not_with_a_missing_key() {
        let server = FakeServer::start(vec![]);
        let mut p = provider("bedrock", ProviderKind::Bedrock, &server, Auth::None);
        p.options = Some(serde_json::from_value(json!({"region": "eu-west-1"})).unwrap());
        let gw = Gateway::builder()
            .providers(vec![p])
            .secrets(Sealed)
            .retry(0, Duration::from_millis(1))
            .build();
        match gw.complete(
            &ChatRequest {
                system_parts: Vec::new(),
                cache_last_message: false,
                model: "bedrock/amazon.nova-pro-v1:0".into(),
                ..request("hi")
            },
            &mut |_| {},
        ) {
            Err(GatewayError::Vault { status, message }) => {
                assert_eq!((status, message.as_str()), (503, "vault sealed"))
            }
            other => panic!(
                "expected the vault's refusal, got {:?}",
                other.map(|r| r.model)
            ),
        }
        assert!(server.requests().is_empty());
    }
}
