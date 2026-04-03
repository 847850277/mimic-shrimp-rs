//! 微信 iLink Bot HTTP API 客户端。

use std::time::Duration;

use aes::Aes128;
use anyhow::{Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use md5::compute as md5_compute;
use reqwest::{
    Client,
    header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use uuid::Uuid;

use crate::config::WeixinChannelConfig;

use super::types::{
    WeixinAccountRecord, WeixinBaseInfo, WeixinGetUpdatesRequest, WeixinGetUpdatesResponse,
    WeixinGetUploadUrlRequest, WeixinGetUploadUrlResponse, WeixinOutboundCdnMedia,
    WeixinOutboundFileItem, WeixinOutboundMessage, WeixinOutboundMessageItem,
    WeixinOutboundTextItem, WeixinOutboundVoiceItem, WeixinQrCodeResponse, WeixinQrStatusResponse,
    WeixinSendMessageRequest, WeixinUploadedFile, WeixinUploadedVoice,
};
use aes::cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};

const DEFAULT_API_TIMEOUT_MS: u64 = 15_000;
const DEFAULT_CDN_TIMEOUT_MS: u64 = 30_000;
const WEIXIN_UPLOAD_MEDIA_TYPE_FILE: u8 = 3;
#[allow(dead_code)]
const WEIXIN_UPLOAD_MEDIA_TYPE_VOICE: u8 = 4;
#[allow(dead_code)]
const WEIXIN_VOICE_ENCODE_MP3: u8 = 7;
const WEIXIN_MEDIA_ENCRYPT_TYPE_PACK: u8 = 1;

/// 微信协议客户端。
#[derive(Clone)]
pub struct WeixinApiClient {
    http: Client,
    config: WeixinChannelConfig,
    channel_version: String,
    client_version: String,
}

impl WeixinApiClient {
    /// 创建一个微信 API 客户端。
    pub fn new(config: WeixinChannelConfig) -> Self {
        Self {
            http: Client::new(),
            config,
            channel_version: env!("CARGO_PKG_VERSION").to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// 获取二维码。
    pub async fn fetch_qr_code(&self, bot_type: &str) -> Result<WeixinQrCodeResponse> {
        let endpoint = format!(
            "ilink/bot/get_bot_qrcode?bot_type={}",
            urlencoding::encode(bot_type)
        );
        let raw = self
            .api_get(self.config.base_url.as_str(), &endpoint, 5_000)
            .await?;
        Ok(serde_json::from_str(&raw)?)
    }

    /// 轮询二维码状态。
    pub async fn poll_qr_status(
        &self,
        api_base_url: &str,
        qrcode: &str,
        timeout_ms: u64,
    ) -> Result<WeixinQrStatusResponse> {
        let endpoint = format!(
            "ilink/bot/get_qrcode_status?qrcode={}",
            urlencoding::encode(qrcode)
        );
        let raw = self.api_get(api_base_url, &endpoint, timeout_ms).await?;
        Ok(serde_json::from_str(&raw)?)
    }

    /// 长轮询获取更新。
    pub async fn get_updates(
        &self,
        account: &WeixinAccountRecord,
        get_updates_buf: &str,
        timeout_ms: u64,
    ) -> Result<WeixinGetUpdatesResponse> {
        let body = serde_json::to_string(&WeixinGetUpdatesRequest {
            get_updates_buf,
            base_info: self.base_info(),
        })?;
        let raw = match self
            .api_post(
                account.base_url.as_str(),
                "ilink/bot/getupdates",
                Some(account.bot_token.as_str()),
                body,
                timeout_ms,
            )
            .await
        {
            Ok(raw) => raw,
            Err(error) if is_timeout_error(&error) => {
                return Ok(WeixinGetUpdatesResponse {
                    ret: Some(0),
                    errcode: Some(0),
                    errmsg: None,
                    msgs: Vec::new(),
                    get_updates_buf: Some(get_updates_buf.to_string()),
                    longpolling_timeout_ms: None,
                });
            }
            Err(error) => return Err(error),
        };
        Ok(serde_json::from_str(&raw)?)
    }

    /// 发送一条文本消息。
    pub async fn send_text_message(
        &self,
        account: &WeixinAccountRecord,
        to_user_id: &str,
        text: &str,
        context_token: Option<&str>,
    ) -> Result<String> {
        let client_id = format!("mimic-shrimp-rs-{}", Uuid::new_v4());
        let payload = WeixinSendMessageRequest {
            msg: WeixinOutboundMessage {
                from_user_id: "",
                to_user_id,
                client_id: &client_id,
                message_type: 2,
                message_state: 2,
                item_list: vec![WeixinOutboundMessageItem {
                    item_type: 1,
                    text_item: Some(WeixinOutboundTextItem { text }),
                    voice_item: None,
                    file_item: None,
                }],
                context_token,
            },
            base_info: self.base_info(),
        };
        let body = serde_json::to_string(&payload)?;
        self.api_post(
            account.base_url.as_str(),
            "ilink/bot/sendmessage",
            Some(account.bot_token.as_str()),
            body,
            DEFAULT_API_TIMEOUT_MS,
        )
        .await?;
        Ok(client_id)
    }

    /// 上传一段语音到微信 CDN，并返回可用于发送消息的媒体引用。
    #[allow(dead_code)]
    pub async fn upload_voice(
        &self,
        account: &WeixinAccountRecord,
        to_user_id: &str,
        audio_bytes: &[u8],
    ) -> Result<WeixinUploadedVoice> {
        let aes_key = *Uuid::new_v4().as_bytes();
        let aes_key_hex = hex_lower(&aes_key);
        let encrypted = aes_128_ecb_pkcs7_encrypt(&aes_key, audio_bytes)?;
        let file_key = Uuid::new_v4().simple().to_string();
        let raw_md5 = format!("{:x}", md5_compute(audio_bytes));
        let payload = WeixinGetUploadUrlRequest {
            filekey: &file_key,
            media_type: WEIXIN_UPLOAD_MEDIA_TYPE_VOICE,
            to_user_id,
            rawsize: audio_bytes.len(),
            rawfilemd5: &raw_md5,
            filesize: encrypted.len(),
            no_need_thumb: true,
            aeskey: &aes_key_hex,
            base_info: self.base_info(),
        };
        let body = serde_json::to_string(&payload)?;
        let raw = self
            .api_post(
                account.base_url.as_str(),
                "ilink/bot/getuploadurl",
                Some(account.bot_token.as_str()),
                body,
                DEFAULT_API_TIMEOUT_MS,
            )
            .await?;
        let response: WeixinGetUploadUrlResponse = serde_json::from_str(&raw)?;
        let ret = response.ret.unwrap_or_default();
        let errcode = response.errcode.unwrap_or_default();
        if ret != 0 || errcode != 0 {
            bail!(
                "weixin getuploadurl failed: ret={} errcode={} message={}",
                ret,
                errcode,
                response
                    .errmsg
                    .unwrap_or_else(|| "unknown weixin upload error".to_string())
            );
        }
        let upload_url = response
            .upload_full_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .or_else(|| {
                response
                    .upload_param
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| {
                        build_cdn_upload_url(self.config.cdn_base_url.as_str(), value, &file_key)
                    })
            })
            .ok_or_else(|| anyhow!("weixin getuploadurl response missing upload target"))?;

        let download_param = self.upload_to_cdn(upload_url.as_str(), &encrypted).await?;
        let encrypt_query_param = download_param
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("weixin cdn upload response missing x-encrypted-param"))?;
        Ok(WeixinUploadedVoice {
            encrypt_query_param,
            aes_key: STANDARD.encode(aes_key_hex.as_bytes()),
        })
    }

    /// 发送一条语音消息。
    #[allow(dead_code)]
    pub async fn send_voice_message(
        &self,
        account: &WeixinAccountRecord,
        to_user_id: &str,
        voice: &WeixinUploadedVoice,
        context_token: Option<&str>,
        voice_size: Option<usize>,
        sample_rate: Option<u32>,
        playtime_ms: Option<u64>,
    ) -> Result<String> {
        let client_id = format!("mimic-shrimp-rs-{}", Uuid::new_v4());
        let payload = WeixinSendMessageRequest {
            msg: WeixinOutboundMessage {
                from_user_id: "",
                to_user_id,
                client_id: &client_id,
                message_type: 2,
                message_state: 2,
                item_list: vec![WeixinOutboundMessageItem {
                    item_type: 3,
                    text_item: None,
                    voice_item: Some(WeixinOutboundVoiceItem {
                        media: WeixinOutboundCdnMedia {
                            encrypt_query_param: &voice.encrypt_query_param,
                            aes_key: &voice.aes_key,
                            encrypt_type: Some(WEIXIN_MEDIA_ENCRYPT_TYPE_PACK),
                        },
                        voice_size,
                        encode_type: Some(WEIXIN_VOICE_ENCODE_MP3),
                        bits_per_sample: Some(16),
                        sample_rate,
                        playtime: playtime_ms,
                    }),
                    file_item: None,
                }],
                context_token,
            },
            base_info: self.base_info(),
        };
        let body = serde_json::to_string(&payload)?;
        self.api_post(
            account.base_url.as_str(),
            "ilink/bot/sendmessage",
            Some(account.bot_token.as_str()),
            body,
            DEFAULT_API_TIMEOUT_MS,
        )
        .await?;
        Ok(client_id)
    }

    /// 上传一个文件附件到微信 CDN，并返回可用于发送文件消息的媒体引用。
    pub async fn upload_file(
        &self,
        account: &WeixinAccountRecord,
        to_user_id: &str,
        file_bytes: &[u8],
    ) -> Result<WeixinUploadedFile> {
        let aes_key = *Uuid::new_v4().as_bytes();
        let aes_key_hex = hex_lower(&aes_key);
        let encrypted = aes_128_ecb_pkcs7_encrypt(&aes_key, file_bytes)?;
        let file_key = Uuid::new_v4().simple().to_string();
        let raw_md5 = format!("{:x}", md5_compute(file_bytes));
        let payload = WeixinGetUploadUrlRequest {
            filekey: &file_key,
            media_type: WEIXIN_UPLOAD_MEDIA_TYPE_FILE,
            to_user_id,
            rawsize: file_bytes.len(),
            rawfilemd5: &raw_md5,
            filesize: encrypted.len(),
            no_need_thumb: true,
            aeskey: &aes_key_hex,
            base_info: self.base_info(),
        };
        let body = serde_json::to_string(&payload)?;
        let raw = self
            .api_post(
                account.base_url.as_str(),
                "ilink/bot/getuploadurl",
                Some(account.bot_token.as_str()),
                body,
                DEFAULT_API_TIMEOUT_MS,
            )
            .await?;
        let response: WeixinGetUploadUrlResponse = serde_json::from_str(&raw)?;
        let ret = response.ret.unwrap_or_default();
        let errcode = response.errcode.unwrap_or_default();
        if ret != 0 || errcode != 0 {
            bail!(
                "weixin getuploadurl failed: ret={} errcode={} message={}",
                ret,
                errcode,
                response
                    .errmsg
                    .unwrap_or_else(|| "unknown weixin upload error".to_string())
            );
        }
        let upload_url = response
            .upload_full_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .or_else(|| {
                response
                    .upload_param
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| {
                        build_cdn_upload_url(self.config.cdn_base_url.as_str(), value, &file_key)
                    })
            })
            .ok_or_else(|| anyhow!("weixin getuploadurl response missing upload target"))?;
        let download_param = self.upload_to_cdn(upload_url.as_str(), &encrypted).await?;
        let encrypt_query_param = download_param
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("weixin cdn upload response missing x-encrypted-param"))?;
        Ok(WeixinUploadedFile {
            encrypt_query_param,
            aes_key: STANDARD.encode(aes_key_hex.as_bytes()),
            file_size: file_bytes.len(),
        })
    }

    /// 发送一条文件消息。
    pub async fn send_file_message(
        &self,
        account: &WeixinAccountRecord,
        to_user_id: &str,
        file_name: &str,
        file: &WeixinUploadedFile,
        context_token: Option<&str>,
    ) -> Result<String> {
        let client_id = format!("mimic-shrimp-rs-{}", Uuid::new_v4());
        let file_len = file.file_size.to_string();
        let payload = WeixinSendMessageRequest {
            msg: WeixinOutboundMessage {
                from_user_id: "",
                to_user_id,
                client_id: &client_id,
                message_type: 2,
                message_state: 2,
                item_list: vec![WeixinOutboundMessageItem {
                    item_type: 4,
                    text_item: None,
                    voice_item: None,
                    file_item: Some(WeixinOutboundFileItem {
                        media: WeixinOutboundCdnMedia {
                            encrypt_query_param: &file.encrypt_query_param,
                            aes_key: &file.aes_key,
                            encrypt_type: Some(WEIXIN_MEDIA_ENCRYPT_TYPE_PACK),
                        },
                        file_name: Some(file_name),
                        md5: None,
                        len: Some(file_len.as_str()),
                    }),
                }],
                context_token,
            },
            base_info: self.base_info(),
        };
        let body = serde_json::to_string(&payload)?;
        self.api_post(
            account.base_url.as_str(),
            "ilink/bot/sendmessage",
            Some(account.bot_token.as_str()),
            body,
            DEFAULT_API_TIMEOUT_MS,
        )
        .await?;
        Ok(client_id)
    }

    fn base_info(&self) -> WeixinBaseInfo<'_> {
        WeixinBaseInfo {
            channel_version: &self.channel_version,
        }
    }

    async fn api_get(&self, base_url: &str, endpoint: &str, timeout_ms: u64) -> Result<String> {
        let url = format!("{}/{}", base_url.trim_end_matches('/'), endpoint);
        let response = self
            .http
            .get(url)
            .headers(self.common_headers()?)
            .timeout(Duration::from_millis(timeout_ms))
            .send()
            .await?;
        Self::read_text_response(response).await
    }

    async fn api_post(
        &self,
        base_url: &str,
        endpoint: &str,
        token: Option<&str>,
        body: String,
        timeout_ms: u64,
    ) -> Result<String> {
        let url = format!("{}/{}", base_url.trim_end_matches('/'), endpoint);
        let mut headers = self.auth_headers(token, body.len())?;
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let response = self
            .http
            .post(url)
            .headers(headers)
            .timeout(Duration::from_millis(timeout_ms))
            .body(body)
            .send()
            .await?;
        Self::read_text_response(response).await
    }

    fn common_headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        let client_version = build_client_version(self.client_version.as_str())?.to_string();
        headers.insert(
            "iLink-App-Id",
            HeaderValue::from_str(self.config.ilink_app_id.as_str())?,
        );
        headers.insert(
            "iLink-App-ClientVersion",
            HeaderValue::from_str(client_version.as_str())?,
        );
        if let Some(route_tag) = self.config.route_tag.as_deref() {
            headers.insert("SKRouteTag", HeaderValue::from_str(route_tag)?);
        }
        Ok(headers)
    }

    fn auth_headers(&self, token: Option<&str>, body_len: usize) -> Result<HeaderMap> {
        let mut headers = self.common_headers()?;
        headers.insert(
            "AuthorizationType",
            HeaderValue::from_static("ilink_bot_token"),
        );
        headers.insert(
            "X-WECHAT-UIN",
            HeaderValue::from_str(random_wechat_uin().as_str())?,
        );
        headers.insert(
            CONTENT_LENGTH,
            HeaderValue::from_str(body_len.to_string().as_str())?,
        );
        if let Some(value) = token.filter(|value| !value.trim().is_empty()) {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(format!("Bearer {}", value.trim()).as_str())?,
            );
        }
        Ok(headers)
    }

    async fn read_text_response(response: reqwest::Response) -> Result<String> {
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            bail!("weixin api returned HTTP {}: {}", status.as_u16(), body);
        }
        Ok(body)
    }

    async fn upload_to_cdn(&self, upload_url: &str, encrypted: &[u8]) -> Result<Option<String>> {
        let response = self
            .http
            .post(upload_url)
            .header(CONTENT_TYPE, "application/octet-stream")
            .timeout(Duration::from_millis(DEFAULT_CDN_TIMEOUT_MS))
            .body(encrypted.to_vec())
            .send()
            .await?;
        let status = response.status();
        let encrypted_query = response
            .headers()
            .get("x-encrypted-param")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string)
            .filter(|value| !value.trim().is_empty());
        let body = response.text().await?;
        if !status.is_success() {
            bail!(
                "weixin cdn upload returned HTTP {}: {}",
                status.as_u16(),
                body
            );
        }
        Ok(encrypted_query)
    }
}

