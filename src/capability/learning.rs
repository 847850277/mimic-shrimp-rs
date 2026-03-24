//! 英语学习能力模块，负责每天抓取新闻、生成“今日英语学习卡片”并处理飞书学习口令。

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{FixedOffset, Timelike, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use serde_xml_rs::from_str;
use tokio::{sync::RwLock, time::sleep};

use super::{StructuredExtractionCapability, StructuredExtractionRequest};
use crate::{config::EnglishLearningConfig, logging};

/// 新闻抓取与摘要结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsDigest {
    pub lesson_date: String,
    pub article: NewsArticle,
    pub summary_en: String,
    pub summary_zh: String,
    pub keywords: Vec<String>,
}

/// 新闻条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsArticle {
    pub source_url: String,
    pub title: String,
    pub link: String,
    pub summary: String,
    pub published_at: Option<String>,
}

/// 每日英语学习卡片。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyEnglishLesson {
    pub lesson_date: String,
    pub generated_at_ms: u64,
    pub article: NewsArticle,
    pub headline_zh: String,
    pub summary_en: String,
    pub summary_zh: String,
    pub keywords: Vec<String>,
    pub vocabulary: Vec<LearningVocabulary>,
    pub example_sentences: Vec<LearningSentence>,
    pub questions: Vec<String>,
    pub shadowing_practice: String,
    pub translation_practice: String,
    pub focus_sentence: String,
}

/// 学习卡片中的重点词汇。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LearningVocabulary {
    pub word: String,
    pub meaning_zh: String,
    pub example_en: String,
    pub example_zh: String,
}

/// 学习卡片中的双语句子。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LearningSentence {
    pub english: String,
    pub chinese: String,
}

/// 新闻抓取能力，负责拉取固定 RSS 源、去重，并产出摘要与关键词。
#[derive(Clone)]
pub struct NewsIngestCapability {
    client: Client,
    extraction: StructuredExtractionCapability,
    config: EnglishLearningConfig,
}

/// 英语学习能力，负责生成今日学习卡片、落盘保存并响应飞书学习口令。
#[derive(Clone)]
pub struct EnglishLearningCapability {
    config: EnglishLearningConfig,
    extraction: StructuredExtractionCapability,
    news_ingest: NewsIngestCapability,
    lesson_store: LessonStore,
    session_store: LearningSessionStore,
}

#[derive(Debug, Clone, Default)]
struct LearningSessionState {
    focus_sentence: String,
    next_question_index: usize,
}

#[derive(Clone, Default)]
struct LearningSessionStore {
    inner: Arc<RwLock<HashMap<String, LearningSessionState>>>,
}

#[derive(Clone)]
struct LessonStore {
    storage_dir: PathBuf,
}

#[derive(Debug, Clone, Copy)]
enum LearningCommand {
    StartTodayLesson,
    ExplainFocusSentence,
    NextQuestion,
}

#[derive(Debug, Deserialize)]
struct DigestFields {
    #[serde(default)]
    summary_en: String,
    #[serde(default)]
    summary_zh: String,
    #[serde(default)]
    keywords: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct GeneratedLessonFields {
    #[serde(default)]
    headline_zh: String,
    #[serde(default)]
    summary_en: String,
    #[serde(default)]
    summary_zh: String,
    #[serde(default)]
    vocabulary: Vec<LearningVocabulary>,
    #[serde(default)]
    example_sentences: Vec<LearningSentence>,
    #[serde(default)]
    questions: Vec<String>,
    #[serde(default)]
    shadowing_practice: String,
    #[serde(default)]
    translation_practice: String,
    #[serde(default)]
    focus_sentence: String,
}

#[derive(Debug, Deserialize)]
struct RssFeed {
    channel: RssChannel,
}

#[derive(Debug, Deserialize)]
struct RssChannel {
    #[serde(rename = "item", default)]
    items: Vec<RssItem>,
}

#[derive(Debug, Deserialize)]
struct RssItem {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    link: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(rename = "pubDate", default)]
    pub_date: Option<String>,
}

