//! Typed intents for the retained Ryogami provider boundary (#175).
//!
//! These values are requests, never authorization, recovery evidence or plans.
//! Source URLs, credentials, executables and destination directories cannot be
//! expressed. The trusted provider must resolve an item from its account-bound
//! search result before acquiring content. Local paths identify read-only import
//! candidates; they do not grant ownership or permission to remove those files.
use serde::{Deserialize, Serialize};

/// Metadata-only HTTP queries are distinct from local asset mutations and
/// process-backed providers. Their result is not a linked-checkpoint proof.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMetadataQuery {
    pub source: Source,
    pub search: Search,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMetadataCursorQuery {
    pub cursor: crate::Sha256,
}
impl SourceMetadataQuery {
    pub fn validate(&self) -> Result<(), RequestError> {
        if matches!(self.source, Source::Steam | Source::Youtube) {
            return Err(RequestError::SourceMismatch);
        }
        ProviderRequest::Search { source: self.source, search: Box::new(self.search.clone()) }
            .validate()
    }
}

/// Created by the authenticated helper from the exact owned settings file.
/// Public callers supply neither this digest nor a prepared actor frame.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedSourceMetadataQuery {
    pub expected_settings: Option<crate::Sha256>,
    pub query: SourceMetadataQuery,
}

