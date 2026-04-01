//! Web handler 模块，负责处理具体 HTTP 请求并把结果渲染为响应。

use salvo::{
    Depot, Request, Response,
    http::StatusCode,
    prelude::{Json, Text, handler},
};
use serde_json::{Map, Value};
use tracing::debug;

use crate::{
    capability::{
        ConversationRequest, DirectToolInvocationRequest, MediaTranslateAudioOutput,
        MediaTranslateInput, MediaTranslateRequest as CapabilityMediaTranslateRequest,
        SpeechSynthesisRequest as CapabilitySpeechSynthesisRequest, StructuredExtractionRequest,
    },
    channel::InboundMessageParseOutcome,
    channel::feishu::{
        FeishuCallbackErrorKind, callback_ack, extract_event_type, handle_audio_message_event,
        handle_text_message_event, parse_message_event,
        process_callback as process_feishu_callback,
    },
    forms::validate_form_data,
    logging,
};

use super::{
    pages::{weixin_connect_disabled_page_html, weixin_connect_page_html},
    state::app_state,
    types::{
        ChatRequest, FormExtractRequest, FormExtractResponse, FormInvalidFieldResponse,
        HealthResponse, MediaTranslateRequest, MediaTranslateResponse, SpeechSynthesisRequest,
        SpeechSynthesisResponse, ToolInvokeRequest, ToolInvokeResponse,
        WeixinLoginStartRequest, WeixinLoginStartResponse, WeixinLoginWaitRequest,
        WeixinLoginWaitResponse,
    },
    util::{merge_action_into_args, render_error},
};

/// 处理健康检查请求。
#[handler]
pub(crate) async fn health(depot: &mut Depot, res: &mut Response) {
    let state = app_state(depot);
    res.render(Json(HealthResponse {
        status: "ok",
        app_name: state.config.app_name.clone(),
        provider: state.config.llm.provider.as_str().to_string(),
        model: state.config.llm.model.clone(),
    }));
}

/// 返回微信扫码接入页面。
#[handler]
pub(crate) async fn weixin_connect_page(depot: &mut Depot, res: &mut Response) {
    let state = app_state(depot);
    let html = if state.weixin_manager.is_enabled() {
        weixin_connect_page_html()
    } else {
        weixin_connect_disabled_page_html()
    };
    res.render(Text::Html(html));
}

/// 处理浏览器的 CORS 预检请求。
#[handler]
pub(crate) async fn cors_preflight(res: &mut Response) {
    res.status_code(StatusCode::NO_CONTENT);
}

/// 开始微信二维码登录。
#[handler]
pub(crate) async fn weixin_login_start(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let body = match req.parse_json::<WeixinLoginStartRequest>().await {
        Ok(value) => value,
        Err(error) => {
            render_error(
                res,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &error.to_string(),
            );
            return;
        }
    };
    let state = app_state(depot);
    match state
        .weixin_manager
        .start_login(body.force, body.account_id)
        .await
    {
        Ok(result) => res.render(Json(WeixinLoginStartResponse {
            ok: true,
            session_key: result.session_key,
            qr_code_url: result.qr_code_url,
            qr_code_data_url: result.qr_code_data_url,
            message: result.message,
        })),
        Err(error) => render_error(
            res,
            StatusCode::INTERNAL_SERVER_ERROR,
            "weixin_login_start_failed",
            &error.to_string(),
        ),
    }
}

/// 等待微信二维码登录结果。
#[handler]
pub(crate) async fn weixin_login_wait(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let body = match req.parse_json::<WeixinLoginWaitRequest>().await {
        Ok(value) => value,
        Err(error) => {
            render_error(
                res,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &error.to_string(),
            );
            return;
        }
    };
    let state = app_state(depot);
    match state
        .weixin_manager
        .wait_login(&body.session_key, body.timeout_ms)
        .await
    {
        Ok(result) => res.render(Json(WeixinLoginWaitResponse {
            ok: true,
            connected: result.connected,
            account_id: result.account_id,
            linked_user_id: result.linked_user_id,
            message: result.message,
        })),
        Err(error) => render_error(
            res,
            StatusCode::INTERNAL_SERVER_ERROR,
            "weixin_login_wait_failed",
            &error.to_string(),
        ),
    }
}

/// 列出当前微信账号。
#[handler]
pub(crate) async fn list_weixin_accounts(depot: &mut Depot, res: &mut Response) {
    let state = app_state(depot);
    match state.weixin_manager.list_accounts().await {
        Ok(accounts) => res.render(Json(accounts)),
        Err(error) => render_error(
            res,
            StatusCode::INTERNAL_SERVER_ERROR,
            "weixin_accounts_failed",
            &error.to_string(),
        ),
    }
}