impl NewsIngestCapability {
    /// 基于抽取能力与配置创建新闻抓取能力。
    pub fn new(config: EnglishLearningConfig, extraction: StructuredExtractionCapability) -> Self {
        Self {
            client: Client::new(),
            extraction,
            config,
        }
    }

    /// 拉取固定新闻源并构建当天的新闻摘要。
    pub async fn ingest_daily_news(&self, lesson_date: &str) -> Result<NewsDigest> {
        logging::log_learning_news_ingest_started(lesson_date, self.config.news_sources.len());
        let article = self.fetch_latest_article().await?;
        logging::log_learning_news_selected(lesson_date, &article.title, &article.link);

        let response = self
            .extraction
            .execute(StructuredExtractionRequest {
                schema: news_digest_schema(),
                input_text: format!(
                    "Source URL: {}\nTitle: {}\nPublished at: {}\nSummary: {}\nLink: {}",
                    article.source_url,
                    article.title,
                    article.published_at.clone().unwrap_or_else(|| "unknown".to_string()),
                    article.summary,
                    article.link,
                ),
                schema_name: Some("daily_english_news_digest".to_string()),
                instructions: Some("Generate an English summary, a Chinese summary, and 3 to 5 concise English keywords for the selected news item.".to_string()),
            })
            .await?;

        let digest_fields: DigestFields = serde_json::from_value(response.data)
            .context("failed to parse news digest fields from structured extraction")?;

        Ok(NewsDigest {
            lesson_date: lesson_date.to_string(),
            article,
            summary_en: coalesce_non_empty(&[
                digest_fields.summary_en.as_str(),
                digest_fields.summary_zh.as_str(),
            ]),
            summary_zh: coalesce_non_empty(&[
                digest_fields.summary_zh.as_str(),
                digest_fields.summary_en.as_str(),
            ]),
            keywords: normalize_keywords(digest_fields.keywords),
        })
    }

    async fn fetch_latest_article(&self) -> Result<NewsArticle> {
        let mut seen = HashSet::new();
        let mut first_error = None;

        for source_url in &self.config.news_sources {
            let response = self
                .client
                .get(source_url)
                .send()
                .await
                .with_context(|| format!("failed to fetch news source: {source_url}"));
            let response = match response {
                Ok(value) => value,
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                    continue;
                }
            };

            let status = response.status();
            let body = response
                .text()
                .await
                .with_context(|| format!("failed to read news source body: {source_url}"))?;
            if !status.is_success() {
                let error = anyhow!("news source returned status {}: {}", status, body);
                if first_error.is_none() {
                    first_error = Some(error);
                }
                continue;
            }

            let articles = parse_rss_feed(source_url, &body)
                .with_context(|| format!("failed to parse RSS feed: {source_url}"))?;
            for article in articles
                .into_iter()
                .take(self.config.max_feed_items_per_source)
            {
                let dedupe_key = normalize_dedupe_key(&article);
                if dedupe_key.is_empty() || !seen.insert(dedupe_key) {
                    continue;
                }
                return Ok(article);
            }
        }

        Err(first_error
            .unwrap_or_else(|| anyhow!("no usable news article found in configured feeds")))
    }
}

impl EnglishLearningCapability {
    /// 基于抽取能力和配置创建英语学习能力。
    pub fn new(config: EnglishLearningConfig, extraction: StructuredExtractionCapability) -> Self {
        let lesson_store = LessonStore::new(config.storage_dir.clone());
        let news_ingest = NewsIngestCapability::new(config.clone(), extraction.clone());
        Self {
            config,
            extraction,
            news_ingest,
            lesson_store,
            session_store: LearningSessionStore::default(),
        }
    }