fn build_client_version(version: &str) -> Result<u32> {
    let parts = version
        .split('.')
        .map(|segment| segment.parse::<u32>())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| anyhow!("invalid client version {version}: {error}"))?;
    let major = parts.first().copied().unwrap_or_default();
    let minor = parts.get(1).copied().unwrap_or_default();
    let patch = parts.get(2).copied().unwrap_or_default();
    Ok(((major & 0xff) << 16) | ((minor & 0xff) << 8) | (patch & 0xff))
}

fn random_wechat_uin() -> String {
    let binding = Uuid::new_v4();
    let bytes = binding.as_bytes();
    let number = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    STANDARD.encode(number.to_string())
}

fn is_timeout_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<reqwest::Error>()
        .is_some_and(reqwest::Error::is_timeout)
}

fn build_cdn_upload_url(cdn_base_url: &str, upload_param: &str, file_key: &str) -> String {
    format!(
        "{}/upload?encrypted_query_param={}&filekey={}",
        cdn_base_url.trim_end_matches('/'),
        urlencoding::encode(upload_param.trim()),
        urlencoding::encode(file_key)
    )
}

fn aes_128_ecb_pkcs7_encrypt(key: &[u8; 16], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes128::new_from_slice(key)
        .map_err(|error| anyhow!("invalid weixin aes key length: {error}"))?;
    let mut buffer = pkcs7_pad(plaintext, 16);
    for chunk in buffer.chunks_exact_mut(16) {
        let block = GenericArray::from_mut_slice(chunk);
        cipher.encrypt_block(block);
    }
    Ok(buffer)
}

fn pkcs7_pad(input: &[u8], block_size: usize) -> Vec<u8> {
    let mut output = input.to_vec();
    let mut pad_len = block_size - (output.len() % block_size);
    if pad_len == 0 {
        pad_len = block_size;
    }
    output.extend(std::iter::repeat_n(pad_len as u8, pad_len));
    output
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}