/// 重启指定微信账号的长轮询任务。
#[handler]
pub(crate) async fn weixin_restart_account(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) {
    let Some(account_id) = req.param::<String>("account_id") else {
        render_error(
            res,
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "missing account_id",
        );
        return;
    };
    let state = app_state(depot);
    match state.weixin_manager.restart_account(&account_id).await {
        Ok(()) => res.render(Json(serde_json::json!({ "ok": true, "account_id": account_id }))),
        Err(error) => render_error(
            res,
            StatusCode::INTERNAL_SERVER_ERROR,
            "weixin_restart_failed",
            &error.to_string(),
        ),
    }
}

/// 处理飞书回调请求，并在识别到文本消息时转交给飞书通道服务。
#[handler]
pub(crate) async fn feishu_callback(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let method = req.method().as_str().to_string();
    let uri = req.uri().to_string();
    let user_agent = req
        .headers()
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<missing>")
        .to_string();
    let content_type = req
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<missing>")
        .to_string();
    let request_id = req
        .headers()
        .get("x-request-id")
        .or_else(|| req.headers().get("x-tt-logid"))
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<missing>")
        .to_string();

    let payload = match req.payload().await {
        Ok(bytes) => bytes.clone(),
        Err(error) => {
            logging::log_channel_callback_body_read_error(
                "feishu",
                &method,
                &uri,
                &user_agent,
                &content_type,
                &request_id,
                &error.to_string(),
            );
            render_error(
                res,
                StatusCode::BAD_REQUEST,
                "invalid_request_body",
                &error.to_string(),
            );
            return;
        }
    };
    let raw_body_preview = logging::preview_bytes(&payload, 320);
    logging::log_channel_callback_ingress(
        "feishu",
        &method,
        &uri,
        &user_agent,
        &content_type,
        &request_id,
        &payload,
    );
    let body = if payload.is_empty() {
        Value::Object(Map::new())
    } else {
        match serde_json::from_slice::<Value>(&payload) {
            Ok(value) => value,
            Err(error) => {
                logging::log_channel_callback_json_error(
                    "feishu",
                    &method,
                    &uri,
                    &request_id,
                    &raw_body_preview,
                    &error.to_string(),
                );
                render_error(
                    res,
                    StatusCode::BAD_REQUEST,
                    "invalid_json",
                    &error.to_string(),
                );
                return;
            }
        }
    };

    let state = app_state(depot);
    match process_feishu_callback(body, &state.config.feishu_callback) {
        Ok(outcome) => {
            let event_type = extract_event_type(&outcome.payload).map(str::to_string);
            logging::log_channel_callback_processed(
                "feishu",
                outcome.encrypted,
                event_type.as_deref(),
                &outcome.payload,
            );
            match parse_message_event(&outcome.payload, &state.config.feishu_callback) {
                Ok(InboundMessageParseOutcome::Text(event)) => {
                    logging::log_channel_text_message_received(
                        event.channel.as_str(),
                        event.event_id.as_deref(),
                        &event.message_id,
                        event.chat_id.as_deref(),
                        event.chat_type.as_deref(),
                        &event.session_id,
                        &event.user_id,
                        &event.text,
                    );
                    let background_state = state.clone();
                    let event_channel = event.channel;
                    tokio::spawn(async move {
                        if let Err(error) = handle_text_message_event(
                            background_state.capabilities.conversation().clone(),
                            background_state.capabilities.english_learning().clone(),
                            background_state.config.feishu_callback.clone(),
                            event,
                        )
                        .await
                        {
                            logging::log_channel_background_error(
                                event_channel.as_str(),
                                &error.to_string(),
                            );
                        }
                    });
                    res.render(Json(callback_ack()));
                }
                Ok(InboundMessageParseOutcome::Audio(event)) => {
                    logging::log_channel_audio_message_received(
                        event.channel.as_str(),
                        event.event_id.as_deref(),
                        &event.message_id,
                        event.chat_id.as_deref(),
                        event.chat_type.as_deref(),
                        &event.session_id,
                        &event.user_id,
                        &event.file_key,
                        event.duration_ms,
                    );
                    let background_state = state.clone();
                    let event_channel = event.channel;
                    tokio::spawn(async move {
                        if let Err(error) = handle_audio_message_event(
                            background_state.capabilities.conversation().clone(),
                            background_state.capabilities.english_learning().clone(),
                            background_state.capabilities.media_translate().clone(),
                            background_state.capabilities.speech_synthesis().clone(),
                            background_state.config.feishu_callback.clone(),
                            event,
                        )
                        .await
                        {
                            logging::log_channel_background_error(
                                event_channel.as_str(),
                                &error.to_string(),
                            );
                        }
                    });
                    res.render(Json(callback_ack()));
                }
                Ok(InboundMessageParseOutcome::Ignored { reason }) => {
                    logging::log_channel_message_ignored("feishu", reason);
                    res.render(Json(callback_ack()));
                }
                Ok(InboundMessageParseOutcome::NotMessageEvent) => {
                    res.render(Json(outcome.response_body));
                }
                Err(error) => {
                    logging::log_channel_message_parse_error("feishu", &error.to_string());
                    res.render(Json(callback_ack()));
                }
            }
        }
        Err(error) => {
            let status = match error.kind {
                FeishuCallbackErrorKind::BadRequest => StatusCode::BAD_REQUEST,
                FeishuCallbackErrorKind::Unauthorized => StatusCode::UNAUTHORIZED,
            };
            let error_type = match error.kind {
                FeishuCallbackErrorKind::BadRequest => "feishu_callback_invalid",
                FeishuCallbackErrorKind::Unauthorized => "feishu_callback_unauthorized",
            };
            logging::log_channel_callback_process_error("feishu", status.as_u16(), &error.message);
            render_error(res, status, error_type, &error.message);
        }
    }
}