    /// 运行服务内每日调度器。
    pub async fn run_scheduler_loop(self) {
        if !self.config.enabled || !self.config.scheduler_enabled {
            return;
        }

        logging::log_learning_scheduler_started(
            self.config.schedule_hour,
            self.config.timezone_offset_hours,
        );
        let mut last_processed_date: Option<String> = None;

        loop {
            match self.current_lesson_date() {
                Ok(lesson_date) => {
                    let local_now = self.current_local_time();
                    if local_now.hour() >= self.config.schedule_hour
                        && last_processed_date.as_deref() != Some(lesson_date.as_str())
                    {
                        last_processed_date = Some(lesson_date.clone());
                        if let Err(error) = self.ensure_lesson_for_date(&lesson_date).await {
                            logging::log_learning_background_error(
                                "scheduler_refresh",
                                &error.to_string(),
                            );
                        }
                    }
                }
                Err(error) => {
                    logging::log_learning_background_error("scheduler_clock", &error.to_string())
                }
            }

            sleep(Duration::from_secs(60)).await;
        }
    }

    /// 在飞书口令命中时返回学习回复；未命中时返回 `None`。
    pub async fn maybe_handle_message(
        &self,
        session_id: &str,
        message: &str,
    ) -> Result<Option<String>> {
        let Some(command) = detect_learning_command(message) else {
            return Ok(None);
        };

        if !self.config.enabled {
            return Ok(Some("英语学习功能当前未启用。".to_string()));
        }

        let lesson = self.ensure_today_lesson().await?;
        let reply = match command {
            LearningCommand::StartTodayLesson => self.start_today_lesson(session_id, &lesson).await,
            LearningCommand::ExplainFocusSentence => {
                self.explain_focus_sentence(session_id, &lesson).await
            }
            LearningCommand::NextQuestion => self.next_question(session_id, &lesson).await,
        };
        logging::log_learning_command_handled(session_id, command.as_str(), &lesson.lesson_date);
        Ok(Some(reply))
    }

    /// 确保今天的英语学习卡片已经存在，不存在则即时生成。
    pub async fn ensure_today_lesson(&self) -> Result<DailyEnglishLesson> {
        let lesson_date = self.current_lesson_date()?;
        self.ensure_lesson_for_date(&lesson_date).await
    }

    async fn ensure_lesson_for_date(&self, lesson_date: &str) -> Result<DailyEnglishLesson> {
        if let Some(existing) = self.lesson_store.load(lesson_date)? {
            return Ok(existing);
        }

        let digest = self.news_ingest.ingest_daily_news(lesson_date).await?;
        let response = self
            .extraction
            .execute(StructuredExtractionRequest {
                schema: daily_lesson_schema(),
                input_text: format!(
                    "Lesson date: {}\nNews title: {}\nNews link: {}\nEnglish summary: {}\nChinese summary: {}\nKeywords: {}\nOriginal summary: {}",
                    digest.lesson_date,
                    digest.article.title,
                    digest.article.link,
                    digest.summary_en,
                    digest.summary_zh,
                    digest.keywords.join(", "),
                    digest.article.summary,
                ),
                schema_name: Some("daily_english_learning_card".to_string()),
                instructions: Some("Generate a concise daily English learning card with Chinese support. Include 3 vocabulary items, 2 bilingual example sentences, 3 comprehension questions, one shadowing practice, one translation practice, and one focus sentence copied from the example sentences when possible.".to_string()),
            })
            .await?;

        let generated: GeneratedLessonFields = serde_json::from_value(response.data)
            .context("failed to parse daily english lesson fields")?;
        let lesson = build_daily_lesson(&digest, generated);
        let path = self.lesson_store.save(&lesson)?;
        logging::log_learning_lesson_saved(
            &lesson.lesson_date,
            &path,
            lesson.questions.len(),
            lesson.vocabulary.len(),
        );
        Ok(lesson)
    }