/// One metadata call on a root-authenticated writer-control connection. It
/// cannot express a mutation frame, executable, output path or recovery claim.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMetadataCall {
    pub actor: crate::wallpaper_protocol::ActorIdentity,
    pub account_home_sha256: crate::Sha256,
    pub request_id: crate::Identifier,
    pub query: PreparedSourceMetadataQuery,
    pub rpc_id: i64,
}
impl SourceMetadataCall {
    pub fn validate(
        &self,
        actor: &crate::wallpaper_protocol::ActorIdentity,
        home: &str,
    ) -> Result<(), RequestError> {
        if &self.actor != actor
            || actor.pid == 0
            || actor.start_time == 0
            || self.rpc_id < 0
            || self.account_home_sha256.as_str()
                != crate::wallpaper_protocol::digest(home.as_bytes())
        {
            return Err(RequestError::Shape);
        }
        self.query.validate()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMetadataFailure {
    Configuration,
    Disabled,
    MissingCredentials,
    ProviderUnavailable,
    Timeout,
    InvalidResponse,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMetadataResponse {
    pub id: i64,
    pub request_id: crate::Identifier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<SourceMetadataPage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<SourceMetadataFailure>,
}
impl SourceMetadataResponse {
    pub fn validate(&self, call: &SourceMetadataCall) -> Result<(), RequestError> {
        self.validate_for_query(call.rpc_id, &call.request_id, &call.query.query)
    }
    pub fn validate_for_query(
        &self,
        id: i64,
        request_id: &crate::Identifier,
        query: &SourceMetadataQuery,
    ) -> Result<(), RequestError> {
        if self.id != id
            || &self.request_id != request_id
            || self.result.is_some() == self.error.is_some()
        {
            return Err(RequestError::Shape);
        }
        if let Some(result) = &self.result {
            result.validate(query)?;
        }
        Ok(())
    }
}
impl PreparedSourceMetadataQuery {
    pub fn validate(&self) -> Result<(), RequestError> {
        self.query.validate()
    }
}

pub const MAX_METADATA_RESULT_BYTES: usize = 256 * 1024;
pub const MAX_METADATA_ITEMS: usize = 256;

/// Private provider metadata, retained in protected account-bound receipts.
/// A URL in this value is data, never authority to fetch it: acquisition and
/// preview must also enforce the reviewed provider's endpoint policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceMetadataItem {
    pub id: String,
    pub full_url: String,
    pub thumb_url: String,
    pub resolution: String,
    pub file_size: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub track_url: String,
    #[serde(default)]
    pub attribution: String,
    #[serde(default)]
    pub attribution_url: String,
    #[serde(default)]
    pub duration_secs: u64,
    #[serde(default)]
    pub purity: String,
    #[serde(default)]
    pub category: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceMetadataPage {
    pub results: Vec<SourceMetadataItem>,
    pub last_page: u32,
    pub current_page: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_cursor: String,
}
impl SourceMetadataPage {
    pub fn validate(&self, query: &SourceMetadataQuery) -> Result<(), RequestError> {
        query.validate()?;
        if self.results.len() > MAX_METADATA_ITEMS
            || self.current_page == 0
            || self.current_page > i32::MAX as u32
            || self.last_page > i32::MAX as u32
            || !text(&self.next_cursor, 4096, true)
        {
            return Err(RequestError::Bounds);
        }
        for item in &self.results {
            let url = |value: &str, optional: bool| {
                (optional && value.is_empty())
                    || (text(value, 8192, false)
                        && value.starts_with("https://")
                        && !value.contains('\\'))
            };
            if !source_item_id_valid(query.source, &item.id)
                || !url(&item.full_url, false)
                || !url(&item.thumb_url, true)
                || !url(&item.track_url, true)
                || !url(&item.attribution_url, true)
                || item.title.len() > 2048
                || !text(&item.resolution, 64, true)
                || item.attribution.len() > 2048
                || !text(&item.purity, 64, true)
                || !text(&item.category, 64, true)
            {
                return Err(RequestError::Bounds);
            }
        }
        if serde_json::to_vec(self).map_err(|_| RequestError::Shape)?.len()
            > MAX_METADATA_RESULT_BYTES
        {
            return Err(RequestError::Frame);
        }
        Ok(())
    }
}

pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_QUERY_BYTES: usize = 512;
pub const MAX_ITEM_BYTES: usize = 512;
pub const MAX_PAGE: u32 = 10_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Wallhaven,
    Steam,
    Unsplash,
    Pexels,
    Youtube,
    Bing,
    Moewalls,
    Motionbgs,
    Ryostore,
    Repos,
}
impl Source {
    pub const ALL: [Self; 10] = [
        Self::Wallhaven,
        Self::Steam,
        Self::Unsplash,
        Self::Pexels,
        Self::Youtube,
        Self::Bing,
        Self::Moewalls,
        Self::Motionbgs,
        Self::Ryostore,
        Self::Repos,
    ];
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderRequest {
    Search { source: Source, search: Box<Search> },
    Download { item: RemoteItem, clip: Option<Clip> },
    Preview { item: RemoteItem },
    Cancel { operation_id: String },
    Collections { username: String },
    ImportModel { path: String },
    AddModel { path: Option<String> },
    ForgetModel { index: u32 },
    DeleteModel { selection: ModelSelection },
    DeleteMedia { key: String },
    RestoreMedia { receipt: String },
    Library { action: LibraryAction },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteItem {
    pub source: Source,
    pub id: String,
    /// Server-issued search receipt identity, scoped to the authenticated UID.
    /// A provider must independently verify it; its spelling grants no access.
    pub selection: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clip {
    pub start: f64,
    pub duration: f64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "selection", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelSelection {
    Active,
    Index { index: u32 },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum LibraryAction {
    Rescan,
    ClearCache,
    ClearVideoCache,
    RecomputeColors,
    CaptureWorkshopThumbnails,
    OptimizeImages,
    ConvertVideos { preset: VideoPreset, resolution: VideoResolution },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoPreset {
    Balanced,
    Quality,
    Light,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoResolution {
    #[serde(rename = "1080p")]
    FullHd,
    #[serde(rename = "2k")]
    Qhd,
    #[serde(rename = "4k")]
    Uhd,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Search {
    pub query: String,
    pub page: u32,
    pub generation: u64,
    pub filters: SearchFilters,
}

/// Each provider has its own closed filter shape. A generic map would permit
/// silently forwarding a URL/path/tool override added upstream in the future.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "lowercase", deny_unknown_fields)]
pub enum SearchFilters {
    Wallhaven {
        categories: String,
        purity: String,
        sorting: String,
        order: String,
        top_range: String,
        atleast: String,
        atmost: String,
        resolutions: String,
        ratios: String,
        colors: String,
        collection: String,
    },
    Steam {
        query_type: u8,
        trend_days: u16,
        required_tags: Vec<String>,
        excluded_tags: Vec<String>,
        num_per_page: u16,
        allow_nsfw: bool,
    },
    Unsplash {
        order_by: String,
        content_filter: String,
        orientation: String,
        color: String,
    },
    Pexels {
        orientation: String,
        size: String,
        color: String,
    },
    Youtube {
        max_duration: u32,
    },
    Bing {
        market: String,
    },
    Moewalls,
    Motionbgs,
    Ryostore,
    Repos {
        repository: String,
        branch: String,
        path: String,
        media: RepositoryMedia,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepositoryMedia {
    All,
    Image,
    Video,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestError {
    Frame,
    Shape,
    Bounds,
    SourceMismatch,
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never echo private search text, an import path or account settings.
        write!(f, "invalid Ryogami provider request: {self:?}")
    }
}
impl std::error::Error for RequestError {}

pub fn decode(bytes: &[u8]) -> Result<ProviderRequest, RequestError> {
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        return Err(RequestError::Frame);
    }
    let request: ProviderRequest =
        serde_json::from_slice(bytes).map_err(|_| RequestError::Shape)?;
    request.validate()?;
    Ok(request)
}

impl ProviderRequest {
    pub fn validate(&self) -> Result<(), RequestError> {
        let valid = match self {
            Self::Search { source, search } => {
                if *source != search.filters.source() {
                    return Err(RequestError::SourceMismatch);
                }
                text(&search.query, MAX_QUERY_BYTES, true)
                    && (1..=MAX_PAGE).contains(&search.page)
                    && search.generation <= i64::MAX as u64
                    && search.filters.valid()
            }
            Self::Download { item, clip } => {
                item.valid()
                    && clip.is_none_or(|clip| {
                        item.source == Source::Youtube
                            && clip.start.is_finite()
                            && clip.duration.is_finite()
                            && (0.0..=86400.0).contains(&clip.start)
                            && (0.001..=3600.0).contains(&clip.duration)
                            && clip.start + clip.duration <= 86400.0
                    })
            }
            Self::Preview { item } => item.valid() && item.source != Source::Steam,
            Self::Cancel { operation_id } => identifier(operation_id, 128),
            Self::Collections { username } => identifier(username, 64),
            Self::ImportModel { path } => import_path(path),
            Self::AddModel { path } => path.as_deref().is_none_or(import_path),
            Self::ForgetModel { index } => *index < 4096,
            Self::DeleteModel { selection } => match selection {
                ModelSelection::Active => true,
                ModelSelection::Index { index } => *index < 4096,
            },
            Self::DeleteMedia { key } => text(key, 4096, false),
            Self::RestoreMedia { receipt } => digest(receipt),
            Self::Library { .. } => true,
        };
        valid.then_some(()).ok_or(RequestError::Bounds)
    }
}
impl RemoteItem {
    fn valid(&self) -> bool {
        digest(&self.selection) && source_item_id_valid(self.source, &self.id)
    }
}
fn source_item_id_valid(source: Source, id: &str) -> bool {
    text(id, MAX_ITEM_BYTES, false)
        && match source {
            Source::Steam => {
                id.len() <= 20
                    && id.bytes().all(|byte| byte.is_ascii_digit())
                    && id.parse::<u64>().is_ok_and(|value| value != 0 && value.to_string() == id)
            }
            Source::Youtube => id.len() == 11 && identifier(id, 11),
            // Remote identifiers never become a relative filename or URL.
            _ => !id.contains("://"),
        }
}
impl SearchFilters {
    pub fn source(&self) -> Source {
        match self {
            Self::Wallhaven { .. } => Source::Wallhaven,
            Self::Steam { .. } => Source::Steam,
            Self::Unsplash { .. } => Source::Unsplash,
            Self::Pexels { .. } => Source::Pexels,
            Self::Youtube { .. } => Source::Youtube,
            Self::Bing { .. } => Source::Bing,
            Self::Moewalls => Source::Moewalls,
            Self::Motionbgs => Source::Motionbgs,
            Self::Ryostore => Source::Ryostore,
            Self::Repos { .. } => Source::Repos,
        }
    }
    fn valid(&self) -> bool {
        match self {
            Self::Wallhaven {
                categories,
                purity,
                sorting,
                order,
                top_range,
                atleast,
                atmost,
                resolutions,
                ratios,
                colors,
                collection,
            } => {
                [categories, purity]
                    .into_iter()
                    .all(|s| s.len() == 3 && s.bytes().all(|b| matches!(b, b'0' | b'1')))
                    && ["date_added", "relevance", "random", "views", "favorites", "toplist", "hot"]
                        .contains(&sorting.as_str())
                    && ["asc", "desc"].contains(&order.as_str())
                    && ["1d", "3d", "1w", "1M", "3M", "6M", "1y"].contains(&top_range.as_str())
                    && [atleast, atmost, resolutions, ratios, colors, collection]
                        .into_iter()
                        .all(|s| text(s, 256, true))
            }
            Self::Steam {
                query_type,
                trend_days,
                required_tags,
                excluded_tags,
                num_per_page,
                ..
            } => {
                [0, 1, 3, 9, 21].contains(query_type)
                    && (1..=365).contains(trend_days)
                    && (1..=100).contains(num_per_page)
                    && [required_tags, excluded_tags]
                        .into_iter()
                        .all(|tags| tags.len() <= 32 && tags.iter().all(|tag| text(tag, 64, false)))
            }
            Self::Unsplash { order_by, content_filter, orientation, color } => {
                ["relevant", "latest"].contains(&order_by.as_str())
                    && ["low", "high"].contains(&content_filter.as_str())
                    && ["", "landscape", "portrait", "squarish"].contains(&orientation.as_str())
                    && text(color, 32, true)
            }
            Self::Pexels { orientation, size, color } => {
                ["", "landscape", "portrait", "square"].contains(&orientation.as_str())
                    && ["", "large", "medium", "small"].contains(&size.as_str())
                    && text(color, 32, true)
            }
            Self::Youtube { max_duration } => *max_duration <= 86400,
            Self::Bing { market } => {
                market.is_empty()
                    || (market.len() == 5
                        && market.as_bytes()[2] == b'-'
                        && market
                            .bytes()
                            .enumerate()
                            .all(|(i, b)| i == 2 || b.is_ascii_alphabetic()))
            }
            Self::Moewalls | Self::Motionbgs | Self::Ryostore => true,
            Self::Repos { repository, branch, path, .. } => {
                let parts: Vec<_> = repository.split('/').collect();
                parts.len() == 2
                    && parts.iter().all(|s| {
                        !s.is_empty()
                            && s.len() <= 100
                            && !matches!(*s, "." | "..")
                            && s.bytes().all(|b| {
                                b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')
                            })
                    })
                    && relative(branch, true)
                    && relative(path, true)
            }
        }
    }
}
fn text(value: &str, max: usize, empty: bool) -> bool {
    (empty || !value.is_empty()) && value.len() <= max && !value.chars().any(char::is_control)
}
fn identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn relative(value: &str, empty: bool) -> bool {
    (empty && value.is_empty())
        || (text(value, 512, false)
            && value.split('/').all(|s| {
                !s.is_empty()
                    && s != "."
                    && s != ".."
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
            }))
}
fn import_path(value: &str) -> bool {
    value.starts_with('/')
        && text(value, 4096, false)
        && value[1..].split('/').all(|part| !part.is_empty() && !matches!(part, "." | ".."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata_call() -> SourceMetadataCall {
        SourceMetadataCall {
            actor: crate::wallpaper_protocol::ActorIdentity { pid: 42, start_time: 100 },
            account_home_sha256: crate::Sha256::new(crate::wallpaper_protocol::digest(
                b"/home/fixture",
            ))
            .unwrap(),
            request_id: crate::Identifier::new("source-fixture").unwrap(),
            rpc_id: 7,
            query: PreparedSourceMetadataQuery {
                expected_settings: None,
                query: SourceMetadataQuery {
                    source: Source::Bing,
                    search: Search {
                        query: "fixture".into(),
                        page: 1,
                        generation: 2,
                        filters: SearchFilters::Bing { market: "en-US".into() },
                    },
                },
            },
        }
    }
    #[test]
    fn metadata_call_binds_actor_home_request_and_http_only_intent_without_mutation_fields() {
        let call = metadata_call();
        assert!(call.validate(&call.actor, "/home/fixture").is_ok());
        assert!(call.validate(&call.actor, "/home/other").is_err());
        let mut actor = call.actor.clone();
        actor.start_time += 1;
        assert!(call.validate(&actor, "/home/fixture").is_err());
        for source in [Source::Steam, Source::Youtube] {
            let mut changed = call.clone();
            changed.query.query.source = source;
            assert!(changed.validate(&changed.actor, "/home/fixture").is_err());
        }
        for key in ["frame", "command", "url", "destination", "checkpoint", "plan_sha256"] {
            let mut value = serde_json::to_value(&call).unwrap();
            value[key] = serde_json::json!("injected");
            assert!(serde_json::from_value::<SourceMetadataCall>(value).is_err(), "{key}");
        }
    }
    #[test]
    fn metadata_response_preserves_empty_pages_and_display_data_but_requires_exact_correlation() {
        let call = metadata_call();
        let response = SourceMetadataResponse {
            id: 7,
            request_id: call.request_id.clone(),
            result: Some(SourceMetadataPage {
                results: vec![],
                last_page: 0,
                current_page: 1,
                next_cursor: String::new(),
            }),
            error: None,
        };
        assert!(response.validate(&call).is_ok());
        let mut wrong = response.clone();
        wrong.id += 1;
        assert!(wrong.validate(&call).is_err());
        let mut wrong = response.clone();
        wrong.request_id = crate::Identifier::new("source-other").unwrap();
        assert!(wrong.validate(&call).is_err());
        let mut wrong = response.clone();
        wrong.error = Some(SourceMetadataFailure::Timeout);
        assert!(wrong.validate(&call).is_err());
        let mut wrong = response.clone();
        wrong.result = None;
        assert!(wrong.validate(&call).is_err());
        let mut wrong = response;
        wrong.result.as_mut().unwrap().current_page = 0;
        assert!(wrong.validate(&call).is_err());
    }
    fn item(source: Source) -> RemoteItem {
        RemoteItem {
            source,
            id: match source {
                Source::Steam => "12345",
                Source::Youtube => "abcdefghijk",
                _ => "fixture",
            }
            .into(),
            selection: "a".repeat(64),
        }
    }
    #[test]
    fn all_retained_providers_have_typed_downloads_and_roundtrip() {
        for source in Source::ALL {
            let request = ProviderRequest::Download { item: item(source), clip: None };
            assert_eq!(decode(&serde_json::to_vec(&request).unwrap()), Ok(request));
        }
    }
    #[test]
    fn client_cannot_inject_url_command_destination_or_checkpoint() {
        let request = ProviderRequest::Download { item: item(Source::Wallhaven), clip: None };
        for key in [
            "url",
            "fullUrl",
            "trackUrl",
            "destination",
            "root",
            "command",
            "checkpoint",
            "credentials",
        ] {
            let mut value = serde_json::to_value(&request).unwrap();
            value[key] = "untrusted".into();
            assert_eq!(
                decode(&serde_json::to_vec(&value).unwrap()),
                Err(RequestError::Shape),
                "{key}"
            );
            let mut value = serde_json::to_value(&request).unwrap();
            value["item"][key] = "untrusted".into();
            assert_eq!(
                decode(&serde_json::to_vec(&value).unwrap()),
                Err(RequestError::Shape),
                "nested {key}"
            );
        }
    }
    #[test]
    fn malformed_oversized_and_duplicate_fields_refuse() {
        assert_eq!(decode(&vec![b' '; MAX_FRAME_BYTES + 1]), Err(RequestError::Frame));
        assert_eq!(
            decode(br#"{"operation":"cancel","operation_id":"a","operation_id":"b"}"#),
            Err(RequestError::Shape)
        );
        assert!(decode(br#"{"operation":"cancel","operation_id":"bad\nsecret"}"#).is_err());
        assert!(decode(br#"{"operation":"cancel","operation_id":"a"}{}"#).is_err());
    }
    #[test]
    fn clips_and_workshop_ids_are_bounded() {
        for id in ["0", "01", "-1", "18446744073709551616", "https://steamcommunity.com/1"] {
            let mut selection = item(Source::Steam);
            selection.id = id.into();
            assert!(ProviderRequest::Download { item: selection, clip: None }.validate().is_err());
        }
        for (source, start, duration) in [
            (Source::Wallhaven, 0.0, 1.0),
            (Source::Youtube, -1.0, 1.0),
            (Source::Youtube, 0.0, 0.0),
            (Source::Youtube, 0.0, f64::NAN),
            (Source::Youtube, f64::INFINITY, 2.0),
        ] {
            assert!(
                ProviderRequest::Download {
                    item: item(source),
                    clip: Some(Clip { start, duration })
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            ProviderRequest::Download {
                item: item(Source::Youtube),
                clip: Some(Clip { start: 0.0, duration: 30.0 })
            }
            .validate()
            .is_ok()
        );
    }
    #[test]
    fn model_paths_are_only_bounded_absolute_import_intents() {
        for path in ["relative", "/home/../etc/shadow", "/home//x", "/", "/tmp/x\0secret"] {
            assert!(ProviderRequest::ImportModel { path: path.into() }.validate().is_err());
        }
        assert!(
            ProviderRequest::ImportModel { path: "/home/user/My Models/semantic-pack.json".into() }
                .validate()
                .is_ok()
        );
        assert!(ProviderRequest::AddModel { path: None }.validate().is_ok());
    }
    #[test]
    fn search_cannot_switch_provider_or_unbound_repository() {
        let mut request = ProviderRequest::Search {
            source: Source::Bing,
            search: Box::new(Search {
                query: String::new(),
                page: 1,
                generation: 0,
                filters: SearchFilters::Ryostore,
            }),
        };
        assert_eq!(request.validate(), Err(RequestError::SourceMismatch));
        if let ProviderRequest::Search { source, search } = &mut request {
            *source = Source::Repos;
            search.filters = SearchFilters::Repos {
                repository: "owner/repo".into(),
                branch: "main".into(),
                path: "images/landscape".into(),
                media: RepositoryMedia::All,
            };
        }
        assert!(request.validate().is_ok());
        if let ProviderRequest::Search { search, .. } = &mut request {
            search.filters = SearchFilters::Repos {
                repository: "https://evil/metadata".into(),
                branch: "main".into(),
                path: "../etc".into(),
                media: RepositoryMedia::All,
            };
        }
        assert!(request.validate().is_err());
    }
    #[test]
    fn requests_and_errors_do_not_echo_private_values() {
        let error =
            ProviderRequest::ImportModel { path: "private-secret".into() }.validate().unwrap_err();
        assert!(!error.to_string().contains("private-secret"));
    }
}

#[cfg(test)]
mod source_contract {
    #[test]
    fn reviewed_provider_paths_match_the_committed_v1_source_manifest() {
        let manifest =
            include_str!("../../../upstream/evidence/ryoku-v1.0/writer-source-manifest.tsv");
        let selected = include_str!(
            "../../../upstream/evidence/ryoku-v1.0/issue175-provider-source-identities.tsv"
        );
        let rows: std::collections::BTreeSet<_> = selected.lines().collect();
        assert_eq!(rows.len(), 58);
        for row in &rows {
            assert!(manifest.lines().any(|line| line == *row), "unbound source identity");
        }
        for required in [
            "daemon/rpc_sources.go",
            "daemon/rpc_workshop.go",
            "daemon/lens_models.go",
            "daemon/library_trash.go",
            "daemon/source_registry.go",
            "picker/src/core/remoteresults.cpp",
            "picker/qml/SheetHost.qml",
        ] {
            assert!(
                rows.iter()
                    .any(|row| row.starts_with(&format!("ryoku/shell/ryogami/{required}\t"))),
                "missing {required}"
            );
        }
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;
    #[test]
    fn every_retained_search_filter_has_a_valid_bounded_intent() {
        let filters = [
            SearchFilters::Wallhaven {
                categories: "111".into(),
                purity: "100".into(),
                sorting: "toplist".into(),
                order: "desc".into(),
                top_range: "1M".into(),
                atleast: String::new(),
                atmost: String::new(),
                resolutions: String::new(),
                ratios: String::new(),
                colors: String::new(),
                collection: String::new(),
            },
            SearchFilters::Steam {
                query_type: 3,
                trend_days: 7,
                required_tags: vec!["Scene".into()],
                excluded_tags: vec![],
                num_per_page: 30,
                allow_nsfw: false,
            },
            SearchFilters::Unsplash {
                order_by: "relevant".into(),
                content_filter: "high".into(),
                orientation: String::new(),
                color: String::new(),
            },
            SearchFilters::Pexels {
                orientation: String::new(),
                size: String::new(),
                color: String::new(),
            },
            SearchFilters::Youtube { max_duration: 0 },
            SearchFilters::Bing { market: "en-US".into() },
            SearchFilters::Moewalls,
            SearchFilters::Motionbgs,
            SearchFilters::Ryostore,
            SearchFilters::Repos {
                repository: "owner/wallpapers.v1".into(),
                branch: String::new(),
                path: String::new(),
                media: RepositoryMedia::All,
            },
        ];
        assert_eq!(filters.len(), Source::ALL.len());
        for filters in filters {
            let source = filters.source();
            let mut search = Search { query: "βουνό".into(), page: 1, generation: 2, filters };
            assert!(
                ProviderRequest::Search { source, search: Box::new(search.clone()) }
                    .validate()
                    .is_ok()
            );
            search.page = 0;
            assert!(
                ProviderRequest::Search { source, search: Box::new(search.clone()) }
                    .validate()
                    .is_err()
            );
            search.page = 1;
            search.query = "x".repeat(MAX_QUERY_BYTES + 1);
            assert!(
                ProviderRequest::Search { source, search: Box::new(search) }.validate().is_err()
            );
        }
    }
    #[test]
    fn native_converter_options_are_preserved_without_arbitrary_process_arguments() {
        for preset in ["light", "balanced", "quality"] {
            for resolution in ["1080p", "2k", "4k"] {
                let value = serde_json::json!({"operation":"library","action":{"action":"convert_videos","preset":preset,"resolution":resolution}});
                assert!(decode(&serde_json::to_vec(&value).unwrap()).is_ok());
            }
        }
        assert!(decode(br#"{"operation":"library","action":{"action":"convert_videos","preset":"-y /private","resolution":"4k"}}"#).is_err());
    }
}