/// 返回当前已注册工具列表。
#[handler]
pub(crate) async fn list_tools(depot: &mut Depot, res: &mut Response) {
    let state = app_state(depot);
    let tools = state.capabilities.tools().list_descriptors();
    debug!(tool_count = tools.len(), "listing tools");
    res.render(Json(tools));
}

/// 返回当前内存中的会话摘要列表。
#[handler]
pub(crate) async fn list_sessions(depot: &mut Depot, res: &mut Response) {
    let state = app_state(depot);
    let sessions = state.capabilities.sessions().list().await;
    debug!(session_count = sessions.len(), "listing sessions");
    res.render(Json(sessions));
}

/// 返回指定会话的历史消息。
#[handler]
pub(crate) async fn session_history(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = app_state(depot);
    let session_id = match req.param::<String>("session_id") {
        Some(value) => value,
        None => {
            render_error(
                res,
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "missing session_id",
            );
            return;
        }
    };
    let limit = req.query::<usize>("limit");
    let history = state
        .capabilities
        .sessions()
        .history(&session_id, limit)
        .await;
    debug!(
        session_id = %session_id,
        limit = ?limit,
        message_count = history.len(),
        "loaded session history"
    );
    res.render(Json(history));
}

/// 处理标准聊天请求，并驱动一次引擎回合。
#[handler]
pub(crate) async fn chat(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let body = match req.parse_json::<ChatRequest>().await {
        Ok(value) => value,
        Err(error) => {
            render_error(
                res,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &error.to_string(),
            );
            return;
        }
    };
    let state = app_state(depot);
    let session_id = body.session_id.clone();
    let user_id = body.user_id.clone();
    logging::log_http_chat_request(
        &session_id,
        &user_id,
        body.persist,
        body.max_iterations,
        &body.message,
    );

    match state
        .capabilities
        .conversation()
        .execute(ConversationRequest {
            session_id: body.session_id,
            user_id: body.user_id,
            message: body.message,
            system_prompt: body.system_prompt,
            max_iterations: body.max_iterations,
            persist: body.persist,
        })
        .await
    {
        Ok(response) => {
            logging::log_http_chat_complete(
                &response.session_id,
                &response.user_id,
                response.iterations,
                response.tool_calls.len(),
                response.finish_reason.as_deref(),
                &response.answer,
            );
            res.render(Json(response));
        }
        Err(error) => {
            logging::log_http_chat_failed(&session_id, &user_id, &error.to_string());
            render_error(
                res,
                StatusCode::INTERNAL_SERVER_ERROR,
                "tool_loop_failed",
                &error.to_string(),
            )
        }
    }
}