    async fn start_today_lesson(&self, session_id: &str, lesson: &DailyEnglishLesson) -> String {
        let first_question = lesson
            .questions
            .first()
            .cloned()
            .unwrap_or_else(|| "请先用英语复述一下今天新闻的核心内容。".to_string());
        let focus_sentence = select_focus_sentence(lesson);
        self.session_store
            .start_session(session_id, &lesson.lesson_date, &focus_sentence, 1)
            .await;

        format_lesson_card(lesson, &first_question)
    }

    async fn explain_focus_sentence(
        &self,
        session_id: &str,
        lesson: &DailyEnglishLesson,
    ) -> String {
        let fallback_focus = select_focus_sentence(lesson);
        let state =
            self.session_store
                .get(session_id)
                .await
                .unwrap_or_else(|| LearningSessionState {
                    focus_sentence: fallback_focus.clone(),
                    next_question_index: 0,
                });
        let focus_sentence = if state.focus_sentence.trim().is_empty() {
            fallback_focus
        } else {
            state.focus_sentence
        };

        let chinese = lesson
            .example_sentences
            .iter()
            .find(|item| item.english == focus_sentence)
            .map(|item| item.chinese.clone())
            .unwrap_or_else(|| lesson.summary_zh.clone());
        let related_vocab = lesson
            .vocabulary
            .iter()
            .take(2)
            .map(|item| format!("{}：{}", item.word, item.meaning_zh))
            .collect::<Vec<_>>();

        format!(
            "当前重点句子：\n{}\n\n中文意思：\n{}\n\n相关词汇：\n{}\n\n你可以继续回复“再出一道题”，我会继续给你练习。",
            focus_sentence,
            chinese,
            if related_vocab.is_empty() {
                "今天这句主要用来感受新闻表达的语气。".to_string()
            } else {
                related_vocab.join("\n")
            }
        )
    }

    async fn next_question(&self, session_id: &str, lesson: &DailyEnglishLesson) -> String {
        let state =
            self.session_store
                .get(session_id)
                .await
                .unwrap_or_else(|| LearningSessionState {
                    focus_sentence: select_focus_sentence(lesson),
                    next_question_index: 0,
                });
        let questions = if lesson.questions.is_empty() {
            vec!["请尝试用英语概括这条新闻的主要信息。".to_string()]
        } else {
            lesson.questions.clone()
        };
        let index = state.next_question_index % questions.len();
        let question = questions[index].clone();
        self.session_store
            .start_session(
                session_id,
                &lesson.lesson_date,
                &state.focus_sentence,
                state.next_question_index + 1,
            )
            .await;

        format!(
            "今日英语练习题 {}/{}：\n{}\n\n你可以直接回复答案，也可以先说说你的理解。",
            index + 1,
            questions.len(),
            question
        )
    }

    fn current_lesson_date(&self) -> Result<String> {
        Ok(self
            .current_local_time()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string())
    }

    fn current_local_time(&self) -> chrono::DateTime<FixedOffset> {
        let offset_seconds = self.config.timezone_offset_hours * 3600;
        let offset = FixedOffset::east_opt(offset_seconds)
            .or_else(|| FixedOffset::east_opt(8 * 3600))
            .expect("valid fallback fixed offset");
        Utc::now().with_timezone(&offset)
    }
}

impl LearningCommand {
    fn as_str(self) -> &'static str {
        match self {
            Self::StartTodayLesson => "start_today_lesson",
            Self::ExplainFocusSentence => "explain_focus_sentence",
            Self::NextQuestion => "next_question",
        }
    }
}

impl LessonStore {
    fn new(storage_dir: PathBuf) -> Self {
        Self { storage_dir }
    }

    fn load(&self, lesson_date: &str) -> Result<Option<DailyEnglishLesson>> {
        let path = self.lesson_path(lesson_date);
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read lesson file: {}", path.display()))?;
        let lesson = serde_json::from_str::<DailyEnglishLesson>(&raw)
            .with_context(|| format!("failed to parse lesson file: {}", path.display()))?;
        Ok(Some(lesson))
    }

