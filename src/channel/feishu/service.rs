//! 飞书服务编排模块，负责把飞书消息事件接到引擎回合并将结果回复给飞书。

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

use crate::{
    capability::{
        ConversationCapability, ConversationRequest, EnglishLearningCapability,
        MediaTranslateCapability, MediaTranslateInput, MediaTranslateRequest,
        SpeechSynthesisCapability, SpeechSynthesisRequest,
    },
    channel::{InboundAudioMessage, InboundTextMessage, OutboundAudioReply, OutboundTextReply},
    config::FeishuCallbackConfig,
    logging,
};

use super::{FeishuBotClient, estimate_audio_duration_ms};

/// 处理一条统一入站文本消息，并通过飞书回复链路将结果回复回原消息。
pub async fn handle_text_message_event(
    conversation: ConversationCapability,
    english_learning: EnglishLearningCapability,
    config: FeishuCallbackConfig,
    event: InboundTextMessage,
) -> Result<()> {
    let answer = build_text_reply(
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
    speech_synthesis: SpeechSynthesisCapability,
    config: FeishuCallbackConfig,
    event: InboundAudioMessage,
) -> Result<()> {
    let client = FeishuBotClient::new(config.clone());
    if matches!(event.duration_ms, Some(0)) {
        let reply = OutboundTextReply {
            channel: event.channel,
            reply_to_message_id: event.message_id.clone(),
            session_id: event.session_id.clone(),
            text: "我收到了这条语音，但飞书回调里显示时长是 0 秒。这种语音通常无法稳定转写，请重新录一条 1 秒以上、内容更完整的语音。".to_string(),
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
    let learning_audio_mode = english_learning
        .has_active_lesson_session(&event.session_id)
        .await;
    let source_lang = if learning_audio_mode {
        Some("en".to_string())
    } else {
        config.audio_source_lang.clone()
    };
    let target_lang = if learning_audio_mode {
        "en".to_string()
    } else {
        config.audio_target_lang.clone()
    };
    let transcript_response = media_translate
        .execute(MediaTranslateRequest {
            source_lang,
            target_lang,
            input: MediaTranslateInput::Audio {
                data: audio_data_url,
                format: audio.format,
            },
            output_audio: None,
            include_usage: true,
        })
        .await;
    let transcript = match transcript_response {
        Ok(value) => value.translated_text.trim().to_string(),
        Err(error) => {
            let reply = OutboundTextReply {
                channel: event.channel,
                reply_to_message_id: event.message_id.clone(),
                session_id: event.session_id.clone(),
                text: if learning_audio_mode {
                    "我收到了这条英语跟读语音，但这次没有成功识别出英文文本。请尽量录制 1 秒以上、语速稍慢一点、环境更安静的语音后再试。".to_string()
                } else {
                    "我收到了这条语音，但这次没有成功识别出可用文本。请重新录一条更清晰、稍长一点的语音后再试。".to_string()
                },
            };
            client.send_text_reply(&reply).await?;
            logging::log_channel_text_replied(
                reply.channel.as_str(),
                &reply.reply_to_message_id,
                &reply.session_id,
                &reply.text,
            );
            return Err(anyhow::anyhow!(
                "failed to transcribe channel audio: {error}"
            ));
        }
    };

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
    let answer = build_audio_reply(
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

    if speech_synthesis.is_configured() && should_send_english_audio_reply(&transcript, &reply.text)
    {
        if let Err(error) = send_english_audio_reply(
            &client,
            &speech_synthesis,
            reply.channel,
            &reply.reply_to_message_id,
            &reply.session_id,
            &reply.text,
        )
        .await
        {
            logging::log_channel_background_error(
                reply.channel.as_str(),
                &format!("failed to send synthesized english audio reply: {error}"),
            );
        }
    }
    Ok(())
}

/// 返回飞书事件接收成功时的标准 ACK 响应。
pub fn callback_ack() -> Value {
    json!({
        "code": 0,
        "msg": "ok"
    })
}

async fn build_text_reply(
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

async fn build_audio_reply(
    conversation: &ConversationCapability,
    english_learning: &EnglishLearningCapability,
    session_id: &str,
    user_id: &str,
    transcript: &str,
) -> Result<String> {
    if let Some(reply) = english_learning
        .maybe_handle_message(session_id, transcript)
        .await?
    {
        let trimmed = reply.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    if let Some(reply) = english_learning
        .maybe_handle_shadowing_audio(session_id, transcript)
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
            message: transcript.to_string(),
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

async fn send_english_audio_reply(
    client: &FeishuBotClient,
    speech_synthesis: &SpeechSynthesisCapability,
    channel: crate::channel::ChannelKind,
    reply_to_message_id: &str,
    session_id: &str,
    text: &str,
) -> Result<()> {
    let normalized_text = normalize_text_for_speech(text);
    if normalized_text.is_empty() || !looks_like_english_text(&normalized_text) {
        return Ok(());
    }

    let synthesized = speech_synthesis
        .execute(SpeechSynthesisRequest {
            text: normalized_text,
            model: None,
            voice: None,
            response_format: Some("opus".to_string()),
            sample_rate: Some(48_000),
            speed: None,
            gain: None,
            stream: Some(false),
        })
        .await?;
    let audio_bytes = STANDARD
        .decode(&synthesized.audio_base64)
        .map_err(|error| anyhow::anyhow!("invalid synthesized audio base64: {error}"))?;
    let duration_ms = estimate_audio_duration_ms(&synthesized.response_format, &audio_bytes);
    let reply = OutboundAudioReply {
        channel,
        reply_to_message_id: reply_to_message_id.to_string(),
        session_id: session_id.to_string(),
        file_name: format!("english-reply.{}", synthesized.response_format),
        file_format: synthesized.response_format,
        content_type: synthesized.content_type,
        bytes: audio_bytes,
        duration_ms,
    };
    client.send_audio_reply(&reply).await?;
    logging::log_channel_audio_replied(
        reply.channel.as_str(),
        &reply.reply_to_message_id,
        &reply.session_id,
        &reply.file_name,
        &reply.file_format,
        reply.duration_ms,
    );
    Ok(())
}

fn should_send_english_audio_reply(transcript: &str, answer: &str) -> bool {
    looks_like_english_text(transcript)
        && looks_like_english_text(&normalize_text_for_speech(answer))
}

fn normalize_text_for_speech(input: &str) -> String {
    input
        .replace("\r\n", "\n")
        .replace("**", "")
        .replace("__", "")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn looks_like_english_text(input: &str) -> bool {
    let mut latin_letters = 0usize;
    let mut cjk_chars = 0usize;
    let mut words = 0usize;
    let mut in_word = false;

    for ch in input.chars() {
        if ch.is_ascii_alphabetic() {
            latin_letters += 1;
            if !in_word {
                words += 1;
                in_word = true;
            }
        } else {
            in_word = false;
            if ('\u{4e00}'..='\u{9fff}').contains(&ch) {
                cjk_chars += 1;
            }
        }
    }

    latin_letters >= 12 && words >= 3 && latin_letters > cjk_chars * 2
}

#[cfg(test)]
mod tests {
    use super::{
        looks_like_english_text, normalize_text_for_speech, should_send_english_audio_reply,
    };

    #[test]
    fn detects_english_text_heuristically() {
        assert!(looks_like_english_text(
            "President Trump is engaging in a blend of diplomacy and diversions."
        ));
        assert!(!looks_like_english_text(
            "总统正在进行外交与消遣的混合活动。"
        ));
    }

    #[test]
    fn normalizes_multiline_markdownish_text_for_speech() {
        assert_eq!(
            normalize_text_for_speech("**Hello**\n\nThis is  a test.\n"),
            "Hello This is a test."
        );
    }

    #[test]
    fn only_sends_audio_reply_for_english_transcript_and_answer() {
        assert!(should_send_english_audio_reply(
            "How can I improve my pronunciation today?",
            "Try reading the focus sentence one more time."
        ));
        assert!(!should_send_english_audio_reply(
            "How can I improve my pronunciation today?",
            "你可以再读一遍重点句子。"
        ));
    }
}