/// 处理单轮结构化表单抽取请求。
#[handler]
pub(crate) async fn extract_form(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let body = match req.parse_json::<FormExtractRequest>().await {
        Ok(value) => value,
        Err(error) => {
            render_error(
                res,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &error.to_string(),
            );
            return;
        }
    };

    logging::log_http_form_extract_request(
        body.form_id.as_deref(),
        &body.text,
        body.schema.is_some(),
    );

    let state = app_state(depot);
    let mut warnings = Vec::new();
    let resolved_form = if let Some(form_id) = body.form_id.as_deref() {
        match state.form_store.load(form_id) {
            Ok(form) => {
                debug!(
                    form_id = %form.form_id,
                    source_path = %form.source_path.display(),
                    "loaded local markdown form definition"
                );
                if body.schema.is_some() {
                    warnings.push(
                        "inline schema was ignored because form_id resolved to a local markdown form"
                            .to_string(),
                    );
                }
                Some(form)
            }
            Err(error) if form_load_not_found(&error) => {
                logging::log_http_form_extract_failed(body.form_id.as_deref(), &error.to_string());
                render_error(
                    res,
                    StatusCode::NOT_FOUND,
                    "form_not_found",
                    &error.to_string(),
                );
                return;
            }
            Err(error) => {
                logging::log_http_form_extract_failed(body.form_id.as_deref(), &error.to_string());
                render_error(
                    res,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "form_definition_invalid",
                    &error.to_string(),
                );
                return;
            }
        }
    } else {
        None
    };

    let (form_id, form_title, schema_source, schema, instructions) =
        if let Some(form) = resolved_form {
            (
                Some(form.form_id),
                form.title,
                "markdown",
                form.schema,
                merge_instructions(form.instructions, body.instructions),
            )
        } else if let Some(schema) = body.schema {
            (
                None,
                None,
                "inline",
                schema,
                body.instructions.filter(|value| !value.trim().is_empty()),
            )
        } else {
            render_error(
                res,
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "either form_id or schema must be provided",
            );
            return;
        };

    match state
        .capabilities
        .extraction()
        .execute(StructuredExtractionRequest {
            schema: schema.clone(),
            input_text: body.text.clone(),
            schema_name: form_title.clone().or_else(|| form_id.clone()),
            instructions,
        })
        .await
    {
        Ok(output) => {
            let mut report = validate_form_data(&schema, &output.data);
            warnings.append(&mut report.warnings);
            logging::log_http_form_extract_complete(
                form_id.as_deref(),
                report.missing_fields.len(),
                report.invalid_fields.len(),
                warnings.len(),
                &output.data,
            );
            res.render(Json(FormExtractResponse {
                ok: true,
                form_id,
                form_title,
                schema_source,
                raw_text: body.text,
                data: output.data,
                missing_fields: report.missing_fields,
                invalid_fields: report
                    .invalid_fields
                    .into_iter()
                    .map(|issue| FormInvalidFieldResponse {
                        field: issue.field,
                        message: issue.message,
                    })
                    .collect(),
                warnings,
            }));
        }
        Err(error) => {
            logging::log_http_form_extract_failed(body.form_id.as_deref(), &error.to_string());
            render_error(
                res,
                StatusCode::INTERNAL_SERVER_ERROR,
                "form_extraction_failed",
                &error.to_string(),
            );
        }
    }
}

/// 处理媒体翻译请求，独立调用阿里百炼媒体翻译接口。
#[handler]
pub(crate) async fn translate_media(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let body = match req.parse_json::<MediaTranslateRequest>().await {
        Ok(value) => value,
        Err(error) => {
            render_error(
                res,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &error.to_string(),
            );
            return;
        }
    };

    let input = match (body.audio, body.video_url) {
        (Some(audio), None) => {
            logging::log_http_media_translate_request(
                "audio",
                body.source_lang.as_deref(),
                &body.target_lang,
                body.output_audio.is_some(),
            );
            MediaTranslateInput::Audio {
                data: audio.data,
                format: audio.format,
            }
        }
        (None, Some(video_url)) => {
            logging::log_http_media_translate_request(
                "video",
                body.source_lang.as_deref(),
                &body.target_lang,
                body.output_audio.is_some(),
            );
            MediaTranslateInput::VideoUrl { url: video_url }
        }
        (Some(_), Some(_)) => {
            render_error(
                res,
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "provide either audio or video_url, not both",
            );
            return;
        }
        (None, None) => {
            render_error(
                res,
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "either audio or video_url must be provided",
            );
            return;
        }
    };

    let state = app_state(depot);
    match state
        .capabilities
        .media_translate()
        .execute(CapabilityMediaTranslateRequest {
            source_lang: body.source_lang.clone(),
            target_lang: body.target_lang.clone(),
            input,
            output_audio: body.output_audio.map(|audio| MediaTranslateAudioOutput {
                format: audio.format,
                voice: audio.voice,
            }),
            include_usage: body.include_usage,
        })
        .await
    {
        Ok(output) => {
            logging::log_http_media_translate_complete(
                &output.model,
                &output.translated_text,
                output.audio_base64.is_some(),
                output.finish_reason.as_deref(),
            );
            res.render(Json(MediaTranslateResponse {
                ok: true,
                model: output.model,
                request_id: output.request_id,
                finish_reason: output.finish_reason,
                source_lang: body.source_lang,
                target_lang: body.target_lang,
                translated_text: output.translated_text,
                audio_base64: output.audio_base64,
                audio_id: output.audio_id,
                usage: output.usage,
            }));
        }
        Err(error) => {
            logging::log_http_media_translate_failed(&error.to_string());
            render_error(
                res,
                StatusCode::INTERNAL_SERVER_ERROR,
                "media_translate_failed",
                &error.to_string(),
            );
        }
    }
}