    fn save(&self, lesson: &DailyEnglishLesson) -> Result<PathBuf> {
        let lessons_dir = self.storage_dir.join("lessons");
        fs::create_dir_all(&lessons_dir)
            .with_context(|| format!("failed to create lesson dir: {}", lessons_dir.display()))?;
        let path = self.lesson_path(&lesson.lesson_date);
        let payload = serde_json::to_string_pretty(lesson)
            .context("failed to serialize daily english lesson")?;
        fs::write(&path, payload)
            .with_context(|| format!("failed to write lesson file: {}", path.display()))?;
        Ok(path)
    }

    fn lesson_path(&self, lesson_date: &str) -> PathBuf {
        self.storage_dir
            .join("lessons")
            .join(format!("{lesson_date}.json"))
    }
}

impl LearningSessionStore {
    async fn get(&self, session_id: &str) -> Option<LearningSessionState> {
        let guard = self.inner.read().await;
        guard.get(session_id).cloned()
    }

    async fn start_session(
        &self,
        session_id: &str,
        _lesson_date: &str,
        focus_sentence: &str,
        next_question_index: usize,
    ) {
        let mut guard = self.inner.write().await;
        guard.insert(
            session_id.to_string(),
            LearningSessionState {
                focus_sentence: focus_sentence.to_string(),
                next_question_index,
            },
        );
    }
}

fn detect_learning_command(message: &str) -> Option<LearningCommand> {
    let normalized = message.split_whitespace().collect::<String>();
    if normalized.contains("开始今天的英语学习")
        || normalized.contains("开始英语学习")
        || normalized.contains("开始今天英语学习")
    {
        return Some(LearningCommand::StartTodayLesson);
    }
    if normalized.contains("这句话什么意思") || normalized.contains("这句什么意思") {
        return Some(LearningCommand::ExplainFocusSentence);
    }
    if normalized.contains("再出一道题")
        || normalized.contains("再来一道题")
        || normalized.contains("再出一题")
    {
        return Some(LearningCommand::NextQuestion);
    }
    None
}

fn parse_rss_feed(source_url: &str, raw_xml: &str) -> Result<Vec<NewsArticle>> {
    let feed: RssFeed = from_str(raw_xml).context("invalid RSS XML payload")?;
    let articles = feed
        .channel
        .items
        .into_iter()
        .filter_map(|item| {
            let title = item.title?.trim().to_string();
            let link = item.link?.trim().to_string();
            if title.is_empty() || link.is_empty() {
                return None;
            }
            let summary = strip_html_tags(item.description.as_deref().unwrap_or(""))
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            Some(NewsArticle {
                source_url: source_url.to_string(),
                title,
                link,
                summary: if summary.is_empty() {
                    "No summary available.".to_string()
                } else {
                    summary
                },
                published_at: item
                    .pub_date
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
            })
        })
        .collect::<Vec<_>>();
    if articles.is_empty() {
        bail!("rss feed did not contain any usable items");
    }
    Ok(articles)
}

fn strip_html_tags(input: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;

    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }

    output
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn normalize_dedupe_key(article: &NewsArticle) -> String {
    if !article.link.trim().is_empty() {
        article.link.trim().to_ascii_lowercase()
    } else {
        article.title.trim().to_ascii_lowercase()
    }
}

fn normalize_keywords(keywords: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for keyword in keywords {
        let value = keyword.trim();
        if value.is_empty() {
            continue;
        }
        let dedupe = value.to_ascii_lowercase();
        if seen.insert(dedupe) {
            normalized.push(value.to_string());
        }
    }
    if normalized.is_empty() {
        vec!["world news".to_string(), "english reading".to_string()]
    } else {
        normalized
    }
}

