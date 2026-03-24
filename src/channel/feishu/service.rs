//! 飞书服务编排模块，负责把飞书消息事件接到引擎回合并将结果回复给飞书。

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

use crate::{
    capability::{
        ConversationCapability, ConversationRequest, EnglishLearningCapability,
        MediaTranslateCapability, MediaTranslateInput, MediaTranslateRequest,
    },
    channel::{InboundAudioMessage, InboundTextMessage, OutboundTextReply},
    config::FeishuCallbackConfig,
    logging,
};

use super::FeishuBotClient;

/// 处理一条统一入站文本消息，并通过飞书回复链路将结果回复回原消息。
pub async fn handle_text_message_event(
    conversation: ConversationCapability,
    english_learning: EnglishLearningCapability,
    config: FeishuCallbackConfig,
    event: InboundTextMessage,
) -> Result<()> {
    let answer = build_reply_text(
        &conversation,
        &english_learning,
        &event.session_id,
        &event.user_id,
        &event.text,
    )
    .await?;
    let reply = OutboundTextReply {
        channel: event.channel,
        reply_to_message_id: event.message_id.clone(),
        session_id: event.session_id.clone(),
        text: answer,
    };

    FeishuBotClient::new(config).send_text_reply(&reply).await?;

    logging::log_channel_text_replied(
        reply.channel.as_str(),
        &reply.reply_to_message_id,
        &reply.session_id,
        &reply.text,
    );
    Ok(())
}

/// 处理一条统一入站语音消息：先下载语音资源，再转写为文本，最后复用现有对话链路生成回复。
pub async fn handle_audio_message_event(
    conversation: ConversationCapability,
    english_learning: EnglishLearningCapability,
    media_translate: MediaTranslateCapability,
    config: FeishuCallbackConfig,
    event: InboundAudioMessage,
) -> Result<()> {
    let client = FeishuBotClient::new(config.clone());
    let audio = client
        .download_audio_resource(
            &event.message_id,
            &event.file_key,
            &event.resource_type,
            event.format_hint.as_deref(),
        )
        .await?;
    let audio_data_url = format!(
        "data:{};base64,{}",
        audio.mime_type,
        STANDARD.encode(&audio.bytes)
    );
    let transcript = media_translate
        .execute(MediaTranslateRequest {
            source_lang: config.audio_source_lang.clone(),
            target_lang: config.audio_target_lang.clone(),
            input: MediaTranslateInput::Audio {
                data: audio_data_url,
                format: audio.format,
            },
            output_audio: None,
            include_usage: true,
        })
        .await?
        .translated_text
        .trim()
        .to_string();

    if transcript.is_empty() {
        let reply = OutboundTextReply {
            channel: event.channel,
            reply_to_message_id: event.message_id.clone(),
            session_id: event.session_id.clone(),
            text: "我收到了语音消息，但这次没有成功识别出可用文本。".to_string(),
        };
        client.send_text_reply(&reply).await?;
        logging::log_channel_text_replied(
            reply.channel.as_str(),
            &reply.reply_to_message_id,
            &reply.session_id,
            &reply.text,
        );
        return Ok(());
    }
    logging::log_channel_audio_transcribed(
        event.channel.as_str(),
        &event.message_id,
        &event.session_id,
        &transcript,
    );
    let answer = build_reply_text(
        &conversation,
        &english_learning,
        &event.session_id,
        &event.user_id,
        &transcript,
    )
    .await?;
    let reply = OutboundTextReply {
        channel: event.channel,
        reply_to_message_id: event.message_id.clone(),
        session_id: event.session_id.clone(),
        text: answer,
    };

    client.send_text_reply(&reply).await?;
    logging::log_channel_text_replied(
        reply.channel.as_str(),
        &reply.reply_to_message_id,
        &reply.session_id,
        &reply.text,
    );
    Ok(())
}

/// 返回飞书事件接收成功时的标准 ACK 响应。
pub fn callback_ack() -> Value {
    json!({
        "code": 0,
        "msg": "ok"
    })
}

async fn build_reply_text(
    conversation: &ConversationCapability,
    english_learning: &EnglishLearningCapability,
    session_id: &str,
    user_id: &str,
    message: &str,
) -> Result<String> {
    if let Some(reply) = english_learning
        .maybe_handle_message(session_id, message)
        .await?
    {
        let trimmed = reply.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    let response = conversation
        .execute(ConversationRequest {
            session_id: session_id.to_string(),
            user_id: user_id.to_string(),
            message: message.to_string(),
            system_prompt: None,
            max_iterations: None,
            persist: true,
        })
        .await?;

    let answer = if response.answer.trim().is_empty() {
        "我暂时还没有合适的回复，请稍后再试。".to_string()
    } else {
        response.answer
    };
    Ok(answer)
}