/// 处理文本转语音请求，独立调用 SiliconFlow 语音合成接口。
#[handler]
pub(crate) async fn synthesize_speech(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let body = match req.parse_json::<SpeechSynthesisRequest>().await {
        Ok(value) => value,
        Err(error) => {
            render_error(
                res,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &error.to_string(),
            );
            return;
        }
    };

    logging::log_http_speech_synthesis_request(
        body.model.as_deref(),
        body.voice.as_deref(),
        body.response_format.as_deref(),
        &body.text,
    );

    let state = app_state(depot);
    match state
        .capabilities
        .speech_synthesis()
        .execute(CapabilitySpeechSynthesisRequest {
            text: body.text,
            model: body.model,
            voice: body.voice,
            response_format: body.response_format,
            sample_rate: body.sample_rate,
            speed: body.speed,
            gain: body.gain,
            stream: body.stream,
        })
        .await
    {
        Ok(output) => {
            logging::log_http_speech_synthesis_complete(
                &output.model,
                &output.voice,
                &output.response_format,
                output.byte_len,
                output.trace_id.as_deref(),
            );
            res.render(Json(SpeechSynthesisResponse {
                ok: true,
                model: output.model,
                voice: output.voice,
                response_format: output.response_format,
                content_type: output.content_type,
                audio_base64: output.audio_base64,
                byte_len: output.byte_len,
                trace_id: output.trace_id,
            }));
        }
        Err(error) => {
            logging::log_http_speech_synthesis_failed(&error.to_string());
            render_error(
                res,
                StatusCode::INTERNAL_SERVER_ERROR,
                "speech_synthesis_failed",
                &error.to_string(),
            );
        }
    }
}

/// 处理直接工具调用请求。
#[handler]
pub(crate) async fn invoke_tool(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let body = match req.parse_json::<ToolInvokeRequest>().await {
        Ok(value) => value,
        Err(error) => {
            render_error(
                res,
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &error.to_string(),
            );
            return;
        }
    };
    let state = app_state(depot);
    let args = merge_action_into_args(body.args, body.action);
    logging::log_http_tool_invoke_request(&body.session_id, &body.user_id, &body.tool, &args);

    match state
        .capabilities
        .tools()
        .invoke(DirectToolInvocationRequest {
            tool: body.tool,
            session_id: body.session_id,
            user_id: body.user_id,
            args,
        })
        .await
    {
        Ok(result) => {
            logging::log_http_tool_invoke_complete(
                &result.tool_name,
                &result.function_call_id,
                &result.output,
            );
            res.render(Json(ToolInvokeResponse {
                ok: true,
                result: result.output,
            }))
        }
        Err(error) => {
            logging::log_http_tool_invoke_failed(&error.to_string());
            render_error(
                res,
                StatusCode::BAD_REQUEST,
                "tool_execution_failed",
                &error.to_string(),
            )
        }
    }
}

/// 合并表单定义中的默认提取说明和本次请求追加说明。
fn merge_instructions(
    form_instructions: Option<String>,
    request_instructions: Option<String>,
) -> Option<String> {
    let form_instructions = form_instructions
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let request_instructions = request_instructions
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    match (form_instructions, request_instructions) {
        (Some(left), Some(right)) => Some(format!(
            "{left}\n\nAdditional request instructions:\n{right}"
        )),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

/// 判断表单加载错误是否来自文件不存在。
fn form_load_not_found(error: &anyhow::Error) -> bool {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())
        .map(|io_error| io_error.kind() == std::io::ErrorKind::NotFound)
        .unwrap_or(false)
}