fn build_daily_lesson(digest: &NewsDigest, generated: GeneratedLessonFields) -> DailyEnglishLesson {
    let vocabulary = generated
        .vocabulary
        .into_iter()
        .filter(|item| !item.word.trim().is_empty() && !item.meaning_zh.trim().is_empty())
        .take(3)
        .collect::<Vec<_>>();
    let example_sentences = generated
        .example_sentences
        .into_iter()
        .filter(|item| !item.english.trim().is_empty() && !item.chinese.trim().is_empty())
        .take(2)
        .collect::<Vec<_>>();
    let questions = generated
        .questions
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .take(3)
        .collect::<Vec<_>>();
    let fallback_focus = example_sentences
        .first()
        .map(|item| item.english.clone())
        .unwrap_or_else(|| digest.article.title.clone());

    DailyEnglishLesson {
        lesson_date: digest.lesson_date.clone(),
        generated_at_ms: now_ms(),
        article: digest.article.clone(),
        headline_zh: coalesce_non_empty(&[
            generated.headline_zh.as_str(),
            digest.summary_zh.as_str(),
        ]),
        summary_en: coalesce_non_empty(&[
            generated.summary_en.as_str(),
            digest.summary_en.as_str(),
        ]),
        summary_zh: coalesce_non_empty(&[
            generated.summary_zh.as_str(),
            digest.summary_zh.as_str(),
        ]),
        keywords: if digest.keywords.is_empty() {
            vec!["english learning".to_string()]
        } else {
            digest.keywords.clone()
        },
        vocabulary,
        example_sentences,
        questions,
        shadowing_practice: coalesce_non_empty(&[
            generated.shadowing_practice.as_str(),
            "请大声跟读重点句子两遍，注意重音和停顿。",
        ]),
        translation_practice: coalesce_non_empty(&[
            generated.translation_practice.as_str(),
            "请把今日重点句子翻译成自然中文，再尝试反向译回英文。",
        ]),
        focus_sentence: if generated.focus_sentence.trim().is_empty() {
            fallback_focus
        } else {
            generated.focus_sentence.trim().to_string()
        },
    }
}

fn select_focus_sentence(lesson: &DailyEnglishLesson) -> String {
    if !lesson.focus_sentence.trim().is_empty() {
        return lesson.focus_sentence.clone();
    }
    lesson
        .example_sentences
        .first()
        .map(|item| item.english.clone())
        .unwrap_or_else(|| lesson.article.title.clone())
}

fn format_lesson_card(lesson: &DailyEnglishLesson, first_question: &str) -> String {
    let vocabulary = if lesson.vocabulary.is_empty() {
        "1. headline：标题\n2. summary：摘要\n3. context：背景".to_string()
    } else {
        lesson
            .vocabulary
            .iter()
            .enumerate()
            .map(|(index, item)| format!("{}. {}：{}", index + 1, item.word, item.meaning_zh))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let focus_sentence = lesson
        .example_sentences
        .iter()
        .find(|item| item.english == lesson.focus_sentence)
        .cloned()
        .unwrap_or_else(|| LearningSentence {
            english: select_focus_sentence(lesson),
            chinese: lesson.summary_zh.clone(),
        });

    format!(
        "今日英语学习卡片 {}\n\n新闻标题：{}\n中文导读：{}\n\n英文摘要：{}\n\n重点词汇：\n{}\n\n重点句子：\n{}\n{}\n\n理解题 1：\n{}\n\n跟读练习：{}\n翻译练习：{}\n\n你可以继续回复“这句话什么意思”或“再出一道题”。",
        lesson.lesson_date,
        lesson.article.title,
        lesson.headline_zh,
        lesson.summary_en,
        vocabulary,
        focus_sentence.english,
        focus_sentence.chinese,
        first_question,
        lesson.shadowing_practice,
        lesson.translation_practice
    )
}

fn news_digest_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "summary_en": { "type": "string" },
            "summary_zh": { "type": "string" },
            "keywords": {
                "type": "array",
                "items": { "type": "string" }
            }
        },
        "required": ["summary_en", "summary_zh", "keywords"]
    })
}

