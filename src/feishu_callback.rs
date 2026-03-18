use aes::Aes256;
use anyhow::{Result, anyhow};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use cbc::Decryptor;
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::FeishuCallbackConfig;

type Aes256CbcDec = Decryptor<Aes256>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeishuCallbackErrorKind {
    BadRequest,
    Unauthorized,
}

#[derive(Debug, Clone)]
pub struct FeishuCallbackError {
    pub kind: FeishuCallbackErrorKind,
    pub message: String,
}

impl FeishuCallbackError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            kind: FeishuCallbackErrorKind::BadRequest,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            kind: FeishuCallbackErrorKind::Unauthorized,
            message: message.into(),
        }
    }
}

#[derive(Debug)]
pub struct FeishuCallbackOutcome {
    pub payload: Value,
    pub response_body: Value,
    pub encrypted: bool,
}

pub fn process_callback(
    raw_body: Value,
    config: &FeishuCallbackConfig,
) -> std::result::Result<FeishuCallbackOutcome, FeishuCallbackError> {
    let encrypted = raw_body.get("encrypt").and_then(Value::as_str).is_some();
    let payload = if let Some(encrypt) = raw_body.get("encrypt").and_then(Value::as_str) {
        let key = config.encrypt_key.as_deref().ok_or_else(|| {
            FeishuCallbackError::bad_request(
                "received encrypted callback but FEISHU_CALLBACK_ENCRYPT_KEY is not configured",
            )
        })?;
        decrypt_payload(encrypt, key).map_err(|error| {
            FeishuCallbackError::bad_request(format!("failed to decrypt callback: {error}"))
        })?
    } else {
        raw_body
    };

    validate_verification_token(&payload, config.verification_token.as_deref())?;

    let response_body = if let Some(challenge) = payload.get("challenge").and_then(Value::as_str) {
        json!({ "challenge": challenge })
    } else {
        json!({
            "toast": {
                "type": "info",
                "content": "已收到飞书回调",
                "i18n": {
                    "zh_cn": "已收到飞书回调",
                    "en_us": "Callback received"
                }
            }
        })
    };

    Ok(FeishuCallbackOutcome {
        payload,
        response_body,
        encrypted,
    })
}

pub fn extract_event_type(payload: &Value) -> Option<&str> {
    payload
        .pointer("/header/event_type")
        .and_then(Value::as_str)
        .or_else(|| payload.pointer("/event/type").and_then(Value::as_str))
        .or_else(|| payload.get("type").and_then(Value::as_str))
}

fn validate_verification_token(
    payload: &Value,
    expected_token: Option<&str>,
) -> std::result::Result<(), FeishuCallbackError> {
    let Some(expected_token) = expected_token else {
        return Ok(());
    };

    let actual_token = payload
        .pointer("/header/token")
        .and_then(Value::as_str)
        .or_else(|| payload.get("token").and_then(Value::as_str))
        .ok_or_else(|| {
            FeishuCallbackError::unauthorized(
                "missing verification token in callback payload while FEISHU_CALLBACK_VERIFICATION_TOKEN is configured",
            )
        })?;

    if actual_token != expected_token {
        return Err(FeishuCallbackError::unauthorized(
            "verification token does not match FEISHU_CALLBACK_VERIFICATION_TOKEN",
        ));
    }

    Ok(())
}

fn decrypt_payload(encrypt: &str, encrypt_key: &str) -> Result<Value> {
    let decoded = BASE64
        .decode(encrypt)
        .map_err(|error| anyhow!("invalid base64 encrypt payload: {error}"))?;
    if decoded.len() < 17 {
        return Err(anyhow!("encrypted payload is too short"));
    }

    let iv = &decoded[..16];
    let ciphertext = &decoded[16..];
    let key = Sha256::digest(encrypt_key.as_bytes());
    let mut buffer = ciphertext.to_vec();
    let plaintext = Aes256CbcDec::new((&key).into(), iv.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buffer)
        .map_err(|error| anyhow!("invalid AES-256-CBC payload: {error}"))?;

    serde_json::from_slice::<Value>(plaintext)
        .map_err(|error| anyhow!("decrypted payload is not valid JSON: {error}"))
}

#[cfg(test)]
mod tests {
    use aes::Aes256;
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
    use cbc::Encryptor;
    use cbc::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};

    use super::*;

    type Aes256CbcEnc = Encryptor<Aes256>;

    #[test]
    fn echoes_plaintext_challenge() {
        let outcome = process_callback(
            json!({
                "challenge": "hello",
                "token": "token-1"
            }),
            &FeishuCallbackConfig {
                verification_token: Some("token-1".to_string()),
                encrypt_key: None,
                ..FeishuCallbackConfig::default()
            },
        )
        .expect("challenge response");

        assert_eq!(outcome.response_body, json!({"challenge": "hello"}));
        assert!(!outcome.encrypted);
    }

    #[test]
    fn decrypts_encrypted_payload_and_returns_toast() {
        let payload = json!({
            "header": {
                "token": "token-1",
                "event_type": "card.action.trigger"
            },
            "event": {
                "action": {
                    "value": {
                        "action": "approve"
                    }
                }
            }
        });
        let encrypted = encrypt_for_test(&payload, "encrypt-key").expect("encrypt");

        let outcome = process_callback(
            json!({ "encrypt": encrypted }),
            &FeishuCallbackConfig {
                verification_token: Some("token-1".to_string()),
                encrypt_key: Some("encrypt-key".to_string()),
                ..FeishuCallbackConfig::default()
            },
        )
        .expect("encrypted callback");

        assert_eq!(
            extract_event_type(&outcome.payload),
            Some("card.action.trigger")
        );
        assert_eq!(
            outcome
                .response_body
                .pointer("/toast/content")
                .and_then(Value::as_str),
            Some("已收到飞书回调")
        );
        assert!(outcome.encrypted);
    }

    #[test]
    fn rejects_invalid_verification_token() {
        let error = process_callback(
            json!({
                "header": {
                    "token": "wrong-token"
                }
            }),
            &FeishuCallbackConfig {
                verification_token: Some("expected-token".to_string()),
                encrypt_key: None,
                ..FeishuCallbackConfig::default()
            },
        )
        .expect_err("token mismatch should fail");

        assert_eq!(error.kind, FeishuCallbackErrorKind::Unauthorized);
    }

    fn encrypt_for_test(payload: &Value, encrypt_key: &str) -> Result<String> {
        let iv = [7_u8; 16];
        let key = Sha256::digest(encrypt_key.as_bytes());
        let plaintext = serde_json::to_vec(payload)?;
        let mut buffer = plaintext.clone();
        let msg_len = buffer.len();
        let padded_len = ((msg_len / 16) + 1) * 16;
        buffer.resize(padded_len, 0);

        let ciphertext = Aes256CbcEnc::new((&key).into(), (&iv).into())
            .encrypt_padded_mut::<Pkcs7>(&mut buffer, msg_len)
            .map_err(|error| anyhow!("failed to encrypt test payload: {error}"))?;

        let mut combined = iv.to_vec();
        combined.extend_from_slice(ciphertext);
        Ok(BASE64.encode(combined))
    }
}