fn daily_lesson_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "headline_zh": { "type": "string" },
            "summary_en": { "type": "string" },
            "summary_zh": { "type": "string" },
            "vocabulary": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "word": { "type": "string" },
                        "meaning_zh": { "type": "string" },
                        "example_en": { "type": "string" },
                        "example_zh": { "type": "string" }
                    },
                    "required": ["word", "meaning_zh", "example_en", "example_zh"]
                }
            },
            "example_sentences": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "english": { "type": "string" },
                        "chinese": { "type": "string" }
                    },
                    "required": ["english", "chinese"]
                }
            },
            "questions": {
                "type": "array",
                "items": { "type": "string" }
            },
            "shadowing_practice": { "type": "string" },
            "translation_practice": { "type": "string" },
            "focus_sentence": { "type": "string" }
        },
        "required": [
            "headline_zh",
            "summary_en",
            "summary_zh",
            "vocabulary",
            "example_sentences",
            "questions",
            "shadowing_practice",
            "translation_practice",
            "focus_sentence"
        ]
    })
}

fn coalesce_non_empty(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_learning_commands() {
        assert!(matches!(
            detect_learning_command("开始今天的英语学习"),
            Some(LearningCommand::StartTodayLesson)
        ));
        assert!(matches!(
            detect_learning_command("这句话什么意思"),
            Some(LearningCommand::ExplainFocusSentence)
        ));
        assert!(matches!(
            detect_learning_command("再出一道题"),
            Some(LearningCommand::NextQuestion)
        ));
        assert!(detect_learning_command("普通聊天").is_none());
    }

    #[test]
    fn parses_rss_feed_items() {
        let xml = r#"
        <rss version="2.0">
          <channel>
            <item>
              <title>World leaders meet for climate talks</title>
              <link>https://example.com/news/1</link>
              <description><![CDATA[Leaders met to discuss climate action.]]></description>
              <pubDate>Mon, 24 Mar 2026 09:00:00 GMT</pubDate>
            </item>
          </channel>
        </rss>
        "#;

        let articles =
            parse_rss_feed("https://example.com/rss.xml", xml).expect("rss should parse");
        assert_eq!(articles.len(), 1);
        assert_eq!(articles[0].title, "World leaders meet for climate talks");
        assert_eq!(articles[0].link, "https://example.com/news/1");
    }

    #[test]
    fn formats_lesson_card_with_focus_sentence() {
        let lesson = DailyEnglishLesson {
            lesson_date: "2026-03-24".to_string(),
            generated_at_ms: 0,
            article: NewsArticle {
                source_url: "https://example.com/rss.xml".to_string(),
                title: "Markets rally after policy signal".to_string(),
                link: "https://example.com/news/2".to_string(),
                summary: "Markets rallied after a major policy signal.".to_string(),
                published_at: None,
            },
            headline_zh: "市场在政策信号后反弹".to_string(),
            summary_en: "Markets rallied after a major policy signal.".to_string(),
            summary_zh: "在重要政策信号后，市场出现反弹。".to_string(),
            keywords: vec!["market".to_string()],
            vocabulary: vec![LearningVocabulary {
                word: "rally".to_string(),
                meaning_zh: "反弹，上涨".to_string(),
                example_en: "Stocks rallied in the afternoon.".to_string(),
                example_zh: "股票在下午反弹。".to_string(),
            }],
            example_sentences: vec![LearningSentence {
                english: "Markets rallied after the announcement.".to_string(),
                chinese: "公告发布后，市场出现反弹。".to_string(),
            }],
            questions: vec!["Why did markets rally?".to_string()],
            shadowing_practice: "Read the focus sentence aloud twice.".to_string(),
            translation_practice: "Translate the focus sentence into Chinese.".to_string(),
            focus_sentence: "Markets rallied after the announcement.".to_string(),
        };

        let card = format_lesson_card(&lesson, "Why did markets rally?");
        assert!(card.contains("今日英语学习卡片"));
        assert!(card.contains("Markets rallied after the announcement."));
        assert!(card.contains("Why did markets rally?"));
    }
}
